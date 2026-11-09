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

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig};
use brocanth_flags::{
    Actor, Flag, FlagStore, Flags, FlagsConfig, FlagsPlugin, MemoryStore, PgStore, Rule,
};

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

/// A schema of its own, a pool whose `search_path` points at it, and a
/// store over that pool. The table is not created: a test says when.
struct Sandbox {
    admin: Db,
    schema: String,
    db: Db,
    store: PgStore,
}

impl Sandbox {
    async fn new() -> Option<Sandbox> {
        static N: AtomicU32 = AtomicU32::new(0);
        let Some(url) = url() else {
            skipped();
            return None;
        };
        let schema = format!(
            "brocanth_flags_s{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        let mut cfg = DbConfig::new(&url);
        cfg.max_connections = 2;
        let admin = Db::connect(&cfg).await.expect("connect");
        exec(&admin, &format!("CREATE SCHEMA {schema}")).await;
        let mut cfg = DbConfig::new(&url);
        cfg.max_connections = 3;
        cfg.search_path = vec![schema.clone()];
        let db = Db::connect(&cfg).await.expect("connect");
        let store = PgStore::new(db.clone());
        Some(Sandbox {
            admin,
            schema,
            db,
            store,
        })
    }

    async fn drop(self) {
        self.db.close().await;
        exec(&self.admin, &format!("DROP SCHEMA {} CASCADE", self.schema)).await;
    }
}

/// What every store must do, run once over memory and once over Postgres.
async fn the_store_contract(store: Arc<dyn FlagStore>) {
    assert!(
        store.load().await.unwrap().is_empty(),
        "a new store is empty"
    );
    let rule = Rule {
        on: false,
        percent: 40,
        actors: vec!["ada".into(), "o'brien; DROP TABLE x".into(), "é".into()],
        groups: vec!["staff".into(), String::new()],
    };
    store.save("beta", &rule).await.unwrap();
    store.save("other.v2-x", &Rule::everyone()).await.unwrap();
    let loaded = store.load().await.unwrap();
    assert_eq!(loaded.get("beta"), Some(&rule), "values round-trip as text");
    assert_eq!(loaded.get("other.v2-x"), Some(&Rule::everyone()));

    // A save replaces the row: no list is merged with the old one.
    store.save("beta", &Rule::percent(5)).await.unwrap();
    assert_eq!(store.load().await.unwrap()["beta"], Rule::percent(5));

    store.remove("beta").await.unwrap();
    store.remove("never-set").await.unwrap();
    let left = store.load().await.unwrap();
    assert_eq!(left.keys().collect::<Vec<_>>(), ["other.v2-x"]);
}

#[tokio::test]
async fn the_memory_store_keeps_the_contract() {
    the_store_contract(Arc::new(MemoryStore::new())).await;
}

#[tokio::test]
async fn the_postgres_store_keeps_the_contract() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    sandbox.store.migrate().await.unwrap();
    the_store_contract(Arc::new(sandbox.store.clone())).await;
    sandbox.drop().await;
}

#[tokio::test]
async fn the_table_refuses_a_percent_outside_zero_to_a_hundred() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    sandbox.store.migrate().await.unwrap();
    for bad in [101, -1] {
        let refused = sqlx::query("INSERT INTO brocanth_flags (key, percent) VALUES ('x', $1)")
            .bind(bad as i16)
            .execute(sandbox.db.pool())
            .await;
        assert!(refused.is_err(), "the CHECK refuses {bad}");
    }
    assert!(sandbox.store.load().await.unwrap().is_empty());
    sandbox.drop().await;
}

#[tokio::test]
async fn the_plugin_creates_the_table_and_reads_it_before_it_is_ready() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let plugin = FlagsPlugin::new(FlagsConfig {
        refresh_secs: 0,
        migrate: true,
    });
    let flags = plugin.flags();
    let mut builder = AppBuilder::new();
    builder.services_mut().insert(sandbox.db.clone());
    let app = builder.plugin(plugin).build().expect("build");
    app.readiness().check_all().await.expect("ready");
    assert!(flags.rules().is_empty());

    // Another process turns the flag on. A readiness check reads the table
    // again, so this copy sees it without a restart.
    let there = Flags::with_store(Arc::new(sandbox.store.clone()));
    there.set("beta", Rule::everyone()).await.unwrap();
    assert!(!flags.enabled(BETA, &Actor::anonymous()), "not read yet");
    app.readiness().check_all().await.expect("ready");
    assert!(flags.enabled(BETA, &Actor::anonymous()));

    // A change made here is in the table for the other process.
    flags.remove("beta").await.unwrap();
    there.refresh().await.unwrap();
    assert!(!there.enabled(BETA, &Actor::anonymous()));
    sandbox.drop().await;
}

#[tokio::test]
async fn without_migrate_a_missing_table_is_not_ready_and_says_what_to_do() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let plugin = FlagsPlugin::new(FlagsConfig {
        refresh_secs: 0,
        migrate: false,
    });
    let mut builder = AppBuilder::new();
    builder.services_mut().insert(sandbox.db.clone());
    let app = builder.plugin(plugin).build().expect("build");
    let e = app.readiness().check_all().await.unwrap_err().to_string();
    assert!(e.contains("[flags] reading the table"), "{e}");
    assert!(e.contains("help:"), "{e}");
    assert!(
        exists(&sandbox.db, "brocanth_flags").await.is_none(),
        "nothing was created"
    );
    sandbox.drop().await;
}

#[tokio::test]
async fn the_refresh_task_reads_the_table_every_refresh_secs() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let plugin = FlagsPlugin::new(FlagsConfig {
        refresh_secs: 1,
        migrate: true,
    });
    let flags = plugin.flags();
    let mut builder = AppBuilder::new();
    builder.services_mut().insert(sandbox.db.clone());
    let mut app = builder.plugin(plugin).build().expect("build");
    app.readiness().check_all().await.expect("ready");
    Flags::with_store(Arc::new(sandbox.store.clone()))
        .set("beta", Rule::everyone())
        .await
        .unwrap();
    let mut seen = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if flags.enabled(BETA, &Actor::anonymous()) {
            seen = true;
            break;
        }
    }
    assert!(seen, "the task read the change within 3 s");
    let _ = app.shutdown(brocanth::plugin_shutdown::ShutdownReason::Requested, None);
    sandbox.drop().await;
}

async fn exists(db: &Db, table: &str) -> Option<String> {
    sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass($1)::text")
        .bind(table)
        .fetch_one(db.pool())
        .await
        .expect("a query")
}
