//! TOTP against RFC 6238's own vectors, and the two things an implementation
//! that passes them can still get wrong: skew, and replay inside a step.

mod common;

use brocanth_auth::clock::Clock;
use brocanth_auth::error::AuthError;
use brocanth_auth::identity::{AccountId, EmailAddress};
use brocanth_auth::impersonation::AccountHolder;
use brocanth_auth::mfa::{FactorKind, FactorSet};
use brocanth_auth::storage::{AuthStore, NewAccount};
use brocanth_auth::totp::{Totp, TotpSecret};
use common::Fixture;

/// The seed RFC 6238 Appendix B uses for its SHA-1 vectors: ASCII
/// "12345678901234567890".
fn rfc_secret() -> TotpSecret {
    TotpSecret::from_bytes(b"12345678901234567890".to_vec())
}

async fn an_account(f: &Fixture) -> AccountId {
    f.store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse("ada@example.com").unwrap()),
            email_verified: true,
            password_hash: None,
        })
        .await
        .unwrap()
        .id
}

#[test]
fn the_published_rfc_6238_vectors_all_match() {
    // Appendix B, the SHA-1 column. Eight digits, which is why `custom` has to
    // exist: nothing else in this crate deviates from six.
    let totp = Totp::custom(8, 30, 0).unwrap();
    let secret = rfc_secret();
    for (time, expected) in [
        (59_i64, "94287082"),
        (1_111_111_109, "07081804"),
        (1_111_111_111, "14050471"),
        (1_234_567_890, "89005924"),
        (2_000_000_000, "69279037"),
        (20_000_000_000, "65353130"),
    ] {
        assert_eq!(totp.code_at(&secret, time), expected, "at T={time}");
    }
}

#[test]
fn the_interoperable_defaults_are_the_ones_every_app_assumes() {
    let totp = Totp::interoperable();
    assert_eq!((totp.digits(), totp.step_seconds()), (6, 30));
    // Six digits is the last six of the eight-digit vector at the same step,
    // because truncation happens once and the digit count is a modulus.
    assert_eq!(totp.code_at(&rfc_secret(), 59), "287082");
}

#[test]
fn a_clock_one_step_out_still_works_and_two_steps_out_does_not() {
    let totp = Totp::interoperable();
    let secret = rfc_secret();
    let now = 1_700_000_000;
    let step = totp.step_seconds();

    for drift in [-step, 0, step] {
        let phone = totp.code_at(&secret, now + drift);
        assert!(
            totp.matching_step(&secret, &phone, now).is_some(),
            "a phone {drift}s out was refused"
        );
    }
    for drift in [-2 * step, 2 * step] {
        let phone = totp.code_at(&secret, now + drift);
        assert!(
            totp.matching_step(&secret, &phone, now).is_none(),
            "a phone {drift}s out was accepted; the window is wider than it says"
        );
    }
}

#[test]
fn the_step_a_code_belongs_to_comes_back_rather_than_a_bool() {
    // This is the whole reason replay prevention is possible: `true` tells the
    // caller nothing it can spend.
    let totp = Totp::interoperable();
    let secret = rfc_secret();
    let now = 1_700_000_000;
    let previous = totp.step_at(now) - 1;
    let code = totp.code_for_step(&secret, previous);
    assert_eq!(totp.matching_step(&secret, &code, now), Some(previous));
}

#[tokio::test]
async fn a_correct_code_cannot_be_used_twice_inside_its_own_step() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let who = AccountHolder::without_a_session(account.clone());

    let enrolment = mfa
        .begin_totp_enrolment(&who, "Brocanth", "ada@example.com")
        .await
        .unwrap();
    // Enrolment is confirmed with one code, which spends that step too.
    let confirming = mfa.totp().code_at(&enrolment.secret, f.clock.now());
    mfa.confirm_totp_enrolment(&who, &confirming).await.unwrap();

    // Move to a fresh step and sign in with it.
    f.clock.advance(mfa.totp().step_seconds());
    let code = mfa.totp().code_at(&enrolment.secret, f.clock.now());
    let proof = mfa.verify_totp(&account, &code).await.expect("first use");
    assert_eq!(proof.account(), &account);

    // The same code is still *correct* for another twenty-odd seconds. That is
    // the window a shoulder surfer, a phishing proxy or a log reader works in,
    // and it is the one almost every implementation leaves open.
    let err = mfa.verify_totp(&account, &code).await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");

    // Still within the same step: not an expiry, a replay.
    f.clock.advance(1);
    let err = mfa.verify_totp(&account, &code).await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
}

#[tokio::test]
async fn the_code_the_enrolment_form_ate_cannot_be_replayed_into_the_sign_in_form() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let who = AccountHolder::without_a_session(account.clone());

    let enrolment = mfa
        .begin_totp_enrolment(&who, "Brocanth", "ada@example.com")
        .await
        .unwrap();
    let code = mfa.totp().code_at(&enrolment.secret, f.clock.now());
    mfa.confirm_totp_enrolment(&who, &code).await.unwrap();

    let err = mfa.verify_totp(&account, &code).await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
}

#[tokio::test]
async fn an_older_step_inside_the_window_is_refused_once_a_newer_one_was_spent() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let who = AccountHolder::without_a_session(account.clone());

    let enrolment = mfa
        .begin_totp_enrolment(&who, "Brocanth", "ada@example.com")
        .await
        .unwrap();
    mfa.confirm_totp_enrolment(&who, &mfa.totp().code_at(&enrolment.secret, f.clock.now()))
        .await
        .unwrap();

    f.clock.advance(mfa.totp().step_seconds() * 4);
    let now = f.clock.now();
    let current = mfa.totp().code_at(&enrolment.secret, now);
    let previous = mfa
        .totp()
        .code_at(&enrolment.secret, now - mfa.totp().step_seconds());

    let proof = mfa.verify_totp(&account, &current).await.expect("current");
    assert_eq!(proof.kinds(), FactorSet::of([FactorKind::Totp]));
    // The previous step is still inside the skew window and its code is still
    // arithmetically correct. The claim is monotonic, so it is refused anyway.
    let err = mfa.verify_totp(&account, &previous).await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
}

#[test]
fn a_secret_round_trips_through_the_uri_a_phone_scans() {
    let secret = TotpSecret::generate().unwrap();
    let uri = secret.otpauth_uri(&Totp::interoperable(), "Brocanth", "ada@example.com");
    let encoded = uri
        .split("secret=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap();
    assert_eq!(TotpSecret::from_base32(encoded).unwrap(), secret);
}
