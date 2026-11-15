//! `Orm::migrate_locked`: two runs at one moment apply each migration once,
//! on a pool that is read-only and has a statement timeout.
//!
//! Skipped, with a line that says so, unless `BROCANTH_MODEL_TEST_POSTGRES`
//! or `BROCANTH_TEST_DATABASE_URL` names a server. The test works in a
//! schema of its own, made at the start and dropped at the end.

#![cfg(feature = "db")]

#[path = "../fixtures/blog.rs"]
mod blog;

use std::time::Duration;

use blog::migration::Migrator;
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig};
use brocanth_model::sea_orm_migration::{MigrationStatus, MigratorTrait};
use brocanth_model::testing::{skipped, Harness};
use brocanth_model::Orm;

#[tokio::test]
async fn two_runs_at_once_apply_each_migration_once() {
    let Some(url) = Harness::postgres_url() else {
        skipped(
            "no Postgres server; set BROCANTH_MODEL_TEST_POSTGRES or \
             BROCANTH_TEST_DATABASE_URL to run this",
        );
        return;
    };
    let schema = format!(
        "brocanth_model_migrating_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .subsec_nanos()
    );
    let admin = Db::connect(&DbConfig::new(url.clone()))
        .await
        .expect("connect");
    sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(admin.pool())
        .await
        .expect("create schema");

    // The limits a server's pool can carry: a migration must not inherit
    // them.
    let mut cfg = DbConfig::new(url);
    cfg.search_path = vec![schema.clone()];
    cfg.read_only = true;
    cfg.statement_timeout = Some(Duration::from_secs(1));
    let db = Db::connect(&cfg).await.expect("connect in the schema");
    let orm = Orm::from_db(&db);

    let run = || orm.migrate_locked(async |txn| Migrator::up(txn, None).await);
    let (a, b) = tokio::join!(run(), run());
    a.expect("first run");
    b.expect("second run");

    let status = orm
        .migrate_locked(async |txn| Migrator::get_migration_with_status(txn).await)
        .await
        .expect("status");
    assert!(!status.is_empty());
    assert!(status
        .iter()
        .all(|m| m.status() == MigrationStatus::Applied));

    // A failed run rolls back, and the next run is not blocked by a lock.
    let failed = orm
        .migrate_locked(async |_txn| -> Result<(), _> {
            Err(brocanth_model::sea_orm::DbErr::Custom("stop".into()))
        })
        .await;
    assert!(failed.is_err());
    orm.migrate_locked(async |txn| Migrator::down(txn, Some(1)).await)
        .await
        .expect("down after a failed run");

    db.close().await;
    sqlx::query(AssertSqlSafe(format!("DROP SCHEMA {schema} CASCADE")))
        .execute(admin.pool())
        .await
        .expect("drop schema");
    admin.close().await;
}
