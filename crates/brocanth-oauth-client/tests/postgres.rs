//! The Postgres store: the store contract, and a refresh that rotates the
//! refresh token through it.
//!
//! Skipped without `BROCANTH_OAUTH_CLIENT_TEST_POSTGRES` or
//! `BROCANTH_TEST_DATABASE_URL`. Each test works in its own schema and
//! drops it at the end, so nothing shared is truncated.

mod support;

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use brocanth_db::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use brocanth_db::sqlx::{self, PgPool};
use brocanth_db::Db;
use brocanth_oauth_client::web::ConnectRoutes;
use brocanth_oauth_client::{PgTokenStore, TokenStore};
use support::{client, connect, Fake, Manual};

fn database_url() -> Option<String> {
    [
        "BROCANTH_OAUTH_CLIENT_TEST_POSTGRES",
        "BROCANTH_TEST_DATABASE_URL",
    ]
    .iter()
    .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
}

/// A pool on a new schema, and the admin pool that drops it.
struct Scoped {
    db: Db,
    admin: PgPool,
    schema: String,
}

impl Scoped {
    async fn drop_schema(self) {
        self.db.pool().close().await;
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP SCHEMA IF EXISTS {} CASCADE",
            self.schema
        )))
        .execute(&self.admin)
        .await;
    }
}

async fn scoped(test: &str) -> Option<Scoped> {
    let Some(url) = database_url() else {
        eprintln!("*** SKIPPED (no database): {test}");
        return None;
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let schema = format!(
        "brocanth_oauth_client_t{}_{}",
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
        .max_connections(8)
        .connect_with(options)
        .await
        .expect("connecting to the test schema");
    Some(Scoped {
        db: Db::from_pool(pool),
        admin,
        schema,
    })
}

#[tokio::test]
async fn the_postgres_store_keeps_the_contract() {
    let Some(scope) = scoped("the_postgres_store_keeps_the_contract").await else {
        return;
    };
    let store = PgTokenStore::new(scope.db.clone());
    store.migrate().await.expect("migrate");
    store.migrate().await.expect("migrate is idempotent");
    brocanth_oauth_client::conformance::run(&store).await;
    scope.drop_schema().await;
}

#[tokio::test]
async fn a_refresh_through_postgres_keeps_the_rotated_token_sealed() {
    let Some(scope) = scoped("a_refresh_through_postgres_keeps_the_rotated_token_sealed").await
    else {
        return;
    };
    let store = Arc::new(PgTokenStore::new(scope.db.clone()));
    store.migrate().await.expect("migrate");
    let fake = Fake::start().await;
    let clock = Manual::at(1_800_000_000);
    let shared: Arc<dyn TokenStore> = store.clone();
    let oauth = client(&fake, Arc::clone(&clock), shared);
    let routes = ConnectRoutes::new(oauth.clone(), "/connections", false);
    connect(&fake, &routes, "alice").await;

    clock.advance(3600);
    let calls = (0..8).map(|_| {
        let oauth = oauth.clone();
        tokio::spawn(async move { oauth.token("alice", "fake").await })
    });
    for call in calls {
        call.await.unwrap().expect("a token");
    }
    assert_eq!(fake.refreshes.load(Ordering::SeqCst), 1);
    let token = oauth.token("alice", "fake").await.unwrap();
    assert_eq!(token.refresh_token(), Some("rt-4"));

    // The columns hold sealed bytes, not the tokens.
    let (access, refresh): (Vec<u8>, Vec<u8>) =
        sqlx::query_as("SELECT access, refresh FROM brocanth_oauth_tokens WHERE account = 'alice'")
            .fetch_one(scope.db.pool())
            .await
            .unwrap();
    for column in [&access, &refresh] {
        assert!(!column.windows(4).any(|w| w == b"at-3" || w == b"rt-4"));
    }

    // A second client over the same table, as a second instance: it reads
    // the token the first one stored.
    let other = client(&fake, Arc::clone(&clock), store.clone());
    assert_eq!(other.token("alice", "fake").await.unwrap(), token);
    scope.drop_schema().await;
}
