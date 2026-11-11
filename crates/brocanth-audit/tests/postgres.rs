//! `PgStore` against a real Postgres. Each test makes its own schema,
//! points its pool's `search_path` at it, and drops it at the end.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pw@localhost:55432/postgres \
//!   cargo test -p brocanth-audit
//! ```
//!
//! Without a URL, each test says `!! SKIPPED` on the real stderr and passes.

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::Arc;

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_audit::{
    AuditConfig, AuditPlugin, AuditStore, Entry, Field, Kind, MemoryStore, PgStore, Query, Trail,
};
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig};
use brocanth_events::{ChangeSink, RowChange};

fn url() -> Option<String> {
    ["BROCANTH_AUDIT_TEST_POSTGRES", "BROCANTH_TEST_DATABASE_URL"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|u| !u.trim().is_empty()))
}

fn skipped() {
    use std::io::Write as _;
    let name = std::thread::current().name().unwrap_or("a test").to_owned();
    let _ = std::io::stderr()
        .write_all(format!("!! SKIPPED {name}: set BROCANTH_TEST_DATABASE_URL\n").as_bytes());
}

struct Sandbox {
    admin: Db,
    schema: String,
    store: PgStore,
    db: Db,
}

impl Sandbox {
    async fn new() -> Option<Sandbox> {
        let sandbox = Sandbox::bare().await?;
        sandbox.store.migrate().await.expect("the schema");
        // Twice: the schema is idempotent.
        sandbox.store.migrate().await.expect("the schema again");
        Some(sandbox)
    }

    /// A schema with no table in it.
    async fn bare() -> Option<Sandbox> {
        static N: AtomicU32 = AtomicU32::new(0);
        let Some(url) = url() else {
            skipped();
            return None;
        };
        let schema = format!(
            "brocanth_audit_t{}_{}",
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
            store,
            db,
        })
    }

    async fn drop(self) {
        self.db.close().await;
        exec(&self.admin, &format!("DROP SCHEMA {} CASCADE", self.schema)).await;
    }
}

async fn exec(db: &Db, sql: &str) {
    sqlx::query(AssertSqlSafe(sql.to_owned()))
        .execute(db.pool())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

fn entry(at: i64, table: &str, row: &str, kind: Kind, fields: Vec<Field>) -> Entry {
    Entry {
        id: 0,
        at,
        actor: Some("ada".to_string()),
        source: "admin".to_string(),
        table: table.to_string(),
        row: row.to_string(),
        label: format!("{table} {row}"),
        kind,
        fields,
    }
}

#[tokio::test]
async fn entries_are_stored_with_their_values_and_read_newest_first() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let edit = vec![
        Field {
            name: "title".into(),
            before: Some("Old".into()),
            after: Some("New".into()),
        },
        Field {
            name: "published_at".into(),
            before: None,
            after: Some("2026-11-09".into()),
        },
    ];
    let entries = vec![
        entry(100, "post", "7", Kind::Created, Vec::new()),
        entry(200, "post", "7", Kind::Updated, edit.clone()),
        entry(300, "user", "1", Kind::Deleted, vec![Field::named("email")]),
    ];
    sandbox.store.append(&entries).await.unwrap();

    let post = sandbox.store.find(&Query::row("post", "7")).await.unwrap();
    assert_eq!(post.len(), 2);
    assert_eq!(post[0].kind, Kind::Updated);
    assert_eq!(post[0].fields, edit, "a NULL value reads back as None");
    assert_eq!(post[0].actor.as_deref(), Some("ada"));
    assert!(post[0].id > post[1].id);

    let all = sandbox.store.find(&Query::recent()).await.unwrap();
    assert_eq!(all.len(), 3);
    let older = sandbox
        .store
        .find(&Query::recent().before(all[0].id).limit(1))
        .await
        .unwrap();
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].id, all[1].id);
    sandbox.drop().await;
}

#[tokio::test]
async fn the_purge_removes_only_the_older_entries() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let entries = vec![
        entry(100, "post", "1", Kind::Created, Vec::new()),
        entry(500, "post", "2", Kind::Created, Vec::new()),
    ];
    sandbox.store.append(&entries).await.unwrap();
    assert_eq!(sandbox.store.purge(200).await.unwrap(), 1);
    let left = sandbox.store.find(&Query::recent()).await.unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].row, "2");
    sandbox.drop().await;
}

/// What every store must do, run once over memory and once over Postgres.
async fn the_store_contract(store: Arc<dyn AuditStore>) {
    assert!(store.find(&Query::recent()).await.unwrap().is_empty());
    store.append(&[]).await.unwrap();
    let mut batch = Vec::new();
    for n in 0..6 {
        batch.push(entry(
            100 + n,
            "post",
            &(n % 3).to_string(),
            Kind::Updated,
            Vec::new(),
        ));
    }
    let mut odd = entry(
        200,
        "user",
        "o'brien; DROP TABLE brocanth_audit --",
        Kind::Deleted,
        vec![Field {
            name: "bio".into(),
            before: None,
            after: None,
        }],
    );
    odd.actor = None;
    batch.push(odd.clone());
    store.append(&batch).await.unwrap();

    let all = store.find(&Query::recent()).await.unwrap();
    assert_eq!(all.len(), 7);
    assert!(all.windows(2).all(|w| w[0].id > w[1].id), "newest first");

    let row = store.find(&Query::row("post", "1")).await.unwrap();
    assert_eq!(row.len(), 2);
    assert!(row.iter().all(|e| e.table == "post" && e.row == "1"));

    let posts = store
        .find(&Query {
            table: Some("post".into()),
            ..Query::default()
        })
        .await
        .unwrap();
    assert_eq!(posts.len(), 6);

    // A row filter with no table matches every table.
    let row_only = Query {
        row: Some("1".into()),
        ..Query::default()
    };
    assert_eq!(store.find(&row_only).await.unwrap().len(), 7);

    // A value that looks like SQL is a value.
    let odd_back = store
        .find(&Query::row("user", "o'brien; DROP TABLE brocanth_audit --"))
        .await
        .unwrap();
    assert_eq!(odd_back.len(), 1);
    assert_eq!(odd_back[0].actor, None);
    assert_eq!(odd_back[0].fields, odd.fields);

    // A limit of 0 reads one; a huge one is capped.
    assert_eq!(
        store.find(&Query::recent().limit(0)).await.unwrap().len(),
        1
    );
    assert_eq!(
        store
            .find(&Query::recent().limit(usize::MAX))
            .await
            .unwrap()
            .len(),
        7
    );

    // Purge: strictly before the cut.
    assert_eq!(store.purge(103).await.unwrap(), 3);
    let left = store.find(&Query::recent()).await.unwrap();
    assert_eq!(left.len(), 4);
    assert!(left.iter().all(|e| e.at >= 103));
    assert_eq!(store.purge(0).await.unwrap(), 0);
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
    the_store_contract(Arc::new(sandbox.store.clone())).await;
    sandbox.drop().await;
}

#[tokio::test]
async fn a_trail_over_postgres_records_create_update_delete_with_the_actor() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let trail = Trail::with_store(Arc::new(sandbox.store.clone()), 0);
    let fields = ["title"];
    trail.by("ada").publish(&RowChange::created("post", 7_i64));
    trail
        .by("bob")
        .publish(&RowChange::updated("post", 7_i64, fields.as_slice()));
    trail.publish(&RowChange::deleted("post", 7_i64));
    assert_eq!(trail.flush().await.unwrap(), 3);
    let history = trail.history("post", "7").await.unwrap();
    let seen: Vec<(Kind, Option<&str>)> = history
        .iter()
        .map(|e| (e.kind, e.actor.as_deref()))
        .collect();
    assert_eq!(
        seen,
        [
            (Kind::Deleted, None),
            (Kind::Updated, Some("bob")),
            (Kind::Created, Some("ada")),
        ]
    );
    assert_eq!(history[1].fields, vec![Field::named("title")]);
    sandbox.drop().await;
}

#[tokio::test]
async fn the_retention_removes_only_rows_past_it_in_postgres() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let now = Arc::new(AtomicI64::new(0));
    let clock = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let trail = Trail::with_clock(30, clock);
    trail.attach(Arc::new(sandbox.store.clone()));
    let day = 86_400;
    for (at, row) in [(0, "old"), (day, "edge"), (31 * day, "new")] {
        now.store(at, Ordering::SeqCst);
        trail.publish(&RowChange::created("post", row));
    }
    trail.flush().await.unwrap();
    // 31 days: the cut is day 1. "edge" is exactly 30 days old and stays.
    assert_eq!(trail.purge().await.unwrap(), 1);
    let rows: Vec<String> = trail
        .find(&Query::recent())
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.row)
        .collect();
    assert_eq!(rows, ["new", "edge"]);

    // Retention 0 keeps every row, however old.
    let forever = Trail::with_store(Arc::new(sandbox.store.clone()), 0);
    assert_eq!(forever.purge().await.unwrap(), 0);
    assert_eq!(forever.find(&Query::recent()).await.unwrap().len(), 2);
    sandbox.drop().await;
}

#[tokio::test]
async fn the_plugin_creates_the_table_on_readiness_and_writes_to_it() {
    let Some(sandbox) = Sandbox::bare().await else {
        return;
    };
    let plugin = AuditPlugin::new(AuditConfig::default());
    let trail = plugin.trail();
    let mut builder = AppBuilder::new();
    builder.services_mut().insert(sandbox.db.clone());
    let app = builder.plugin_hooking(plugin).build().expect("build");
    app.readiness().check_all().await.expect("ready");
    assert!(table_exists(&sandbox.db).await);
    trail.by("ada").publish(&RowChange::created("post", 1_i64));
    trail.flush().await.unwrap();
    let rows = sandbox.store.find(&Query::row("post", "1")).await.unwrap();
    assert_eq!(rows[0].actor.as_deref(), Some("ada"));
    sandbox.drop().await;
}

#[tokio::test]
async fn without_migrate_no_table_is_made_and_a_failed_flush_keeps_the_entries() {
    let Some(sandbox) = Sandbox::bare().await else {
        return;
    };
    let plugin = AuditPlugin::new(AuditConfig {
        migrate: false,
        ..AuditConfig::default()
    });
    let trail = plugin.trail();
    let mut builder = AppBuilder::new();
    builder.services_mut().insert(sandbox.db.clone());
    let app = builder.plugin_hooking(plugin).build().expect("build");
    app.readiness()
        .check_all()
        .await
        .expect("no check waits on the table");
    assert!(!table_exists(&sandbox.db).await);

    trail.publish(&RowChange::created("post", 1_i64));
    assert!(trail.flush().await.is_err());
    assert_eq!(trail.pending(), 1, "kept for the next flush");
    assert!(trail.last_error().unwrap().contains("brocanth_audit"));

    // The table appears (a migration ran); the next flush writes it.
    sandbox.store.migrate().await.unwrap();
    assert_eq!(trail.flush().await.unwrap(), 1);
    assert_eq!(trail.last_error(), None);
    sandbox.drop().await;
}

async fn table_exists(db: &Db) -> bool {
    sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass('brocanth_audit')::text")
        .fetch_one(db.pool())
        .await
        .expect("a query")
        .is_some()
}
