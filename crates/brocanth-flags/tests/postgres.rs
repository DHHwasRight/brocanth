//! `PgStore` against a real Postgres. Each test makes its own schema,
//! points its pool's `search_path` at it, and drops it at the end.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pw@localhost:55432/postgres \
//!   cargo test -p brocanth-flags
//! ```
//!
//! Without a URL, each test says `!! SKIPPED` on the real stderr and passes.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig};
use brocanth_flags::{Actor, Flag, FlagStore, Flags, PgStore, Rule};

fn url() -> Option<String> {
    ["BROCANTH_FLAGS_TEST_POSTGRES", "BROCANTH_TEST_DATABASE_URL"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|u| !u.trim().is_empty()))
}

fn skipped() {
    use std::io::Write as _;
    let name = std::thread::current().name().unwrap_or("a test").to_owned();
    let _ = std::io::stderr()
        .write_all(format!("!! SKIPPED {name}: set BROCANTH_TEST_DATABASE_URL\n").as_bytes());
}

async fn exec(db: &Db, sql: &str) {
    sqlx::query(AssertSqlSafe(sql.to_owned()))
        .execute(db.pool())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

const BETA: Flag = Flag::new("beta", "The beta.");

#[tokio::test]
async fn a_rule_is_stored_read_back_by_another_copy_and_removed() {
    static N: AtomicU32 = AtomicU32::new(0);
    let Some(url) = url() else {
        skipped();
        return;
    };
    let schema = format!(
        "brocanth_flags_t{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let admin = Db::connect(&DbConfig::new(&url)).await.expect("connect");
    exec(&admin, &format!("CREATE SCHEMA {schema}")).await;
    let mut cfg = DbConfig::new(&url);
    cfg.search_path = vec![schema.clone()];
    let db = Db::connect(&cfg).await.expect("connect");
    let store = PgStore::new(db.clone());
    store.migrate().await.expect("the schema");
    store.migrate().await.expect("the schema again");

    // Two processes, one table.
    let here = Flags::with_store(Arc::new(store.clone()));
    let there = Flags::with_store(Arc::new(store.clone()));
    let rule = Rule {
        percent: 30,
        actors: vec!["ada".into()],
        groups: vec!["staff".into()],
        ..Rule::default()
    };
    here.set("beta", rule.clone()).await.unwrap();
    there.refresh().await.unwrap();
    assert_eq!(there.rule("beta"), Some(rule));
    assert!(there.enabled(BETA, &Actor::new("ada")));
    assert!(there.enabled(BETA, &Actor::new("zed").in_group("staff")));

    here.remove("beta").await.unwrap();
    assert!(store.load().await.unwrap().is_empty());
    there.refresh().await.unwrap();
    assert!(!there.enabled(BETA, &Actor::new("ada")));

    db.close().await;
    exec(&admin, &format!("DROP SCHEMA {schema} CASCADE")).await;
}
