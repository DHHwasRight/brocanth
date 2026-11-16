//! The embedded server, started for real: under the feature `embedded`
//! only (`cargo test -p brocanth-db --features embedded --test embedded`).
//!
//! The directory is under cargo's temporary target directory, so the
//! binaries are extracted once per target directory, not once per run.
#![cfg(feature = "embedded")]

use brocanth_db::embedded::{EmbeddedConfig, EmbeddedPostgres};
use brocanth_db::{Db, DbConfig};

#[test]
fn the_pool_opens_on_the_url_and_a_restart_keeps_the_rows() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("embedded-postgres");
    let config = EmbeddedConfig::new(&dir, "brocanth_test");
    let runtime = tokio::runtime::Runtime::new().expect("a runtime");

    // Started inside a runtime, as a plugin hook is.
    let server = runtime.block_on(async { EmbeddedPostgres::start(&config) });
    let server = server.expect("the server starts");
    assert!(!server.redacted_url().contains(&password(&dir)));
    runtime.block_on(async {
        let db = Db::connect(&DbConfig::new(server.url()))
            .await
            .expect("the pool connects");
        let mut tx = db.pool().begin().await.expect("a transaction");
        brocanth_db::sqlx::query("CREATE TABLE IF NOT EXISTS kept (n int)")
            .execute(&mut *tx)
            .await
            .expect("create");
        brocanth_db::sqlx::query("INSERT INTO kept VALUES (1)")
            .execute(&mut *tx)
            .await
            .expect("insert");
        tx.commit().await.expect("commit");
        db.close().await;
    });
    server.stop().expect("the server stops");
    assert!(!dir.join("data/postmaster.pid").exists());

    let server = EmbeddedPostgres::start(&config).expect("it starts again");
    let count: i64 = runtime.block_on(async {
        let db = Db::connect(&DbConfig::new(server.url()))
            .await
            .expect("the pool connects again");
        let (count,): (i64,) = brocanth_db::sqlx::query_as("SELECT count(*) FROM kept")
            .fetch_one(db.pool())
            .await
            .expect("count");
        db.close().await;
        count
    });
    assert!(count >= 1, "the row survived the restart");
    // A drop stops it too.
    drop(server);
    assert!(!dir.join("data/postmaster.pid").exists());
}

fn password(dir: &std::path::Path) -> String {
    std::fs::read_to_string(dir.join("password")).expect("the password file")
}
