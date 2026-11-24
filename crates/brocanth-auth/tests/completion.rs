//! What the `auth-otp`, `auth-mfa` and `impersonate` modules need from the
//! crate: a mailed code as a primary factor when the policy says so, a
//! holder read from a sign-in that must enrol, and `Otp` and `LoginAlerts`
//! built from the services.

mod common;

use std::sync::Arc;

use brocanth_auth::http_client::StubHttp;
use brocanth_auth::mfa::{Admission, Enrolled, FactorKind, FactorPolicy, FactorSet, Verified};
use brocanth_auth::{
    AccountHolder, AccountId, Auth, AuthConfig, LoginAlert, MemoryStore, OtpDelivery, PasswordCost,
    Passwords, SessionConfig, SigningKey, Staff, TestClock,
};

fn mailed(account: &str) -> Verified {
    Verified::without_a_credential_check(
        AccountId::new(account),
        FactorSet::of([FactorKind::EmailOtp]),
    )
}

fn enrolled(account: &str, kinds: &[FactorKind]) -> Enrolled {
    Enrolled::without_asking_the_store(AccountId::new(account), FactorSet::of(kinds.to_vec()))
}

#[test]
fn a_mailed_code_alone_opens_nothing_by_default() {
    let answer = FactorPolicy::recommended().admit(&enrolled("ada", &[]), mailed("ada"));
    assert_eq!(answer, Admission::Insufficient);
}

#[test]
fn a_mailed_code_is_the_primary_factor_when_the_policy_says_so() {
    let policy = FactorPolicy::recommended().accepting_mailbox_as_primary();
    assert!(policy.mailbox_is_primary());
    let Admission::Admit(admitted) = policy.admit(&enrolled("ada", &[]), mailed("ada")) else {
        panic!("an account with no second factor signs in with the code");
    };
    assert_eq!(admitted.factors(), FactorSet::of([FactorKind::EmailOtp]));
}

#[test]
fn a_mailed_code_is_never_also_the_second_factor() {
    let policy = FactorPolicy::recommended().accepting_mailbox_as_primary();
    let answer = policy.admit(
        &enrolled("ada", &[FactorKind::Totp, FactorKind::RecoveryCode]),
        mailed("ada"),
    );
    let Admission::Incomplete { candidates, proof } = answer else {
        panic!("an account with TOTP still owes its code: {answer:?}");
    };
    assert!(candidates.contains(FactorKind::Totp));
    assert!(!candidates.contains(FactorKind::EmailOtp));

    // The code and then TOTP: admitted.
    let totp = Verified::without_a_credential_check(
        AccountId::new("ada"),
        FactorSet::of([FactorKind::Totp]),
    );
    let both = proof.and(totp).expect("one account");
    assert!(matches!(
        policy.admit(&enrolled("ada", &[FactorKind::Totp]), both),
        Admission::Admit(_)
    ));
}

#[test]
fn a_policy_that_always_wants_a_second_factor_sends_a_mailed_sign_in_to_enrol() {
    let policy = FactorPolicy::recommended()
        .accepting_mailbox_as_primary()
        .requiring_a_second_factor_always();
    assert!(matches!(
        policy.admit(&enrolled("ada", &[]), mailed("ada")),
        Admission::EnrolmentRequired { .. }
    ));
}

#[test]
fn a_holder_read_from_a_proof_is_fresh_and_names_the_account() {
    let proof = common::password_proof(&AccountId::new("ada"));
    let holder = AccountHolder::enrolling(&proof).expect("a proof with a factor");
    assert_eq!(holder.account().as_str(), "ada");
    assert!(holder.elevation_is_fresh(1_000_000, 60));

    let nothing = Verified::without_a_credential_check(AccountId::new("ada"), FactorSet::empty());
    assert!(AccountHolder::enrolling(&nothing).is_err());
}

#[test]
fn impersonation_is_edit_on_the_impersonate_resource() {
    assert_eq!(Staff::IMPERSONATE, "impersonate.edit");
}

fn services() -> Auth<MemoryStore, StubHttp, TestClock> {
    Auth::build(AuthConfig {
        store: Arc::new(MemoryStore::new()),
        http: Arc::new(StubHttp::new()),
        clock: Arc::new(TestClock::default()),
        signing_key: SigningKey::from_bytes([5; 32]),
        session: SessionConfig::default(),
        passwords: Arc::new(Passwords::new(PasswordCost::insecure_for_tests()).unwrap()),
        providers: Vec::new(),
    })
    .unwrap()
}

#[tokio::test]
async fn the_services_build_one_time_codes_over_their_own_store() {
    let auth = services();
    auth.accounts()
        .register("ada@example.com", "a long enough password")
        .await
        .unwrap();
    let otp = auth.otp().expect("randomness");
    let requested = otp.request("ada@example.com").await.unwrap();
    let OtpDelivery::Send { code, .. } = requested.delivery else {
        panic!("the address has an account");
    };
    let proof = otp
        .authenticate(&requested.challenge, code.as_str())
        .await
        .expect("the code the mail carries");
    assert_eq!(proof.kinds(), FactorSet::of([FactorKind::EmailOtp]));
}

#[tokio::test]
async fn the_services_build_login_alerts_keyed_with_the_session_key() {
    let auth = services();
    let alerts = auth.login_alerts();
    let account = AccountId::new("ada");
    let first = alerts.note(&account, Some("203.0.113.7")).await.unwrap();
    assert!(matches!(
        first,
        LoginAlert::Unfamiliar {
            first_ever: true,
            ..
        }
    ));
    let again = alerts.note(&account, Some("203.0.113.9")).await.unwrap();
    assert!(matches!(again, LoginAlert::Familiar { .. }));
    let elsewhere = alerts.note(&account, Some("198.51.100.1")).await.unwrap();
    assert!(matches!(
        elsewhere,
        LoginAlert::Unfamiliar {
            first_ever: false,
            ..
        }
    ));
}
