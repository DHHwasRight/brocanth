//! The factor set, the sufficiency rule, and recovery codes.

mod common;

use brocanth_auth::clock::Clock;
use brocanth_auth::error::AuthError;
use brocanth_auth::identity::{AccountId, EmailAddress};
use brocanth_auth::impersonation::AccountHolder;
use brocanth_auth::mfa::{
    Admission, Enrolled, FactorClass, FactorKind, FactorPolicy, FactorSet, TotpEnrolment, Verified,
};
use brocanth_auth::recovery::RecoveryCodes;
use brocanth_auth::session::{Privilege, SigningKey};
use brocanth_auth::storage::{AuthStore, NewAccount};
use common::Fixture;

async fn an_account(f: &Fixture) -> AccountId {
    f.store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse("ada@example.com").unwrap()),
            email_verified: true,
            password_hash: Some("$argon2id$not-a-real-hash".into()),
        })
        .await
        .unwrap()
        .id
}

/// Enrols TOTP and returns the enrolment plus the recovery codes.
async fn enrol(f: &Fixture, account: &AccountId) -> (TotpEnrolment, RecoveryCodes) {
    let mfa = f.mfa();
    let who = AccountHolder::without_a_session(account.clone());
    let enrolment = mfa
        .begin_totp_enrolment(&who, "Brocanth", "ada@example.com")
        .await
        .unwrap();
    let code = mfa.totp().code_at(&enrolment.secret, f.clock.now());
    let codes = mfa.confirm_totp_enrolment(&who, &code).await.unwrap();
    (enrolment, codes)
}

// ---------------------------------------------------------------------------
// The set and the rule
// ---------------------------------------------------------------------------

#[test]
fn every_kind_has_exactly_one_class_and_a_stable_name() {
    for kind in FactorKind::ALL {
        assert_eq!(FactorKind::parse(kind.as_str()), Some(kind));
    }
    assert_eq!(FactorKind::Password.class(), FactorClass::Primary);
    assert_eq!(FactorKind::Totp.class(), FactorClass::Second);
    // The kind nothing implements yet still has to be carried, and it is
    // standalone because that is what makes a passkey worth having.
    assert_eq!(FactorKind::Passkey.class(), FactorClass::Standalone);
}

/// An account id to hang the witnesses off. The policy never reads it beyond
/// checking that the proof and the enrolment agree about whose they are.
fn whom() -> AccountId {
    AccountId::new("acct-1")
}

fn proof(kinds: &[FactorKind]) -> Verified {
    Verified::without_a_credential_check(whom(), FactorSet::of(kinds.iter().copied()))
}

fn enrolled(kinds: &[FactorKind]) -> Enrolled {
    Enrolled::without_asking_the_store(whom(), FactorSet::of(kinds.iter().copied()))
}

#[test]
fn a_password_alone_is_enough_for_an_account_that_enrolled_nothing() {
    let policy = FactorPolicy::recommended();
    match policy.admit(&enrolled(&[]), proof(&[FactorKind::Password])) {
        Admission::Admit(admitted) => {
            assert_eq!(admitted.account(), &whom());
            assert_eq!(admitted.factors(), FactorSet::of([FactorKind::Password]));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_password_alone_is_not_enough_once_a_second_factor_exists() {
    let policy = FactorPolicy::recommended();
    let has = enrolled(&[FactorKind::Totp, FactorKind::RecoveryCode]);

    match policy.admit(&has, proof(&[FactorKind::Password])) {
        Admission::Incomplete { candidates, proof } => {
            assert!(candidates.contains(FactorKind::Totp));
            assert!(candidates.contains(FactorKind::RecoveryCode));
            assert!(!candidates.contains(FactorKind::Password));
            // The proof is handed back, because the flow has to carry it into
            // the second step and there is nothing else to carry it in.
            assert_eq!(proof.kinds(), FactorSet::of([FactorKind::Password]));
        }
        other => panic!("a password alone was accepted: {other:?}"),
    }

    for second in [FactorKind::Totp, FactorKind::RecoveryCode] {
        let admission = policy.admit(&has, proof(&[FactorKind::Password, second]));
        assert!(
            matches!(admission, Admission::Admit(_)),
            "{second} did not complete the sign-in: {admission:?}"
        );
    }
}

#[test]
fn a_second_factor_without_a_primary_one_is_not_a_sign_in() {
    let policy = FactorPolicy::recommended();
    assert_eq!(
        policy.admit(&enrolled(&[FactorKind::Totp]), proof(&[FactorKind::Totp])),
        Admission::Insufficient
    );
}

#[test]
fn a_proof_for_one_account_cannot_admit_another() {
    let policy = FactorPolicy::recommended();
    let somebody_else = Verified::without_a_credential_check(
        AccountId::new("acct-2"),
        FactorSet::of([FactorKind::Password]),
    );
    assert_eq!(
        policy.admit(&enrolled(&[]), somebody_else),
        Admission::Insufficient
    );
    // And the same refusal in the merge, before it ever reaches the policy.
    assert!(proof(&[FactorKind::Password])
        .and(Verified::without_a_credential_check(
            AccountId::new("acct-2"),
            FactorSet::of([FactorKind::Totp])
        ))
        .is_err());
}

#[test]
fn a_passkey_stands_alone() {
    let policy = FactorPolicy::recommended();
    assert!(matches!(
        policy.admit(
            &enrolled(&[FactorKind::Totp]),
            proof(&[FactorKind::Passkey])
        ),
        Admission::Admit(_)
    ));
}

#[test]
fn demanding_a_second_factor_from_everybody_sends_the_unenrolled_to_enrolment() {
    let policy = FactorPolicy::recommended().requiring_a_second_factor_always();
    assert!(matches!(
        policy.admit(&enrolled(&[]), proof(&[FactorKind::Password])),
        Admission::EnrolmentRequired { .. }
    ));
}

#[test]
fn a_sign_in_never_grants_elevation_however_many_factors_it_had() {
    let policy = FactorPolicy::recommended();
    let admitted = match policy.admit(
        &enrolled(&[FactorKind::Totp]),
        proof(&[FactorKind::Password, FactorKind::Totp]),
    ) {
        Admission::Admit(a) => a,
        other => panic!("{other:?}"),
    };
    // `Admitted` carries no privilege at all, and `begin_with_factors` always
    // opens an `Authenticated` session. Elevation is a separate, later proof —
    // which is why `Elevation` is a different type.
    assert_eq!(
        admitted.factors(),
        FactorSet::of([FactorKind::Password, FactorKind::Totp])
    );
    assert!(Privilege::Authenticated < Privilege::Elevated);
}

#[test]
fn elevation_needs_a_fresh_second_factor_and_a_recovery_code_is_not_one() {
    let policy = FactorPolicy::recommended();
    let has = enrolled(&[FactorKind::Totp, FactorKind::RecoveryCode]);

    assert!(policy
        .for_elevation(&has, proof(&[FactorKind::Totp]))
        .is_some());
    assert!(policy
        .for_elevation(&has, proof(&[FactorKind::Passkey]))
        .is_some());
    // The paper in the drawer does not open the dangerous door.
    assert!(policy
        .for_elevation(&has, proof(&[FactorKind::RecoveryCode]))
        .is_none());
    assert!(policy
        .for_elevation(&has, proof(&[FactorKind::Password]))
        .is_none());

    // Unless the application said so, in writing.
    let lenient = FactorPolicy::recommended().allowing_recovery_codes_to_elevate();
    assert!(lenient
        .for_elevation(&has, proof(&[FactorKind::RecoveryCode]))
        .is_some());
}

#[test]
fn an_account_with_only_a_password_can_still_elevate_with_it() {
    // Otherwise nobody without a phone could ever change their own password.
    let policy = FactorPolicy::recommended();
    let granted = policy
        .for_elevation(&enrolled(&[]), proof(&[FactorKind::Password]))
        .expect("a password is the strongest proof such an account has");
    assert_eq!(granted.account(), &whom());
    assert_eq!(granted.factors(), FactorSet::of([FactorKind::Password]));
}

#[test]
fn a_sealed_proof_survives_the_round_trip_and_nothing_else_does() {
    // A two-step sign-in is two HTTP requests. Without this, the second one
    // would have to call `without_a_credential_check`, which would hollow out
    // the whole chain.
    let key = SigningKey::from_bytes([5u8; 32]);
    let now = 1_700_000_000;
    let sealed = proof(&[FactorKind::Password]).seal(&key, now, 300);

    let recovered = Verified::unseal(&key, now + 299, &sealed).expect("inside its life");
    assert_eq!(recovered.account(), &whom());
    assert_eq!(recovered.kinds(), FactorSet::of([FactorKind::Password]));

    // Expired.
    assert!(Verified::unseal(&key, now + 300, &sealed).is_none());
    // Forged: the right payload under somebody else's key.
    let other = SigningKey::from_bytes([6u8; 32]);
    assert!(Verified::unseal(&other, now, &sealed).is_none());
    // Tampered: a payload that is not what was signed.
    let (_, tag) = sealed.rsplit_once('.').unwrap();
    assert!(Verified::unseal(&key, now, &format!("AAAA.{tag}")).is_none());
    for malformed in ["", "no-separator", ".", "a."] {
        assert!(
            Verified::unseal(&key, now, malformed).is_none(),
            "`{malformed}`"
        );
    }
}

// ---------------------------------------------------------------------------
// Enrolment
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unconfirmed_enrolment_is_not_a_factor() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let who = AccountHolder::without_a_session(account.clone());

    mfa.begin_totp_enrolment(&who, "Brocanth", "ada@example.com")
        .await
        .unwrap();
    // A user who scanned the code and closed the tab has locked nothing.
    assert_eq!(
        mfa.second_factors(&common::password_proof(&account))
            .await
            .unwrap()
            .factors(),
        FactorSet::empty()
    );
}

#[tokio::test]
async fn a_wrong_code_does_not_confirm_an_enrolment() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let who = AccountHolder::without_a_session(account.clone());

    mfa.begin_totp_enrolment(&who, "Brocanth", "ada@example.com")
        .await
        .unwrap();
    let err = mfa
        .confirm_totp_enrolment(&who, "000000")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
    assert_eq!(
        mfa.second_factors(&common::password_proof(&account))
            .await
            .unwrap()
            .factors(),
        FactorSet::empty()
    );
}

#[tokio::test]
async fn confirming_an_enrolment_mints_the_recovery_codes_once() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let (_, codes) = enrol(&f, &account).await;

    let who = AccountHolder::without_a_session(account.clone());
    assert_eq!(codes.codes().len(), 10);
    assert_eq!(
        mfa.second_factors(&common::password_proof(&account))
            .await
            .unwrap()
            .factors(),
        FactorSet::of([FactorKind::Totp, FactorKind::RecoveryCode])
    );
    assert_eq!(
        mfa.enrolled(&who).await.unwrap(),
        FactorSet::of([
            FactorKind::Password,
            FactorKind::Totp,
            FactorKind::RecoveryCode
        ])
    );
    // There is no call that reads them back, because there is no copy to read.
    assert_eq!(mfa.recovery_codes_remaining(&who).await.unwrap(), 10);
}

// ---------------------------------------------------------------------------
// Recovery codes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_recovery_code_works_exactly_once() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let (_, codes) = enrol(&f, &account).await;
    let code = codes.codes()[0].as_str().to_string();

    let accepted = mfa.verify_recovery_code(&account, &code).await.unwrap();
    assert_eq!(accepted.remaining, 9, "nine should be left");
    assert_eq!(
        accepted.proof.kinds(),
        FactorSet::of([FactorKind::RecoveryCode])
    );
    let err = mfa.verify_recovery_code(&account, &code).await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
    assert_eq!(
        mfa.recovery_codes_remaining(&AccountHolder::without_a_session(account.clone()))
            .await
            .unwrap(),
        9
    );

    // The other nine are untouched.
    assert_eq!(
        mfa.verify_recovery_code(&account, codes.codes()[1].as_str())
            .await
            .unwrap()
            .remaining,
        8
    );
}

#[tokio::test]
async fn a_recovery_code_is_accepted_however_a_human_typed_it() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let (_, codes) = enrol(&f, &account).await;

    let typed = codes.codes()[0].as_str().to_uppercase().replace('-', " ");
    assert_eq!(
        mfa.verify_recovery_code(&account, &typed)
            .await
            .unwrap()
            .remaining,
        9
    );
}

#[tokio::test]
async fn regenerating_kills_the_whole_previous_set() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let who = AccountHolder::without_a_session(account.clone());
    let (_, old) = enrol(&f, &account).await;

    let new = mfa.regenerate_recovery_codes(&who).await.unwrap();
    assert_eq!(mfa.recovery_codes_remaining(&who).await.unwrap(), 10);

    for code in old.codes() {
        assert!(
            mfa.verify_recovery_code(&account, code.as_str())
                .await
                .is_err(),
            "a code from the list the user replaced still works"
        );
    }
    assert!(mfa
        .verify_recovery_code(&account, new.codes()[0].as_str())
        .await
        .is_ok());
}

#[tokio::test]
async fn disabling_the_second_factor_takes_the_recovery_codes_with_it() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let who = AccountHolder::without_a_session(account.clone());
    let (enrolment, codes) = enrol(&f, &account).await;

    mfa.disable_totp(&who).await.unwrap();
    assert_eq!(
        mfa.second_factors(&common::password_proof(&account))
            .await
            .unwrap()
            .factors(),
        FactorSet::empty()
    );

    // Leaving the codes behind would be a second password, on paper, that the
    // user believes they have deleted.
    assert!(mfa
        .verify_recovery_code(&account, codes.codes()[0].as_str())
        .await
        .is_err());
    f.clock.advance(60);
    let code = mfa.totp().code_at(&enrolment.secret, f.clock.now());
    assert!(mfa.verify_totp(&account, &code).await.is_err());
}

#[tokio::test]
async fn an_account_with_no_totp_answers_the_same_error_as_a_wrong_code() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let err = mfa.verify_totp(&account, "123456").await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
}
