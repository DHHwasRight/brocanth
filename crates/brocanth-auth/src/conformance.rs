//! The storage contract as an executable suite.
//!
//! [`crate::storage`] states what a store must do. This module checks it.
//! [`MemoryStore`] and `PgStore` both run it, and an adapter of your own can
//! run it too: a store that passes here behaves as the services expect.
//!
//! [`MemoryStore`]: crate::memory::MemoryStore
//!
//! # How to run it
//!
//! Write an async function that answers a fresh, empty store, or `None` to
//! skip. Then call one macro per trait the store implements:
//!
//! ```ignore
//! use std::sync::Arc;
//! use brocanth_auth::MemoryStore;
//!
//! async fn fresh() -> Option<Arc<MemoryStore>> {
//!     Some(Arc::new(MemoryStore::new()))
//! }
//!
//! brocanth_auth::auth_store_conformance!(fresh);
//! brocanth_auth::account_directory_conformance!(fresh);
//! brocanth_auth::mfa_store_conformance!(fresh);
//! brocanth_auth::otp_store_conformance!(fresh);
//! brocanth_auth::sign_in_location_store_conformance!(fresh);
//! ```
//!
//! Each macro writes a module of `#[tokio::test]` functions, one per case, so
//! a failure names the case. The test binary needs `tokio` with the `macros`
//! and `rt-multi-thread` features.
//!
//! `fresh` can also answer any [`Fixture`]: a value that holds the store and
//! cleans up after the case. The Postgres tests use one to drop the schema
//! each case ran in. The cleanup runs when the case panics too.
//!
//! [`Fixture`]: crate::conformance::Fixture
//!
//! # Concurrency cases
//!
//! The five atomic operations in [`crate::storage`] each have a case that
//! starts [`RACERS`] callers at once on one row and counts the winners. A
//! store that reads and then writes in two steps fails these. The cases need
//! a multi-threaded runtime, and the macros ask for one.
//!
//! [`RACERS`]: crate::conformance::RACERS

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;

use crate::clock::UnixTime;
use crate::identity::{AccountId, EmailAddress};
use crate::impersonation::Impersonation;
use crate::mfa::{FactorKind, FactorSet};
use crate::otp::OtpChallengeId;
use crate::session::{Privilege, SessionId};
use crate::storage::{
    AccountDirectory, AuthStore, MfaStore, NewAccount, NewProviderLink, OtpChallengeRecord,
    OtpStore, SessionRecord, SignInLocationStore, StoreError, TokenRecord, TotpSecretRecord,
};
use crate::tokens::TokenPurpose;

/// How many callers each concurrency case starts at once.
pub const RACERS: usize = 16;

/// A fixed point in time, so a failure prints a number you recognise.
const T0: UnixTime = 1_700_000_000;

/// A store for one case, and what to do after it.
///
/// `Arc<S>` is a fixture whose cleanup does nothing, which is all
/// [`MemoryStore`](crate::memory::MemoryStore) needs.
#[async_trait]
pub trait Fixture: Send + Sync + 'static {
    /// The store under test.
    type Store: Send + Sync + 'static;

    /// The store, for the case to use.
    fn store(&self) -> Arc<Self::Store>;

    /// Runs after the case, whether it passed or panicked.
    async fn finish(self);
}

#[async_trait]
impl<S: Send + Sync + 'static> Fixture for Arc<S> {
    type Store = S;

    fn store(&self) -> Arc<S> {
        self.clone()
    }

    async fn finish(self) {}
}

/// Runs one case on a fresh fixture. The macros call this; you do not need to.
///
/// The case runs on its own task, so a panic in it is caught, the fixture
/// still cleans up, and then the panic carries on.
#[doc(hidden)]
pub async fn run<F, C, Fut>(fixture: Option<F>, name: &str, case: C)
where
    F: Fixture,
    C: FnOnce(Arc<F::Store>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    let Some(fixture) = fixture else {
        use std::io::Write as _;
        // Straight to stderr: libtest hides `eprintln!` output from a test
        // that passes, and a skip must be seen.
        let _ = writeln!(std::io::stderr(), "*** SKIPPED (no store): {name}");
        return;
    };
    let outcome = tokio::spawn(case(fixture.store())).await;
    fixture.finish().await;
    if let Err(e) = outcome {
        match e.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            Err(e) => panic!("{name}: the case did not finish: {e}"),
        }
    }
}

/// Writes one `#[tokio::test]` per case. Used by the public macros.
#[doc(hidden)]
#[macro_export]
macro_rules! __auth_conformance_cases {
    ($fresh:ident, $group:ident, [$($case:ident),* $(,)?]) => {
        mod $group {
            $(
                #[::tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $case() {
                    $crate::conformance::run(
                        super::$fresh().await,
                        concat!(stringify!($group), "::", stringify!($case)),
                        $crate::conformance::$group::$case,
                    )
                    .await;
                }
            )*
        }
    };
}

/// Every [`AuthStore`] case, as tests. See the [module](crate::conformance).
#[macro_export]
macro_rules! auth_store_conformance {
    ($fresh:ident) => {
        $crate::__auth_conformance_cases!(
            $fresh,
            auth_store,
            [
                an_account_round_trips,
                the_email_is_unique,
                accounts_without_an_email_do_not_collide,
                one_of_many_concurrent_registrations_wins,
                the_password_and_the_verified_flag_change,
                changing_an_unknown_account_fails,
                a_provider_link_round_trips,
                a_subject_belongs_to_one_account,
                concurrent_links_of_one_subject_have_one_owner,
                unlinking_removes_only_that_provider,
                a_session_round_trips_with_factors_and_impersonation,
                a_session_is_touched_and_deleted,
                signing_out_everywhere_spares_other_accounts,
                a_token_is_spent_once,
                a_token_is_bound_to_its_purpose,
                an_expired_token_is_still_handed_back_once,
                concurrent_spends_of_one_token_win_once,
                deleting_tokens_is_per_account_and_purpose,
            ]
        );
    };
}

/// Every [`AccountDirectory`] case, as tests.
#[macro_export]
macro_rules! account_directory_conformance {
    ($fresh:ident) => {
        $crate::__auth_conformance_cases!(
            $fresh,
            account_directory,
            [
                an_empty_directory_is_empty,
                accounts_page_in_one_total_order,
                sessions_are_listed_newest_first,
            ]
        );
    };
}

/// Every [`MfaStore`] case, as tests.
#[macro_export]
macro_rules! mfa_store_conformance {
    ($fresh:ident) => {
        $crate::__auth_conformance_cases!(
            $fresh,
            mfa_store,
            [
                a_totp_secret_round_trips_and_is_replaced,
                confirming_without_a_secret_does_nothing,
                totp_steps_only_move_forward,
                concurrent_claims_of_one_step_win_once,
                a_recovery_code_is_spent_once,
                replacing_recovery_codes_with_nothing_removes_them,
                concurrent_spends_of_one_recovery_code_win_once,
            ]
        );
    };
}

/// Every [`OtpStore`] case, as tests.
#[macro_export]
macro_rules! otp_store_conformance {
    ($fresh:ident) => {
        $crate::__auth_conformance_cases!(
            $fresh,
            otp_store,
            [
                a_claim_counts_the_attempt,
                the_last_attempt_deletes_the_challenge,
                an_expired_challenge_is_deleted_by_its_claim,
                an_unknown_challenge_is_none,
                purging_removes_only_expired_challenges,
                concurrent_claims_never_pass_the_cap,
            ]
        );
    };
}

/// Every [`SignInLocationStore`] case, as tests.
#[macro_export]
macro_rules! sign_in_location_store_conformance {
    ($fresh:ident) => {
        $crate::__auth_conformance_cases!(
            $fresh,
            sign_in_location_store,
            [
                a_network_is_new_once,
                purging_forgets_unused_networks,
                concurrent_first_sign_ins_report_new_once,
            ]
        );
    };
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn email(raw: &str) -> EmailAddress {
    EmailAddress::parse(raw).expect("a valid test address")
}

async fn account<S: AuthStore>(store: &S, address: Option<&str>) -> AccountId {
    store
        .create_account(NewAccount {
            email: address.map(email),
            email_verified: false,
            password_hash: None,
        })
        .await
        .expect("create_account")
        .id
}

fn session(id: &str, account: &AccountId, created_at: UnixTime) -> SessionRecord {
    SessionRecord {
        id: SessionId::new(id),
        account: account.clone(),
        privilege: Privilege::Authenticated,
        created_at,
        last_seen_at: created_at,
        absolute_expires_at: created_at + 86_400,
        factors: FactorSet::of([FactorKind::Password]),
        impersonation: None,
    }
}

fn token(purpose: TokenPurpose, selector: &str, account: &AccountId) -> TokenRecord {
    TokenRecord {
        purpose,
        selector: selector.to_string(),
        verifier_hash: [7u8; 32],
        account: account.clone(),
        expires_at: T0 + 3_600,
    }
}

fn challenge(id: &str, account: &AccountId, max_attempts: u32) -> OtpChallengeRecord {
    OtpChallengeRecord {
        id: OtpChallengeId::new(id),
        account: account.clone(),
        code_hash: "$argon2id$v=19$m=8,t=1,p=1$c2FsdHNhbHQ$aGFzaGhhc2g".to_string(),
        expires_at: T0 + 600,
        attempts: 0,
        max_attempts,
    }
}

/// Starts [`RACERS`] copies of `call` at once, behind a barrier, and answers
/// every result.
async fn race<S, F, Fut, T>(store: &Arc<S>, call: F) -> Vec<T>
where
    S: Send + Sync + 'static,
    F: Fn(Arc<S>, usize) -> Fut,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let barrier = Arc::new(tokio::sync::Barrier::new(RACERS));
    let mut handles = Vec::with_capacity(RACERS);
    for i in 0..RACERS {
        let barrier = barrier.clone();
        let racer = call(store.clone(), i);
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            racer.await
        }));
    }
    let mut out = Vec::with_capacity(RACERS);
    for handle in handles {
        out.push(handle.await.expect("a racer panicked"));
    }
    out
}

// ---------------------------------------------------------------------------
// AuthStore
// ---------------------------------------------------------------------------

/// The [`AuthStore`] cases.
pub mod auth_store {
    use super::*;

    /// An account comes back by id and by address, with every field.
    pub async fn an_account_round_trips<S: AuthStore>(store: Arc<S>) {
        let created = store
            .create_account(NewAccount {
                email: Some(email("ada@example.com")),
                email_verified: true,
                password_hash: Some("$argon2id$stub".into()),
            })
            .await
            .expect("create_account");
        assert_eq!(created.email, Some(email("ada@example.com")));
        assert!(created.email_verified);
        assert_eq!(created.password_hash.as_deref(), Some("$argon2id$stub"));

        let by_id = store.account(&created.id).await.unwrap();
        assert_eq!(by_id.as_ref(), Some(&created), "by id");
        let by_email = store
            .account_by_email(&email("ada@example.com"))
            .await
            .unwrap();
        assert_eq!(by_email.as_ref(), Some(&created), "by email");

        assert_eq!(
            store.account(&AccountId::new("nobody")).await.unwrap(),
            None
        );
        assert_eq!(
            store
                .account_by_email(&email("nobody@example.com"))
                .await
                .unwrap(),
            None
        );
    }

    /// A second account with the same address is [`StoreError::EmailTaken`].
    pub async fn the_email_is_unique<S: AuthStore>(store: Arc<S>) {
        account(&*store, Some("ada@example.com")).await;
        // Parsed from a different spelling: normalisation is the boundary's
        // job, and the store must still see one address.
        let again = store
            .create_account(NewAccount {
                email: Some(email("  ADA@Example.com ")),
                ..NewAccount::default()
            })
            .await;
        assert!(
            matches!(again, Err(StoreError::EmailTaken)),
            "expected EmailTaken, got {again:?}"
        );
    }

    /// The uniqueness is on addresses, not on their absence.
    pub async fn accounts_without_an_email_do_not_collide<S: AuthStore>(store: Arc<S>) {
        let a = account(&*store, None).await;
        let b = account(&*store, None).await;
        assert_ne!(a, b);
    }

    /// Many registrations of one address at once: exactly one wins, and every
    /// other one is told [`StoreError::EmailTaken`].
    pub async fn one_of_many_concurrent_registrations_wins<S: AuthStore>(store: Arc<S>) {
        let results = race(&store, |store, _| async move {
            store
                .create_account(NewAccount {
                    email: Some(email("race@example.com")),
                    ..NewAccount::default()
                })
                .await
        })
        .await;
        let won = results.iter().filter(|r| r.is_ok()).count();
        let taken = results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::EmailTaken)))
            .count();
        assert_eq!(won, 1, "exactly one registration wins: {results:?}");
        assert_eq!(taken, RACERS - 1, "the rest are EmailTaken: {results:?}");
    }

    /// The two setters change their field and nothing else.
    pub async fn the_password_and_the_verified_flag_change<S: AuthStore>(store: Arc<S>) {
        let id = account(&*store, Some("ada@example.com")).await;
        store.set_password_hash(&id, Some("$new")).await.unwrap();
        store.set_email_verified(&id, true).await.unwrap();
        let read = store.account(&id).await.unwrap().unwrap();
        assert_eq!(read.password_hash.as_deref(), Some("$new"));
        assert!(read.email_verified);
        assert_eq!(read.email, Some(email("ada@example.com")));

        store.set_password_hash(&id, None).await.unwrap();
        store.set_email_verified(&id, false).await.unwrap();
        let read = store.account(&id).await.unwrap().unwrap();
        assert_eq!(read.password_hash, None, "None removes the password");
        assert!(!read.email_verified);
    }

    /// A setter on an account that does not exist is an error, not a silent
    /// success.
    pub async fn changing_an_unknown_account_fails<S: AuthStore>(store: Arc<S>) {
        let ghost = AccountId::new("ghost");
        assert!(store.set_password_hash(&ghost, Some("$x")).await.is_err());
        assert!(store.set_email_verified(&ghost, true).await.is_err());
    }

    /// A link comes back by subject and by account. Linking the same subject
    /// to the same account again refreshes it.
    pub async fn a_provider_link_round_trips<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, Some("ada@example.com")).await;
        let link = store
            .link_provider(
                &ada,
                NewProviderLink {
                    provider: "github".into(),
                    subject: "1".into(),
                    email: Some(email("ada@github.test")),
                },
            )
            .await
            .unwrap();
        assert_eq!(link.account, ada);
        assert_eq!(link.provider, "github");
        assert_eq!(link.subject, "1");
        assert_eq!(link.email, Some(email("ada@github.test")));
        assert_eq!(store.link("github", "1").await.unwrap(), Some(link.clone()));
        assert_eq!(store.link("github", "2").await.unwrap(), None);
        assert_eq!(store.link("google", "1").await.unwrap(), None);

        let again = store
            .link_provider(
                &ada,
                NewProviderLink {
                    provider: "github".into(),
                    subject: "1".into(),
                    email: None,
                },
            )
            .await
            .expect("relinking to the same account is allowed");
        assert_eq!(again.email, None);
        assert_eq!(store.links_for_account(&ada).await.unwrap(), vec![again]);
    }

    /// A subject already linked to one account cannot be linked to another.
    pub async fn a_subject_belongs_to_one_account<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, Some("ada@example.com")).await;
        let eve = account(&*store, Some("eve@example.com")).await;
        let new = || NewProviderLink {
            provider: "github".into(),
            subject: "1".into(),
            email: None,
        };
        store.link_provider(&ada, new()).await.unwrap();
        assert!(store.link_provider(&eve, new()).await.is_err());
        assert_eq!(
            store.link("github", "1").await.unwrap().unwrap().account,
            ada,
            "the refused link changed nothing"
        );
        assert!(store.links_for_account(&eve).await.unwrap().is_empty());
    }

    /// Many accounts link one subject at once: exactly one owns it.
    pub async fn concurrent_links_of_one_subject_have_one_owner<S: AuthStore>(store: Arc<S>) {
        let mut accounts = Vec::new();
        for _ in 0..RACERS {
            accounts.push(account(&*store, None).await);
        }
        let accounts = Arc::new(accounts);
        let results = race(&store, |store, i| {
            let accounts = accounts.clone();
            async move {
                store
                    .link_provider(
                        &accounts[i],
                        NewProviderLink {
                            provider: "github".into(),
                            subject: "contested".into(),
                            email: None,
                        },
                    )
                    .await
            }
        })
        .await;
        let winners: Vec<_> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
        assert_eq!(winners.len(), 1, "exactly one link wins: {results:?}");
        assert_eq!(
            store.link("github", "contested").await.unwrap().as_ref(),
            Some(winners[0])
        );
    }

    /// Unlinking removes that provider for that account, and nothing else.
    pub async fn unlinking_removes_only_that_provider<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let eve = account(&*store, None).await;
        for (who, provider, subject) in [
            (&ada, "github", "a1"),
            (&ada, "google", "a2"),
            (&eve, "github", "e1"),
        ] {
            store
                .link_provider(
                    who,
                    NewProviderLink {
                        provider: provider.into(),
                        subject: subject.into(),
                        email: None,
                    },
                )
                .await
                .unwrap();
        }
        store.unlink_provider(&ada, "github").await.unwrap();
        let left: Vec<_> = store
            .links_for_account(&ada)
            .await
            .unwrap()
            .into_iter()
            .map(|l| l.provider)
            .collect();
        assert_eq!(left, vec!["google".to_string()]);
        assert!(store.link("github", "e1").await.unwrap().is_some());
        // Unlinking what is not there is not an error.
        store.unlink_provider(&ada, "github").await.unwrap();
    }

    /// Every field of a session comes back, including the factor set and both
    /// identities of an impersonation.
    pub async fn a_session_round_trips_with_factors_and_impersonation<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let operator = account(&*store, None).await;

        let mut plain = session("plain", &ada, T0);
        plain.privilege = Privilege::Elevated;
        plain.factors = FactorSet::of([FactorKind::Password, FactorKind::Totp]);
        store.create_session(plain.clone()).await.unwrap();

        let mut anonymous = session("anonymous", &ada, T0);
        anonymous.privilege = Privilege::Anonymous;
        anonymous.factors = FactorSet::empty();
        store.create_session(anonymous.clone()).await.unwrap();

        let mut acted = session("acted", &ada, T0 + 5);
        acted.factors = FactorSet::empty();
        acted.impersonation = Some(Impersonation {
            operator: operator.clone(),
            operator_session: SessionId::new("operator-session"),
            started_at: T0 + 5,
            reason: "ticket #42: invoice will not render".into(),
        });
        store.create_session(acted.clone()).await.unwrap();

        assert_eq!(store.session(&plain.id).await.unwrap(), Some(plain));
        assert_eq!(store.session(&anonymous.id).await.unwrap(), Some(anonymous));
        let read = store.session(&acted.id).await.unwrap().expect("stored");
        assert!(read.is_impersonated());
        assert_eq!(read.operator(), Some(&operator));
        assert_eq!(read, acted);
        assert_eq!(store.session(&SessionId::new("nope")).await.unwrap(), None);
    }

    /// `touch_session` moves `last_seen_at` only; `delete_session` removes the
    /// row. Neither fails on an unknown id.
    pub async fn a_session_is_touched_and_deleted<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let s = session("s", &ada, T0);
        store.create_session(s.clone()).await.unwrap();

        store.touch_session(&s.id, T0 + 90).await.unwrap();
        let read = store.session(&s.id).await.unwrap().unwrap();
        assert_eq!(read.last_seen_at, T0 + 90);
        assert_eq!(read.created_at, s.created_at);
        assert_eq!(read.absolute_expires_at, s.absolute_expires_at);

        store.delete_session(&s.id).await.unwrap();
        assert_eq!(store.session(&s.id).await.unwrap(), None);

        store
            .touch_session(&SessionId::new("gone"), T0)
            .await
            .unwrap();
        store.delete_session(&SessionId::new("gone")).await.unwrap();
    }

    /// "Sign me out everywhere" deletes that account's sessions, answers how
    /// many, and leaves every other account's alone.
    pub async fn signing_out_everywhere_spares_other_accounts<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let eve = account(&*store, None).await;
        for id in ["a1", "a2", "a3"] {
            store.create_session(session(id, &ada, T0)).await.unwrap();
        }
        store.create_session(session("e1", &eve, T0)).await.unwrap();

        assert_eq!(store.delete_sessions_for_account(&ada).await.unwrap(), 3);
        assert_eq!(store.delete_sessions_for_account(&ada).await.unwrap(), 0);
        assert_eq!(store.session(&SessionId::new("a1")).await.unwrap(), None);
        assert!(store
            .session(&SessionId::new("e1"))
            .await
            .unwrap()
            .is_some());
    }

    /// `consume_token` hands the row back once, then never again.
    pub async fn a_token_is_spent_once<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let t = token(TokenPurpose::PasswordReset, "sel", &ada);
        store.store_token(t.clone()).await.unwrap();
        assert_eq!(
            store
                .consume_token(TokenPurpose::PasswordReset, "sel")
                .await
                .unwrap(),
            Some(t)
        );
        assert_eq!(
            store
                .consume_token(TokenPurpose::PasswordReset, "sel")
                .await
                .unwrap(),
            None
        );
    }

    /// The purpose is part of the key: a verification token cannot be spent
    /// as a reset token, and a failed try does not spend it.
    pub async fn a_token_is_bound_to_its_purpose<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let t = token(TokenPurpose::EmailVerification, "sel", &ada);
        store.store_token(t.clone()).await.unwrap();
        assert_eq!(
            store
                .consume_token(TokenPurpose::PasswordReset, "sel")
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .consume_token(TokenPurpose::EmailVerification, "sel")
                .await
                .unwrap(),
            Some(t)
        );
    }

    /// The store does not judge expiry: it deletes and returns, and the caller
    /// refuses. So an expired token is gone after one try, whatever the clock.
    pub async fn an_expired_token_is_still_handed_back_once<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let mut t = token(TokenPurpose::PasswordReset, "old", &ada);
        t.expires_at = 1;
        store.store_token(t.clone()).await.unwrap();
        assert_eq!(
            store
                .consume_token(TokenPurpose::PasswordReset, "old")
                .await
                .unwrap(),
            Some(t)
        );
        assert_eq!(
            store
                .consume_token(TokenPurpose::PasswordReset, "old")
                .await
                .unwrap(),
            None
        );
    }

    /// Many requests spend one token at once: exactly one gets it.
    pub async fn concurrent_spends_of_one_token_win_once<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        store
            .store_token(token(TokenPurpose::PasswordReset, "contested", &ada))
            .await
            .unwrap();
        let results = race(&store, |store, _| async move {
            store
                .consume_token(TokenPurpose::PasswordReset, "contested")
                .await
                .expect("consume_token")
        })
        .await;
        let won = results.iter().filter(|r| r.is_some()).count();
        assert_eq!(won, 1, "a single-use token was spent {won} times");
    }

    /// Deleting an account's tokens of one purpose leaves its other purpose
    /// and other accounts alone.
    pub async fn deleting_tokens_is_per_account_and_purpose<S: AuthStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let eve = account(&*store, None).await;
        store
            .store_token(token(TokenPurpose::PasswordReset, "ada-reset", &ada))
            .await
            .unwrap();
        store
            .store_token(token(TokenPurpose::EmailVerification, "ada-verify", &ada))
            .await
            .unwrap();
        store
            .store_token(token(TokenPurpose::PasswordReset, "eve-reset", &eve))
            .await
            .unwrap();

        store
            .delete_tokens_for_account(&ada, TokenPurpose::PasswordReset)
            .await
            .unwrap();

        let take = |purpose, selector: &'static str| {
            let store = store.clone();
            async move { store.consume_token(purpose, selector).await.unwrap() }
        };
        assert!(take(TokenPurpose::PasswordReset, "ada-reset")
            .await
            .is_none());
        assert!(take(TokenPurpose::EmailVerification, "ada-verify")
            .await
            .is_some());
        assert!(take(TokenPurpose::PasswordReset, "eve-reset")
            .await
            .is_some());
    }
}

// ---------------------------------------------------------------------------
// AccountDirectory
// ---------------------------------------------------------------------------

/// The [`AccountDirectory`] cases.
pub mod account_directory {
    use super::*;

    /// No accounts is zero accounts and an empty page, not an error.
    pub async fn an_empty_directory_is_empty<S: AccountDirectory>(store: Arc<S>) {
        assert_eq!(store.count_accounts().await.unwrap(), 0);
        assert!(store.accounts(10, 0).await.unwrap().is_empty());
        assert!(store
            .sessions_for_account(&AccountId::new("nobody"))
            .await
            .unwrap()
            .is_empty());
    }

    /// Newest first, ties by id, and pages that add up to the whole list with
    /// no row twice.
    pub async fn accounts_page_in_one_total_order<S: AccountDirectory>(store: Arc<S>) {
        for i in 0..7 {
            account(&*store, Some(&format!("user{i}@example.com"))).await;
        }
        assert_eq!(store.count_accounts().await.unwrap(), 7);

        let all = store.accounts(100, 0).await.unwrap();
        assert_eq!(all.len(), 7);
        for pair in all.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            assert!(
                a.created_at > b.created_at || (a.created_at == b.created_at && a.id < b.id),
                "out of order: {a:?} before {b:?}"
            );
        }

        let mut paged = Vec::new();
        for page in 0..3 {
            paged.extend(store.accounts(3, page * 3).await.unwrap());
        }
        assert_eq!(paged, all, "three pages of three are the whole list");
        let unique: HashSet<_> = paged.iter().map(|a| a.id.clone()).collect();
        assert_eq!(unique.len(), 7);

        assert!(store.accounts(0, 0).await.unwrap().is_empty(), "limit 0");
        assert!(
            store.accounts(10, 50).await.unwrap().is_empty(),
            "offset past the end"
        );
    }

    /// One account's sessions, newest first, and nobody else's.
    pub async fn sessions_are_listed_newest_first<S: AccountDirectory>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let eve = account(&*store, None).await;
        store
            .create_session(session("old", &ada, T0))
            .await
            .unwrap();
        store
            .create_session(session("new", &ada, T0 + 60))
            .await
            .unwrap();
        store
            .create_session(session("mid-b", &ada, T0 + 30))
            .await
            .unwrap();
        store
            .create_session(session("mid-a", &ada, T0 + 30))
            .await
            .unwrap();
        store
            .create_session(session("eve", &eve, T0 + 90))
            .await
            .unwrap();

        let ids: Vec<String> = store
            .sessions_for_account(&ada)
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.id.as_str().to_string())
            .collect();
        assert_eq!(ids, ["new", "mid-a", "mid-b", "old"]);
    }
}

// ---------------------------------------------------------------------------
// MfaStore
// ---------------------------------------------------------------------------

/// The [`MfaStore`] cases.
pub mod mfa_store {
    use super::*;

    fn secret(byte: u8) -> TotpSecretRecord {
        TotpSecretRecord {
            secret: vec![byte; 20],
            created_at: T0,
            confirmed_at: None,
            last_step: 0,
        }
    }

    /// A secret comes back as stored; a second put replaces it; confirm sets
    /// the time; delete removes it.
    pub async fn a_totp_secret_round_trips_and_is_replaced<S: MfaStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        assert_eq!(store.totp_secret(&ada).await.unwrap(), None);

        store.put_totp_secret(&ada, secret(1)).await.unwrap();
        assert_eq!(store.totp_secret(&ada).await.unwrap(), Some(secret(1)));

        store.put_totp_secret(&ada, secret(2)).await.unwrap();
        assert_eq!(store.totp_secret(&ada).await.unwrap(), Some(secret(2)));

        store.confirm_totp_secret(&ada, T0 + 10).await.unwrap();
        let read = store.totp_secret(&ada).await.unwrap().unwrap();
        assert_eq!(read.confirmed_at, Some(T0 + 10));
        assert_eq!(read.secret, vec![2; 20]);

        store.delete_totp_secret(&ada).await.unwrap();
        assert_eq!(store.totp_secret(&ada).await.unwrap(), None);
        store.delete_totp_secret(&ada).await.unwrap();
    }

    /// Confirming an account with no secret creates nothing.
    pub async fn confirming_without_a_secret_does_nothing<S: MfaStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        store.confirm_totp_secret(&ada, T0).await.unwrap();
        assert_eq!(store.totp_secret(&ada).await.unwrap(), None);
        assert!(!store.claim_totp_step(&ada, 1).await.unwrap());
    }

    /// A step is won only when it is greater than every step before it.
    pub async fn totp_steps_only_move_forward<S: MfaStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        store.put_totp_secret(&ada, secret(1)).await.unwrap();
        assert!(store.claim_totp_step(&ada, 100).await.unwrap());
        assert!(!store.claim_totp_step(&ada, 100).await.unwrap(), "replay");
        assert!(!store.claim_totp_step(&ada, 99).await.unwrap(), "older");
        assert!(store.claim_totp_step(&ada, 101).await.unwrap());
        assert_eq!(
            store.totp_secret(&ada).await.unwrap().unwrap().last_step,
            101
        );
    }

    /// Many requests carrying one correct code at once: exactly one wins the
    /// step. This is the TOTP replay defence.
    pub async fn concurrent_claims_of_one_step_win_once<S: MfaStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        store.put_totp_secret(&ada, secret(1)).await.unwrap();
        let results = race(&store, |store, _| {
            let ada = ada.clone();
            async move { store.claim_totp_step(&ada, 500).await.expect("claim") }
        })
        .await;
        let won = results.iter().filter(|w| **w).count();
        assert_eq!(won, 1, "one TOTP step was accepted {won} times");
    }

    /// A recovery code is spent once, and only from its own account.
    pub async fn a_recovery_code_is_spent_once<S: MfaStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let eve = account(&*store, None).await;
        store
            .replace_recovery_codes(&ada, vec![[1; 32], [2; 32], [3; 32]])
            .await
            .unwrap();
        store
            .replace_recovery_codes(&eve, vec![[1; 32]])
            .await
            .unwrap();
        assert_eq!(store.count_recovery_codes(&ada).await.unwrap(), 3);

        assert!(store.consume_recovery_code(&ada, &[2; 32]).await.unwrap());
        assert!(!store.consume_recovery_code(&ada, &[2; 32]).await.unwrap());
        assert!(!store.consume_recovery_code(&ada, &[9; 32]).await.unwrap());
        assert_eq!(store.count_recovery_codes(&ada).await.unwrap(), 2);
        assert_eq!(
            store.count_recovery_codes(&eve).await.unwrap(),
            1,
            "another account's identical code is untouched"
        );

        store
            .replace_recovery_codes(&ada, vec![[7; 32]])
            .await
            .unwrap();
        assert_eq!(store.count_recovery_codes(&ada).await.unwrap(), 1);
        assert!(
            !store.consume_recovery_code(&ada, &[1; 32]).await.unwrap(),
            "replacing invalidates the old set"
        );
    }

    /// An empty list removes every code.
    pub async fn replacing_recovery_codes_with_nothing_removes_them<S: MfaStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        store
            .replace_recovery_codes(&ada, vec![[1; 32], [2; 32]])
            .await
            .unwrap();
        store
            .replace_recovery_codes(&ada, Vec::new())
            .await
            .unwrap();
        assert_eq!(store.count_recovery_codes(&ada).await.unwrap(), 0);
        assert!(!store.consume_recovery_code(&ada, &[1; 32]).await.unwrap());
    }

    /// Many requests spend one recovery code at once: exactly one gets it.
    pub async fn concurrent_spends_of_one_recovery_code_win_once<S: MfaStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        store
            .replace_recovery_codes(&ada, vec![[4; 32]])
            .await
            .unwrap();
        let results = race(&store, |store, _| {
            let ada = ada.clone();
            async move {
                store
                    .consume_recovery_code(&ada, &[4; 32])
                    .await
                    .expect("consume")
            }
        })
        .await;
        let won = results.iter().filter(|w| **w).count();
        assert_eq!(won, 1, "one recovery code was spent {won} times");
    }
}

// ---------------------------------------------------------------------------
// OtpStore
// ---------------------------------------------------------------------------

/// The [`OtpStore`] cases.
pub mod otp_store {
    use super::*;

    /// A claim answers the row with the attempt already counted, and the row
    /// stays while attempts remain.
    pub async fn a_claim_counts_the_attempt<S: OtpStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let c = challenge("c", &ada, 3);
        store.create_otp_challenge(c.clone()).await.unwrap();

        let first = store.claim_otp_attempt(&c.id, T0).await.unwrap().unwrap();
        assert_eq!(first.attempts, 1);
        assert_eq!(first.account, ada);
        assert_eq!(first.code_hash, c.code_hash);
        assert_eq!(first.max_attempts, 3);
        let second = store.claim_otp_attempt(&c.id, T0).await.unwrap().unwrap();
        assert_eq!(second.attempts, 2);

        store.delete_otp_challenge(&c.id).await.unwrap();
        assert_eq!(store.claim_otp_attempt(&c.id, T0).await.unwrap(), None);
    }

    /// The claim that uses the last attempt deletes the row and still answers
    /// it.
    pub async fn the_last_attempt_deletes_the_challenge<S: OtpStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let c = challenge("c", &ada, 2);
        store.create_otp_challenge(c.clone()).await.unwrap();
        assert_eq!(
            store
                .claim_otp_attempt(&c.id, T0)
                .await
                .unwrap()
                .unwrap()
                .attempts,
            1
        );
        let last = store.claim_otp_attempt(&c.id, T0).await.unwrap().unwrap();
        assert_eq!(last.attempts, 2);
        assert_eq!(store.claim_otp_attempt(&c.id, T0).await.unwrap(), None);
    }

    /// A claim on an expired challenge deletes it and still answers it, so the
    /// caller does the same work either way.
    pub async fn an_expired_challenge_is_deleted_by_its_claim<S: OtpStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let c = challenge("c", &ada, 5);
        store.create_otp_challenge(c.clone()).await.unwrap();
        let late = store
            .claim_otp_attempt(&c.id, c.expires_at)
            .await
            .unwrap()
            .expect("still answered");
        assert_eq!(late.attempts, 1);
        assert_eq!(store.claim_otp_attempt(&c.id, T0).await.unwrap(), None);
    }

    /// No challenge is `None`, and deleting it is not an error.
    pub async fn an_unknown_challenge_is_none<S: OtpStore>(store: Arc<S>) {
        let id = OtpChallengeId::new("never");
        assert_eq!(store.claim_otp_attempt(&id, T0).await.unwrap(), None);
        store.delete_otp_challenge(&id).await.unwrap();
    }

    /// The purge removes what expired at or before `now`, answers how many,
    /// and leaves the rest.
    pub async fn purging_removes_only_expired_challenges<S: OtpStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        for (id, expires_at) in [("old", T0 - 1), ("edge", T0), ("live", T0 + 1)] {
            let mut c = challenge(id, &ada, 5);
            c.expires_at = expires_at;
            store.create_otp_challenge(c).await.unwrap();
        }
        assert_eq!(store.purge_otp_challenges(T0).await.unwrap(), 2);
        assert_eq!(store.purge_otp_challenges(T0).await.unwrap(), 0);
        assert!(store
            .claim_otp_attempt(&OtpChallengeId::new("live"), T0)
            .await
            .unwrap()
            .is_some());
        assert!(store
            .claim_otp_attempt(&OtpChallengeId::new("old"), T0)
            .await
            .unwrap()
            .is_none());
    }

    /// Many guesses at once against a cap of five: exactly five are counted,
    /// each with its own attempt number, and the rest find no challenge.
    pub async fn concurrent_claims_never_pass_the_cap<S: OtpStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let c = challenge("contested", &ada, 5);
        store.create_otp_challenge(c.clone()).await.unwrap();
        let results = race(&store, |store, _| {
            let id = c.id.clone();
            async move { store.claim_otp_attempt(&id, T0).await.expect("claim") }
        })
        .await;
        let mut counted: Vec<u32> = results.iter().flatten().map(|r| r.attempts).collect();
        counted.sort_unstable();
        assert_eq!(
            counted,
            vec![1, 2, 3, 4, 5],
            "a cap of five allowed these attempts: {counted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// SignInLocationStore
// ---------------------------------------------------------------------------

/// The [`SignInLocationStore`] cases.
pub mod sign_in_location_store {
    use super::*;

    /// The first sign-in from a network answers `None`; the next one answers
    /// when the first was. Networks are per account.
    pub async fn a_network_is_new_once<S: SignInLocationStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let eve = account(&*store, None).await;
        let home = [1u8; 32];
        assert_eq!(
            store
                .record_sign_in_location(&ada, &home, T0)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            store
                .record_sign_in_location(&ada, &home, T0 + 10)
                .await
                .unwrap(),
            Some(T0)
        );
        assert_eq!(
            store
                .record_sign_in_location(&ada, &home, T0 + 20)
                .await
                .unwrap(),
            Some(T0 + 10)
        );
        assert_eq!(
            store
                .record_sign_in_location(&eve, &home, T0)
                .await
                .unwrap(),
            None,
            "the same network is new for another account"
        );
        store
            .record_sign_in_location(&ada, &[2u8; 32], T0)
            .await
            .unwrap();
        assert_eq!(store.count_sign_in_locations(&ada).await.unwrap(), 2);
        assert_eq!(store.count_sign_in_locations(&eve).await.unwrap(), 1);
    }

    /// The purge forgets networks last seen before the cut-off, and keeps one
    /// seen exactly at it.
    pub async fn purging_forgets_unused_networks<S: SignInLocationStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        store
            .record_sign_in_location(&ada, &[1; 32], T0 - 1)
            .await
            .unwrap();
        store
            .record_sign_in_location(&ada, &[2; 32], T0)
            .await
            .unwrap();
        store
            .record_sign_in_location(&ada, &[3; 32], T0 + 1)
            .await
            .unwrap();
        assert_eq!(store.purge_sign_in_locations(T0).await.unwrap(), 1);
        assert_eq!(store.count_sign_in_locations(&ada).await.unwrap(), 2);
        assert_eq!(
            store
                .record_sign_in_location(&ada, &[1; 32], T0 + 5)
                .await
                .unwrap(),
            None,
            "a forgotten network is new again"
        );
    }

    /// Many sign-ins from one new network at once: exactly one reports it new,
    /// and each of the others reports a different earlier sign-in.
    pub async fn concurrent_first_sign_ins_report_new_once<S: SignInLocationStore>(store: Arc<S>) {
        let ada = account(&*store, None).await;
        let results = race(&store, |store, i| {
            let ada = ada.clone();
            async move {
                store
                    .record_sign_in_location(&ada, &[5; 32], T0 + i as i64)
                    .await
                    .expect("record")
            }
        })
        .await;
        let new = results.iter().filter(|r| r.is_none()).count();
        assert_eq!(new, 1, "one network was reported new {new} times");
        let previous: HashSet<UnixTime> = results.iter().flatten().copied().collect();
        assert_eq!(
            previous.len(),
            RACERS - 1,
            "each sign-in saw a different previous one: {results:?}"
        );
        assert_eq!(store.count_sign_in_locations(&ada).await.unwrap(), 1);
    }
}
