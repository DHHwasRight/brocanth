//! `PgStore` against a real Postgres. Each test makes its own schema,
//! points its pool's `search_path` at it, and drops it at the end.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pw@localhost:55432/postgres \
//!   cargo test -p brocanth-analytics
//! ```
//!
//! Without a URL, each test says `!! SKIPPED` on the real stderr and passes.

use std::sync::atomic::{AtomicU32, Ordering};

use brocanth_analytics::{Day, Dimension, PgStore, Store, View};
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig};

fn url() -> Option<String> {
    [
        "BROCANTH_ANALYTICS_TEST_POSTGRES",
        "BROCANTH_TEST_DATABASE_URL",
    ]
    .iter()
    .find_map(|v| std::env::var(v).ok())
    .filter(|u| !u.trim().is_empty())
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
            "brocanth_analytics_t{}_{}",
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

fn view(day: i32, visitor: i64, path: &str, referrer: &str) -> View {
    View {
        site: "site".to_string(),
        day: Day(day),
        visitor,
        path: path.to_string(),
        query: String::new(),
        referrer: referrer.to_string(),
        browser: "Firefox".to_string(),
        os: "Linux".to_string(),
        device: "desktop".to_string(),
        country: String::new(),
    }
}

#[tokio::test]
async fn a_salt_is_made_once_per_day_and_read_back() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let first = sandbox.store.salt(Day(20_000)).await.unwrap();
    assert_eq!(sandbox.store.salt(Day(20_000)).await.unwrap(), first);
    assert_ne!(sandbox.store.salt(Day(20_001)).await.unwrap(), first);
    sandbox.drop().await;
}

#[tokio::test]
async fn views_are_written_and_reported_by_day_and_dimension() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let views = vec![
        view(20_000, 1, "/", "https://news.example"),
        view(20_000, 1, "/pricing", ""),
        view(20_000, 2, "/", ""),
        view(20_001, 1, "/", "https://news.example"),
    ];
    sandbox.store.write(&views).await.unwrap();
    let report = sandbox
        .store
        .report("site", Day(20_000), Day(20_001))
        .await
        .unwrap();
    assert_eq!(report.views, 4);
    assert_eq!(report.visitors, 3);
    assert_eq!(report.days, vec![(Day(20_000), 3, 2), (Day(20_001), 1, 1)]);
    let pages = report.breakdown(Dimension::Path);
    assert_eq!(pages[0].label, "/");
    assert_eq!((pages[0].views, pages[0].visitors), (3, 3));
    let referrers = report.breakdown(Dimension::Referrer);
    assert_eq!(referrers.len(), 1, "an empty referrer is not a line");
    assert_eq!(referrers[0].views, 2);
    assert!(report.breakdown(Dimension::Country).is_empty());
    // Another site's rows are not this site's numbers.
    let other = sandbox
        .store
        .report("other", Day(0), Day(30_000))
        .await
        .unwrap();
    assert_eq!(other.views, 0);
    sandbox.drop().await;
}

#[tokio::test]
async fn forget_deletes_old_salts_and_rows_past_the_retention() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    sandbox.store.salt(Day(19_990)).await.unwrap();
    let today_salt = sandbox.store.salt(Day(20_000)).await.unwrap();
    sandbox
        .store
        .write(&[view(19_960, 1, "/old", ""), view(19_999, 1, "/new", "")])
        .await
        .unwrap();
    sandbox
        .store
        .forget(Day(20_000), Some(Day(19_970)))
        .await
        .unwrap();
    let (salts,): (i64,) = sqlx::query_as("SELECT count(*) FROM brocanth_analytics_salts")
        .fetch_one(sandbox.db.pool())
        .await
        .unwrap();
    assert_eq!(salts, 1, "only today's salt is left");
    assert_eq!(sandbox.store.salt(Day(20_000)).await.unwrap(), today_salt);
    let report = sandbox
        .store
        .report("site", Day(0), Day(30_000))
        .await
        .unwrap();
    assert_eq!(report.views, 1);
    assert_eq!(report.breakdown(Dimension::Path)[0].label, "/new");
    sandbox.drop().await;
}

#[tokio::test]
async fn no_column_can_hold_an_address_or_a_user_agent() {
    let Some(sandbox) = Sandbox::new().await else {
        return;
    };
    let columns: Vec<(String,)> = sqlx::query_as(
        "SELECT column_name::text FROM information_schema.columns \
         WHERE table_name = 'brocanth_analytics_views' AND table_schema = current_schema() \
         ORDER BY ordinal_position",
    )
    .fetch_all(sandbox.db.pool())
    .await
    .unwrap();
    let names: Vec<String> = columns.into_iter().map(|(c,)| c).collect();
    assert_eq!(
        names,
        [
            "id", "site", "day", "visitor", "path", "query", "referrer", "browser", "os", "device",
            "country"
        ]
    );
    sandbox.drop().await;
}
