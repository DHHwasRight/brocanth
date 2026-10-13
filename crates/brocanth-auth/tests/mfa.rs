//! The factor set, the sufficiency rule, and recovery codes.

mod common;

use brocanth_auth::clock::Clock;
use brocanth_auth::error::AuthError;
use brocanth_auth::identity::{AccountId, EmailAddress};
use brocanth_auth::impersonation::AccountHolder;
use brocanth_auth::mfa::{
    FactorClass, FactorKind, FactorPolicy, FactorSet, Sufficiency, TotpEnrolment,
};
use brocanth_auth::recovery::RecoveryCodes;
use brocanth_auth::session::Privilege;
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

#[test]
fn a_password_alone_is_enough_for_an_account_that_enrolled_nothing() {
    let policy = FactorPolicy::recommended();
    assert_eq!(
        policy.for_sign_in(&FactorSet::empty(), &FactorSet::of([FactorKind::Password])),
        Sufficiency::Sufficient(Privilege::Authenticated)
    );
}

#[test]
fn a_password_alone_is_not_enough_once_a_second_factor_exists() {
    let policy = FactorPolicy::recommended();
    let enrolled = FactorSet::of([FactorKind::Totp, FactorKind::RecoveryCode]);

    match policy.for_sign_in(&enrolled, &FactorSet::of([FactorKind::Password])) {
        Sufficiency::Incomplete(candidates) => {
            assert!(candidates.contains(FactorKind::Totp));
            assert!(candidates.contains(FactorKind::RecoveryCode));
            assert!(!candidates.contains(FactorKind::Password));
        }
        other => panic!("a password alone was accepted: {other:?}"),
    }

    for second in [FactorKind::Totp, FactorKind::RecoveryCode] {
        assert_eq!(
            policy.for_sign_in(&enrolled, &FactorSet::of([FactorKind::Password, second])),
            Sufficiency::Sufficient(Privilege::Authenticated),
            "{second} did not complete the sign-in"
        );
    }
}

#[test]
fn a_second_factor_without_a_primary_one_is_not_a_sign_in() {
    let policy = FactorPolicy::recommended();
    assert_eq!(
        policy.for_sign_in(
            &FactorSet::of([FactorKind::Totp]),
            &FactorSet::of([FactorKind::Totp])
        ),
        Sufficiency::Insufficient
    );
}

#[test]
fn a_passkey_stands_alone() {
    let policy = FactorPolicy::recommended();
    assert_eq!(
        policy.for_sign_in(
            &FactorSet::of([FactorKind::Totp]),
            &FactorSet::of([FactorKind::Passkey])
        ),
        Sufficiency::Sufficient(Privilege::Authenticated)
    );
}

#[test]
fn demanding_a_second_factor_from_everybody_sends_the_unenrolled_to_enrolment() {
    let policy = FactorPolicy::recommended().requiring_a_second_factor_always();
    assert_eq!(
        policy.for_sign_in(&FactorSet::empty(), &FactorSet::of([FactorKind::Password])),
        Sufficiency::EnrolmentRequired
    );
}

#[test]
fn a_sign_in_never_grants_elevation_however_many_factors_it_had() {
    let policy = FactorPolicy::recommended();
    let everything = FactorSet::of([FactorKind::Password, FactorKind::Totp]);
    assert_eq!(
        policy.for_sign_in(&FactorSet::of([FactorKind::Totp]), &everything),
        Sufficiency::Sufficient(Privilege::Authenticated)
    );
}

#[test]
fn elevation_needs_a_fresh_second_factor_and_a_recovery_code_is_not_one() {
    let policy = FactorPolicy::recommended();
    let enrolled = FactorSet::of([FactorKind::Totp, FactorKind::RecoveryCode]);

    assert!(policy.for_elevation(&enrolled, &FactorSet::of([FactorKind::Totp])));
    assert!(policy.for_elevation(&enrolled, &FactorSet::of([FactorKind::Passkey])));
    // The paper in the drawer does not open the dangerous door.
    assert!(!policy.for_elevation(&enrolled, &FactorSet::of([FactorKind::RecoveryCode])));
    assert!(!policy.for_elevation(&enrolled, &FactorSet::of([FactorKind::Password])));

    // Unless the application said so, in writing.
    let lenient = FactorPolicy::recommended().allowing_recovery_codes_to_elevate();
    assert!(lenient.for_elevation(&enrolled, &FactorSet::of([FactorKind::RecoveryCode])));
}

#[test]
fn an_account_with_only_a_password_can_still_elevate_with_it() {
    // Otherwise nobody without a phone could ever change their own password.
    let policy = FactorPolicy::recommended();
    assert!(policy.for_elevation(&FactorSet::empty(), &FactorSet::of([FactorKind::Password])));
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
        mfa.second_factors(&account).await.unwrap(),
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
        mfa.second_factors(&account).await.unwrap(),
        FactorSet::empty()
    );
}

#[tokio::test]
async fn confirming_an_enrolment_mints_the_recovery_codes_once() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let (_, codes) = enrol(&f, &account).await;

    assert_eq!(codes.codes().len(), 10);
    assert_eq!(
        mfa.second_factors(&account).await.unwrap(),
        FactorSet::of([FactorKind::Totp, FactorKind::RecoveryCode])
    );
    assert_eq!(
        mfa.enrolled(&account).await.unwrap(),
        FactorSet::of([
            FactorKind::Password,
            FactorKind::Totp,
            FactorKind::RecoveryCode
        ])
    );
    // There is no call that reads them back, because there is no copy to read.
    assert_eq!(mfa.recovery_codes_remaining(&account).await.unwrap(), 10);
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

    assert_eq!(
        mfa.verify_recovery_code(&account, &code).await.unwrap(),
        9,
        "nine should be left"
    );
    let err = mfa.verify_recovery_code(&account, &code).await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
    assert_eq!(mfa.recovery_codes_remaining(&account).await.unwrap(), 9);

    // The other nine are untouched.
    assert_eq!(
        mfa.verify_recovery_code(&account, codes.codes()[1].as_str())
            .await
            .unwrap(),
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
    assert_eq!(mfa.verify_recovery_code(&account, &typed).await.unwrap(), 9);
}

#[tokio::test]
async fn regenerating_kills_the_whole_previous_set() {
    let f = Fixture::new();
    let mfa = f.mfa();
    let account = an_account(&f).await;
    let who = AccountHolder::without_a_session(account.clone());
    let (_, old) = enrol(&f, &account).await;

    let new = mfa.regenerate_recovery_codes(&who).await.unwrap();
    assert_eq!(mfa.recovery_codes_remaining(&account).await.unwrap(), 10);

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
        mfa.second_factors(&account).await.unwrap(),
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
