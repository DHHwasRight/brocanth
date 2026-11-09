//! `PgStore` against a real Postgres. Each test makes its own schema,
//! points its pool's `search_path` at it, and drops it at the end.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pw@localhost:55432/postgres \
//!   cargo test -p brocanth-audit
//! ```
//!
//! Without a URL, each test says `!! SKIPPED` on the real stderr and passes.

use std::sync::atomic::{AtomicU32, Ordering};

use brocanth_audit::{AuditStore, Entry, Field, Kind, PgStore, Query};
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig};

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
        store.migrate().await.expect("the schema");
        // Twice: the schema is idempotent.
        store.migrate().await.expect("the schema again");
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
