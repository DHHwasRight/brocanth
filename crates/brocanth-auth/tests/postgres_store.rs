//! `PgStore` against the storage conformance suite, and the things only a
//! database can get wrong.
//!
//! Skipped unless a database is named:
//!
//! ```sh
//! BROCANTH_AUTH_TEST_POSTGRES=postgres://localhost/brocanth_auth_test \
//!   cargo test -p brocanth-auth --features postgres
//! ```
//!
//! `BROCANTH_TEST_DATABASE_URL` is read as a fallback. Set
//! `BROCANTH_AUTH_REQUIRE_POSTGRES=1` as well and a missing database fails
//! the run instead of skipping it.
//!
//! Every case gets its own schema, made at the start and dropped at the end,
//! so the cases run in parallel and nothing shared is truncated.

#![cfg(feature = "postgres")]

use std::str::FromStr as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use brocanth_auth::conformance::Fixture;
use brocanth_auth::storage::{AccountDirectory, AuthStore, NewAccount, SessionRecord, StoreError};
use brocanth_auth::{
    AccountId, EmailAddress, FactorSet, PgStore, Privilege, SessionId, Swept, TestClock,
    TokenPurpose, TokenRecord,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;

fn url() -> Option<String> {
    std::env::var("BROCANTH_AUTH_TEST_POSTGRES")
        .or_else(|_| std::env::var("BROCANTH_TEST_DATABASE_URL"))
        .ok()
        .filter(|u| !u.trim().is_empty())
}

/// A `PgStore` in a schema of its own, which [`Fixture::finish`] drops.
struct Scoped {
    store: Arc<PgStore>,
    admin: PgPool,
    schema: String,
}

#[async_trait]
impl Fixture for Scoped {
    type Store = PgStore;

    fn store(&self) -> Arc<PgStore> {
        self.store.clone()
    }

    async fn finish(self) {
        self.store.pool().close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await
        .expect("dropping the test schema");
        self.admin.close().await;
    }
}

async fn scoped(clock: Option<Arc<TestClock>>) -> Option<Scoped> {
    let Some(url) = url() else {
        assert!(
            std::env::var("BROCANTH_AUTH_REQUIRE_POSTGRES").is_err(),
            "BROCANTH_AUTH_REQUIRE_POSTGRES is set and no database is.\n\
             help: set BROCANTH_AUTH_TEST_POSTGRES or BROCANTH_TEST_DATABASE_URL"
        );
        return None;
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // Process id and a counter: unique within a run, and a leftover from a
    // crashed run is dropped by the next run with the same id.
    let schema = format!(
        "brocanth_auth_t{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let admin = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("connecting to the test database");
    for statement in [
        format!("DROP SCHEMA IF EXISTS {schema} CASCADE"),
        format!("CREATE SCHEMA {schema}"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&admin)
            .await
            .expect("making the test schema");
    }
    let options = PgConnectOptions::from_str(&url)
        .expect("a Postgres URL")
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(brocanth_auth::conformance::RACERS as u32 + 2)
        .connect_with(options)
        .await
        .expect("connecting to the test schema");
    let mut store = PgStore::from_pool(pool);
    if let Some(clock) = clock {
        store = store.with_clock(clock);
    }
    store.migrate().await.expect("migrate");
    Some(Scoped {
        store: Arc::new(store),
        admin,
        schema,
    })
}

async fn fresh() -> Option<Scoped> {
    scoped(None).await
}

brocanth_auth::auth_store_conformance!(fresh);
brocanth_auth::account_directory_conformance!(fresh);
brocanth_auth::mfa_store_conformance!(fresh);
brocanth_auth::otp_store_conformance!(fresh);
brocanth_auth::sign_in_location_store_conformance!(fresh);

/// Runs a body against a scoped store, and drops the schema afterwards.
macro_rules! with_store {
    ($clock:expr, |$store:ident| $body:block) => {{
        let Some(fixture) = scoped($clock).await else {
            use std::io::Write as _;
            let _ = writeln!(std::io::stderr(), "*** SKIPPED (no database)");
            return;
        };
        let $store = fixture.store();
        let outcome = tokio::spawn(async move { $body }).await;
        fixture.finish().await;
        if let Err(e) = outcome {
            std::panic::resume_unwind(e.into_panic());
        }
    }};
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_postgres_store_says_it_is_durable() {
    with_store!(None, |store| {
        assert!(store.is_durable());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rows_survive_a_new_pool() {
    // The point of the store: a second process (here, a second pool on the
    // same schema) sees what the first one wrote.
    with_store!(None, |store| {
        let account = store
            .create_account(NewAccount {
                email: Some(EmailAddress::parse("ada@example.com").unwrap()),
                ..NewAccount::default()
            })
            .await
            .unwrap();
        let options = store.pool().connect_options().as_ref().clone();
        let again = PgStore::from_pool(PgPoolOptions::new().connect_with(options).await.unwrap());
        assert_eq!(again.account(&account.id).await.unwrap(), Some(account));
        again.pool().close().await;
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn migrate_runs_twice_and_from_many_callers_at_once() {
    with_store!(None, |store| {
        let mut racers = Vec::new();
        for _ in 0..8 {
            let store = store.clone();
            racers.push(tokio::spawn(async move { store.migrate().await }));
        }
        for racer in racers {
            racer.await.unwrap().expect("a concurrent migrate failed");
        }
        store.migrate().await.unwrap();
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_address_written_with_capitals_still_collides() {
    // Some other code wrote this row, past `EmailAddress::parse`. The unique
    // index is on `lower(email)`, so the normalised address still collides
    // and still finds it.
    with_store!(None, |store| {
        sqlx::query(
            "INSERT INTO brocanth_auth_accounts (id, email, email_verified, created_at) \
             VALUES ('legacy', 'Ada@Example.COM', true, 1)",
        )
        .execute(store.pool())
        .await
        .unwrap();
        let ada = EmailAddress::parse("ada@example.com").unwrap();
        let found = store.account_by_email(&ada).await.unwrap().expect("found");
        assert_eq!(found.id, AccountId::new("legacy"));
        assert_eq!(found.email, Some(ada.clone()));
        let again = store
            .create_account(NewAccount {
                email: Some(ada),
                ..NewAccount::default()
            })
            .await;
        assert!(matches!(again, Err(StoreError::EmailTaken)), "{again:?}");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_token_secret_is_not_in_the_table() {
    // The row holds a selector and a digest. Issue a real token and look for
    // its secret half anywhere in the row.
    with_store!(None, |store| {
        let clock = Arc::new(TestClock::default());
        let tokens = brocanth_auth::Tokens::new(store.clone(), clock);
        let ada = AccountId::new("ada");
        let issued = tokens
            .issue(TokenPurpose::PasswordReset, &ada)
            .await
            .unwrap();
        let (selector, verifier) = issued.secret().split_once('.').unwrap();
        let row: (String, Vec<u8>) =
            sqlx::query_as("SELECT selector, verifier_hash FROM brocanth_auth_tokens")
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(row.0, selector);
        assert_eq!(row.1.len(), 32);
        assert!(!row
            .1
            .windows(verifier.len())
            .any(|w| w == verifier.as_bytes()));
        assert_eq!(
            tokens
                .consume(TokenPurpose::PasswordReset, issued.secret())
                .await
                .unwrap(),
            ada
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_store_stamps_rows_with_its_clock() {
    let clock = Arc::new(TestClock::at(1_234_567));
    with_store!(Some(clock), |store| {
        let account = store.create_account(NewAccount::default()).await.unwrap();
        assert_eq!(account.created_at, 1_234_567);
        assert_eq!(
            store
                .account(&account.id)
                .await
                .unwrap()
                .unwrap()
                .created_at,
            1_234_567
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sweep_deletes_dead_rows_and_keeps_live_ones() {
    with_store!(None, |store| {
        let now = 1_700_000_000;
        let idle = 1_800;
        let ada = AccountId::new("ada");
        let session = |id: &str, last_seen_at: i64, absolute_expires_at: i64| SessionRecord {
            id: SessionId::new(id),
            account: ada.clone(),
            privilege: Privilege::Authenticated,
            created_at: now - 10_000,
            last_seen_at,
            absolute_expires_at,
            factors: FactorSet::empty(),
            impersonation: None,
        };
        for s in [
            session("live", now - 10, now + 100),
            session("past-deadline", now - 10, now),
            session("idle", now - idle, now + 100),
        ] {
            store.create_session(s).await.unwrap();
        }
        for (selector, expires_at) in [("live", now + 1), ("dead", now)] {
            store
                .store_token(TokenRecord {
                    purpose: TokenPurpose::PasswordReset,
                    selector: selector.into(),
                    verifier_hash: [0; 32],
                    account: ada.clone(),
                    expires_at,
                })
                .await
                .unwrap();
        }

        let swept = store.sweep(now, idle).await.unwrap();
        assert_eq!(
            swept,
            Swept {
                sessions: 2,
                tokens: 1,
                otp_challenges: 0
            }
        );
        assert!(store
            .session(&SessionId::new("live"))
            .await
            .unwrap()
            .is_some());
        assert!(store
            .consume_token(TokenPurpose::PasswordReset, "live")
            .await
            .unwrap()
            .is_some());
        assert_eq!(store.sweep(now, idle).await.unwrap(), Swept::default());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_half_written_impersonation_marker_is_refused() {
    // The CHECK constraint: an impersonated session without its operator is
    // the audit failure the marker exists to prevent.
    with_store!(None, |store| {
        let refused = sqlx::query(
            "INSERT INTO brocanth_auth_sessions \
             (id, account, privilege, created_at, last_seen_at, absolute_expires_at, \
              impersonation_operator) \
             VALUES ('s', 'a', 'authenticated', 0, 0, 1, 'operator')",
        )
        .execute(store.pool())
        .await;
        assert!(refused.is_err());
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_url_that_is_not_postgres_is_refused_with_help() {
    let err = PgStore::connect("sqlite://app.db").await.unwrap_err();
    let text = err.to_string();
    assert!(text.contains("`sqlite` is not a Postgres URL"), "{text}");
    assert!(text.contains("help:"), "{text}");
}
