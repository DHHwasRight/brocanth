//! `PgStore`, against a real Postgres.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://localhost/brocanth_test \
//!   cargo test -p brocanth-middleware --features postgres
//! ```
//!
//! Without the variable each test prints a line to the real standard
//! error and returns. Each test creates its own schema and drops it.

#![cfg(feature = "postgres")]

use std::sync::Arc;
use std::time::Duration;

use brocanth_middleware::rate_limit::{PgStore, Store};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

fn url() -> Option<String> {
    std::env::var("BROCANTH_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
}

/// Writes past libtest's capture, so a skip is seen.
fn shout(name: &str) {
    use std::io::Write as _;
    let _ = writeln!(
        std::io::stderr(),
        "SKIPPED {name}: BROCANTH_TEST_DATABASE_URL is not set"
    );
}

/// A pool whose every connection works in a fresh schema.
async fn pool(name: &str) -> Option<(PgPool, String)> {
    let Some(url) = url() else {
        shout(name);
        return None;
    };
    let schema = format!("rl_{name}_{}", std::process::id());
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .expect("a pool");
    drop_schema(&pool, &schema).await;
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&pool)
        .await
        .expect("a schema");
    Some((pool, schema))
}

async fn drop_schema(pool: &PgPool, schema: &str) {
    let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP SCHEMA IF EXISTS {schema} CASCADE"
    )))
    .execute(pool)
    .await;
}

async fn store(pool: &PgPool, schema: &str) -> PgStore {
    let store = PgStore::in_table(pool.clone(), format!("{schema}.limits")).expect("a name");
    store.create_table().await.expect("the table");
    store
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_table_is_unlogged() {
    let Some((pool, schema)) = pool("unlogged").await else {
        return;
    };
    store(&pool, &schema).await;
    let persistence: String = sqlx::query_scalar(
        "SELECT c.relpersistence::text FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = $1 AND c.relname = 'limits'",
    )
    .bind(&schema)
    .fetch_one(&pool)
    .await
    .expect("the table exists");
    assert_eq!(persistence, "u");
    drop_schema(&pool, &schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_key_counts_up_inside_its_window() {
    let Some((pool, schema)) = pool("counts").await else {
        return;
    };
    let store = store(&pool, &schema).await;
    // A long window, so the test does not straddle its end.
    let window = Duration::from_secs(86_400);
    let first = store.hit("login:ip:192.0.2.1", window).await.unwrap();
    let second = store.hit("login:ip:192.0.2.1", window).await.unwrap();
    let other = store.hit("login:ip:192.0.2.2", window).await.unwrap();
    assert_eq!((first.count, second.count, other.count), (1, 2, 1));
    assert!(second.resets_in <= window && second.resets_in > Duration::ZERO);
    drop_schema(&pool, &schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ended_window_starts_again_at_one_and_is_swept() {
    let Some((pool, schema)) = pool("ended").await else {
        return;
    };
    let store = store(&pool, &schema).await;
    let window = Duration::from_secs(86_400);
    store.hit("k", window).await.unwrap();
    store.hit("k", window).await.unwrap();
    // Move the stored window into the past, as time would.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {schema}.limits SET window_end = window_end - 172800"
    )))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(store.hit("k", window).await.unwrap().count, 1);

    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {schema}.limits SET window_end = 0"
    )))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(store.sweep().await.unwrap(), 1);
    drop_schema(&pool, &schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_counts_of_one_key_are_never_lost() {
    let Some((pool, schema)) = pool("concurrent").await else {
        return;
    };
    let store = Arc::new(store(&pool, &schema).await);
    let window = Duration::from_secs(86_400);
    let mut tasks = Vec::new();
    for _ in 0..50 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store.hit("busy", window).await.unwrap().count
        }));
    }
    let mut counts = Vec::new();
    for task in tasks {
        counts.push(task.await.unwrap());
    }
    counts.sort_unstable();
    assert_eq!(counts, (1..=50).collect::<Vec<u64>>());
    drop_schema(&pool, &schema).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_shared_plugin_refuses_through_postgres() {
    use brocanth::plugin_api::Plugin;
    use brocanth::plugin_app::AppBuilder;
    use brocanth::plugin_body::Body;
    use brocanth::plugin_context::Base;
    use brocanth::plugin_routes::{Handler, RouteCx};
    use brocanth::routing::{Router, Verb};
    use brocanth_middleware::rate_limit::{RateLimitConfig, Rule, SharedConfig};
    use brocanth_middleware::SharedRateLimit;
    use brocanth_test::{TestClient, TestRequest};

    let Some((pool, schema)) = pool("plugin").await else {
        return;
    };
    let store = Arc::new(store(&pool, &schema).await);
    let app = AppBuilder::new()
        .plugin_hooking(SharedRateLimit::new(SharedConfig {
            config: RateLimitConfig::rules([Rule::new("api", 2, 86_400)])
                .ip_header("fly-client-ip"),
            store,
        }))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    let ok: Handler<Base> =
        Box::new(|_cx: &mut RouteCx<'_, Base>| Ok(http::Response::new(Body::from(Vec::new()))));
    table.route("/", Verb::Get, ok).expect("a route");
    let client = TestClient::serving(app, table.build().expect("a table"));
    let get = || TestRequest::get("/").header("fly-client-ip", "192.0.2.1");

    client.send_async(get()).await.assert_ok();
    client
        .send_async(get())
        .await
        .assert_ok()
        .assert_header("ratelimit-remaining", "0");
    let refused = client.send_async(get()).await;
    refused.assert_status(429);
    assert!(refused.header("retry-after").is_some());
    drop_schema(&pool, &schema).await;
}

#[test]
fn a_table_name_that_is_not_an_identifier_is_refused() {
    // `connect_lazy` opens nothing, so this needs no server.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let pool = PgPool::connect_lazy("postgres://localhost/none").unwrap();
        for bad in ["a;drop table x", "a.b.c", "", "1abc", "a b"] {
            assert!(PgStore::in_table(pool.clone(), bad).is_err(), "{bad}");
        }
        assert!(PgStore::in_table(pool.clone(), "app.limits").is_ok());
    });
}
