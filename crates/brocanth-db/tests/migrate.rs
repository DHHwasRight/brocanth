//! SQL migrations against a real Postgres. Each test makes its own schema,
//! points its pool's `search_path` at it, and drops it at the end, so the
//! `brocanth_migrations` table of one test is not the table of another.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pw@localhost:55432/postgres \
//!   cargo test -p brocanth-db --test migrate
//! ```
//!
//! Without a URL, each test says `!! SKIPPED` on the real stderr and passes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use brocanth_db::migrate::{read_dir, MigrateError, Migrator, State};
use brocanth_db::{Db, DbConfig};

fn url() -> Option<String> {
    ["BROCANTH_DB_TEST_POSTGRES", "BROCANTH_TEST_DATABASE_URL"]
        .iter()
        .find_map(|v| std::env::var(v).ok())
        .filter(|u| !u.trim().is_empty())
}

fn skipped() {
    use std::io::Write as _;
    let name = std::thread::current().name().unwrap_or("a test").to_owned();
    let _ = std::io::stderr().write_all(
        format!(
            "!! SKIPPED {name}: set BROCANTH_TEST_DATABASE_URL (or BROCANTH_DB_TEST_POSTGRES)\n"
        )
        .as_bytes(),
    );
}

/// One schema and one migrations directory per test.
struct Sandbox {
    admin: Db,
    schema: String,
    db: Db,
    dir: PathBuf,
}

impl Sandbox {
    async fn new() -> Option<Sandbox> {
        static N: AtomicU32 = AtomicU32::new(0);
        let Some(url) = url() else {
            skipped();
            return None;
        };
        let n = N.fetch_add(1, Ordering::Relaxed);
        let schema = format!("brocanth_migrate_t{}_{n}", std::process::id());
        let admin = Db::connect(&DbConfig::new(&url)).await.expect("connect");
        exec(&admin, &format!("CREATE SCHEMA {schema}")).await;
        let mut cfg = DbConfig::new(&url);
        cfg.max_connections = 2;
        cfg.search_path = vec![schema.clone()];
        let db = Db::connect(&cfg).await.expect("connect");
        let dir = std::env::temp_dir().join(format!("{schema}_files"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Some(Sandbox {
            admin,
            schema,
            db,
            dir,
        })
    }

    fn write(&self, name: &str, text: &str) {
        std::fs::write(self.dir.join(format!("{name}.sql")), text).unwrap();
    }

    fn migrator(&self) -> Migrator<'_> {
        Migrator::new(&self.db, read_dir(&self.dir).expect("read the files"))
    }

    async fn tables(&self) -> Vec<String> {
        sqlx::query_scalar::<_, String>(
            "SELECT table_name::text FROM information_schema.tables \
             WHERE table_schema = $1 ORDER BY table_name",
        )
        .bind(&self.schema)
        .fetch_all(self.admin.pool())
        .await
        .unwrap()
    }

    async fn drop(self) {
        self.db.close().await;
        exec(&self.admin, &format!("DROP SCHEMA {} CASCADE", self.schema)).await;
        self.admin.close().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn exec(db: &Db, sql: &str) {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(db.pool())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

const POSTS: &str = "CREATE TABLE posts (id bigserial PRIMARY KEY, title text NOT NULL);\n\
                     CREATE INDEX posts_title ON posts (title);\n\
                     \n-- down\n\
                     DROP TABLE posts;\n";

const TAGS: &str = "CREATE TABLE tags (name text PRIMARY KEY);\n\n-- down\nDROP TABLE tags;\n";

#[tokio::test]
async fn up_applies_each_file_once_in_name_order_and_down_undoes_the_newest() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    sb.write("20261105_100000_add_posts", POSTS);
    sb.write("20261105_110000_add_tags", TAGS);

    let applied = sb.migrator().up().await.unwrap();
    assert_eq!(
        applied,
        ["20261105_100000_add_posts", "20261105_110000_add_tags"]
    );
    assert_eq!(sb.tables().await, ["brocanth_migrations", "posts", "tags"]);
    assert!(
        sb.migrator().up().await.unwrap().is_empty(),
        "applied twice"
    );

    let status = sb.migrator().status().await.unwrap();
    assert!(
        status.iter().all(|(_, s)| *s == State::Applied),
        "{status:?}"
    );

    let undone = sb.migrator().down(1).await.unwrap();
    assert_eq!(undone, ["20261105_110000_add_tags"]);
    assert_eq!(sb.tables().await, ["brocanth_migrations", "posts"]);
    let status = sb.migrator().status().await.unwrap();
    assert_eq!(
        status,
        [
            ("20261105_100000_add_posts".to_string(), State::Applied),
            ("20261105_110000_add_tags".to_string(), State::Pending),
        ]
    );
    sb.drop().await;
}

#[tokio::test]
async fn a_failed_file_leaves_nothing_and_stops_the_run() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    sb.write("20261105_100000_add_posts", POSTS);
    // The first statement works; the second fails. The transaction keeps
    // neither, and the third file is not reached.
    sb.write(
        "20261105_110000_broken",
        "CREATE TABLE half (id int);\nSELECT * FROM no_such_table;\n",
    );
    sb.write("20261105_120000_add_tags", TAGS);

    let error = sb.migrator().up().await.unwrap_err();
    assert!(
        matches!(&error, MigrateError::Failed { name, .. } if name == "20261105_110000_broken"),
        "{error}"
    );
    assert!(error.to_string().contains("no_such_table"), "{error}");
    assert_eq!(sb.tables().await, ["brocanth_migrations", "posts"]);
    let status = sb.migrator().status().await.unwrap();
    assert_eq!(status[1].1, State::Pending);
    assert_eq!(status[2].1, State::Pending);
    sb.drop().await;
}

#[tokio::test]
async fn down_refuses_before_any_change_when_one_step_cannot_be_undone() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    sb.write("20261105_100000_add_posts", POSTS);
    sb.write(
        "20261105_110000_seed",
        "INSERT INTO posts (title) VALUES ('hello');\n",
    );
    sb.write("20261105_120000_add_tags", TAGS);
    sb.migrator().up().await.unwrap();

    let error = sb.migrator().down(2).await.unwrap_err();
    assert!(matches!(error, MigrateError::Irreversible(_)), "{error}");
    assert!(error.to_string().contains("help:"), "{error}");
    // `tags` was the first of the two to undo, and it is still there.
    assert_eq!(sb.tables().await, ["brocanth_migrations", "posts", "tags"]);

    // A file deleted after it was applied is reported, and blocks `down`.
    std::fs::remove_file(sb.dir.join("20261105_120000_add_tags.sql")).unwrap();
    let status = sb.migrator().status().await.unwrap();
    assert_eq!(
        status.last().unwrap(),
        &("20261105_120000_add_tags".to_string(), State::Missing)
    );
    let error = sb.migrator().down(1).await.unwrap_err();
    assert!(matches!(error, MigrateError::NoFile(_)), "{error}");
    sb.drop().await;
}

#[tokio::test]
async fn a_new_file_is_pending_and_applies_as_written() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let path = brocanth_db::migrate::new_file(&sb.dir, "add_notes").unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let filled = text.replacen(
        "-- down\n",
        "CREATE TABLE notes (id int);\n\n-- down\nDROP TABLE notes;\n",
        1,
    );
    std::fs::write(&path, filled).unwrap();
    let name = Path::new(&path)
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        sb.migrator().status().await.unwrap(),
        [(name.clone(), State::Pending)]
    );
    assert_eq!(
        sb.migrator().up().await.unwrap(),
        std::slice::from_ref(&name)
    );
    assert_eq!(sb.tables().await, ["brocanth_migrations", "notes"]);
    assert_eq!(sb.migrator().down(5).await.unwrap(), [name]);
    assert_eq!(sb.tables().await, ["brocanth_migrations"]);
    sb.drop().await;
}
