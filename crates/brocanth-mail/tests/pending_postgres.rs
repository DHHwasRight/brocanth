//! `PgPending`: a message reads back until it is deleted or expires.
//!
//! Skipped without `BROCANTH_TEST_DATABASE_URL`. The test works in its own
//! schema and drops it at the end, so nothing shared is truncated.
#![cfg(feature = "postgres")]

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use brocanth_db::sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use brocanth_db::sqlx::{self, PgPool};
use brocanth_db::Db;
use brocanth_mail::{Address, Mail, Memory, Message, Pending, PgPending};

async fn scoped(schema: &str) -> Option<(Db, PgPool)> {
    let Some(url) = std::env::var("BROCANTH_TEST_DATABASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        eprintln!("*** SKIPPED (no database): {schema}");
        return None;
    };
    let schema = format!("{schema}_{}", std::process::id());
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
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("connecting to the test schema");
    Some((Db::from_pool(pool), admin))
}

fn message() -> Message {
    let mail = Mail::new(
        Arc::new(Memory::new()),
        Address::parse("no-reply@example.test").expect("an address"),
    );
    mail.message()
        .to(Address::parse("person@example.test").expect("an address"))
        .subject("Reset your password")
        .text("https://example.test/reset/SECRET-TOKEN")
        .build()
        .expect("a message")
}

#[tokio::test]
async fn a_pending_row_reads_back_until_it_is_deleted_or_expires() {
    let Some((db, admin)) = scoped("brocanth_mail_pending_t").await else {
        return;
    };
    let store = PgPending::new(db.clone(), Duration::from_secs(60));
    store.migrate().await.expect("migrate");
    store.migrate().await.expect("migrate is idempotent");

    let id = store.put(&message()).await.expect("put");
    assert!(!id.contains("SECRET"));
    let back = store.get(&id).await.expect("get").expect("a row");
    assert_eq!(back.subject().as_str(), "Reset your password");
    assert!(store
        .get(&id)
        .await
        .expect("a retry reads it again")
        .is_some());
    store.delete(&id).await.expect("delete");
    assert!(store.get(&id).await.expect("get").is_none());

    let expired = PgPending::new(db.clone(), Duration::ZERO);
    let old = expired.put(&message()).await.expect("put");
    assert!(expired.get(&old).await.expect("get").is_none());
    // The next put deletes the expired row.
    let kept = store.put(&message()).await.expect("put");
    let rows: Vec<String> = sqlx::query_scalar("SELECT id FROM brocanth_mail_pending")
        .fetch_all(db.pool())
        .await
        .expect("rows");
    assert_eq!(rows, vec![kept]);

    let schema: String = sqlx::query_scalar("SELECT current_schema()")
        .fetch_one(db.pool())
        .await
        .expect("schema");
    db.pool().close().await;
    let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
        "DROP SCHEMA IF EXISTS {schema} CASCADE"
    )))
    .execute(&admin)
    .await;
}
