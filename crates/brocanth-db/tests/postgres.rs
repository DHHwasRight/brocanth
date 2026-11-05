//! Against a real Postgres. Each test makes its own schema, points its pool's
//! `search_path` at it, and drops it at the end. No test touches a shared
//! table, so the tests run in parallel and in parallel with other crates.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pw@localhost:55432/postgres \
//!   cargo test -p brocanth-db
//! ```
//!
//! Without a URL, each test says `!! SKIPPED` on the real stderr and passes.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use brocanth_db::sea_query::{Expr, ExprTrait, Order, Query, Values};
use brocanth_db::{Db, DbConfig, DbError, Exec};

fn url() -> Option<String> {
    ["BROCANTH_DB_TEST_POSTGRES", "BROCANTH_TEST_DATABASE_URL"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|u| !u.trim().is_empty()))
}

/// Writes to the real stderr, which libtest does not capture, so a skip is
/// visible in a passing run.
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

/// One schema per test, with a `posts` table in it.
struct Sandbox {
    admin: Db,
    schema: String,
    url: String,
}

impl Sandbox {
    async fn new() -> Option<Sandbox> {
        static N: AtomicU32 = AtomicU32::new(0);
        let Some(url) = url() else {
            skipped();
            return None;
        };
        let schema = format!(
            "brocanth_db_t{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        let mut cfg = DbConfig::new(&url);
        cfg.max_connections = 2;
        let admin = Db::connect(&cfg).await.expect("connect");
        for stmt in [
            format!("CREATE SCHEMA {schema}"),
            format!(
                "CREATE TABLE {schema}.posts (id bigserial PRIMARY KEY, title text NOT NULL, \
                 score numeric NOT NULL DEFAULT 0, meta jsonb)"
            ),
        ] {
            sqlx_exec(&admin, &stmt).await;
        }
        Some(Sandbox { admin, schema, url })
    }

    fn config(&self) -> DbConfig {
        let mut cfg = DbConfig::new(&self.url);
        cfg.max_connections = 3;
        cfg.search_path = vec![self.schema.clone()];
        cfg
    }

    async fn db(&self) -> Db {
        Db::connect(&self.config()).await.expect("connect")
    }

    async fn drop(self) {
        sqlx_exec(&self.admin, &format!("DROP SCHEMA {} CASCADE", self.schema)).await;
    }
}

async fn sqlx_exec(db: &Db, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(db.pool())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

use brocanth_db::sqlx;

fn insert(title: &str) -> brocanth_db::sea_query::InsertStatement {
    Query::insert()
        .into_table("posts")
        .columns(["title"])
        .values_panic([title.into()])
        .to_owned()
}

fn titles() -> brocanth_db::sea_query::SelectStatement {
    Query::select()
        .column("title")
        .from("posts")
        .order_by("id", Order::Asc)
        .to_owned()
}

async fn count(db: &mut impl Exec) -> i64 {
    let q = Query::select()
        .expr(Expr::cust("count(*)"))
        .from("posts")
        .to_owned();
    let (n,): (i64,) = db.one(&q).await.unwrap();
    n
}

#[tokio::test]
async fn all_one_optional_execute() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let mut db = sb.db().await;

    assert_eq!(db.execute(&insert("a")).await.unwrap(), 1);
    assert_eq!(db.execute(&insert("b")).await.unwrap(), 1);

    let rows: Vec<(String,)> = db.all(&titles()).await.unwrap();
    assert_eq!(rows, [("a".to_owned(),), ("b".to_owned(),)]);

    let returning = Query::insert()
        .into_table("posts")
        .columns(["title"])
        .values_panic(["c".into()])
        .returning_col("id")
        .to_owned();
    let (id,): (i64,) = db.one(&returning).await.unwrap();
    assert_eq!(id, 3);

    let none = Query::select()
        .column("title")
        .from("posts")
        .and_where(Expr::col("id").eq(99))
        .to_owned();
    let missing: Option<(String,)> = db.optional(&none).await.unwrap();
    assert_eq!(missing, None);
    let err = db.one::<(String,)>(&none).await.unwrap_err();
    assert!(err.is_not_found(), "{err}");

    let update = Query::update()
        .table("posts")
        .value("score", 2)
        .and_where(Expr::col("id").lte(2))
        .to_owned();
    assert_eq!(db.execute(&update).await.unwrap(), 2);

    // `&Db` and `&mut Tx` go through the same helper.
    assert_eq!(count(&mut &db).await, 3);

    sb.drop().await;
}

#[tokio::test]
async fn json_lets_postgres_encode() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let mut db = sb.db().await;
    sqlx_exec(
        &db,
        r#"INSERT INTO posts (title, score, meta) VALUES ('a', 1.50, '{"tags":["x"]}'), ('b', 2, NULL)"#,
    )
    .await;

    let q = Query::select()
        .columns(["id", "title", "score", "meta"])
        .from("posts")
        .order_by("id", Order::Desc)
        .to_owned();
    let rows = db.json(&q).await.unwrap();
    assert_eq!(
        rows,
        [
            serde_json::json!({"id": 2, "title": "b", "score": 2, "meta": null}),
            serde_json::json!({"id": 1, "title": "a", "score": 1.50, "meta": {"tags": ["x"]}}),
        ]
    );

    // RETURNING through the CTE.
    let ins = Query::insert()
        .into_table("posts")
        .columns(["title"])
        .values_panic(["c".into()])
        .returning_col("title")
        .to_owned();
    assert_eq!(
        db.json(&ins).await.unwrap(),
        [serde_json::json!({"title": "c"})]
    );

    let raw = db
        .raw_json(
            "SELECT title FROM posts WHERE id = $1;",
            Values(vec![1i64.into()]),
        )
        .await
        .unwrap();
    assert_eq!(raw, [serde_json::json!({"title": "a"})]);

    sb.drop().await;
}

#[tokio::test]
async fn transact_commits_on_ok_and_rolls_back_on_err() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let mut db = sb.db().await;

    let seen = db
        .transact(async |tx| {
            tx.execute(&insert("kept")).await?;
            Ok(count(tx).await)
        })
        .await
        .unwrap();
    assert_eq!(seen, 1);

    let err = db
        .transact(async |tx| -> Result<(), DbError> {
            tx.execute(&insert("lost")).await?;
            assert_eq!(count(tx).await, 2);
            Err(DbError::NotFound)
        })
        .await
        .unwrap_err();
    assert!(err.is_not_found());

    let rows: Vec<(String,)> = db.all(&titles()).await.unwrap();
    assert_eq!(rows, [("kept".to_owned(),)]);

    sb.drop().await;
}

#[tokio::test]
async fn nested_transact_is_a_savepoint() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let mut db = sb.db().await;

    db.transact(async |tx| {
        tx.execute(&insert("outer")).await?;
        let inner = tx
            .transact(async |tx| -> Result<(), DbError> {
                tx.execute(&insert("inner")).await?;
                Err(DbError::NotFound)
            })
            .await;
        assert!(inner.is_err());
        // The outer transaction still works after the savepoint rolled back.
        tx.transact(async |tx| tx.execute(&insert("inner ok")).await)
            .await?;
        Ok(())
    })
    .await
    .unwrap();

    let rows: Vec<(String,)> = db.all(&titles()).await.unwrap();
    assert_eq!(rows, [("outer".to_owned(),), ("inner ok".to_owned(),)]);

    sb.drop().await;
}

#[tokio::test]
async fn a_dropped_tx_rolls_back() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let mut db = sb.db().await;

    {
        let mut tx = db.begin().await.unwrap();
        tx.execute(&insert("dropped")).await.unwrap();
        assert_eq!(count(&mut tx).await, 1);
    }
    assert_eq!(count(&mut db).await, 0);

    let mut tx = db.begin().await.unwrap();
    tx.execute(&insert("committed")).await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(count(&mut db).await, 1);

    sb.drop().await;
}

#[tokio::test]
async fn transact_is_send() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let db = sb.db().await;

    // A handler future must be `Send` to run on a multi-thread runtime.
    let task = tokio::spawn({
        let db = db.clone();
        async move {
            db.transact(async |tx| tx.execute(&insert("spawned")).await)
                .await
        }
    });
    assert_eq!(task.await.unwrap().unwrap(), 1);

    sb.drop().await;
}

#[tokio::test]
async fn search_path_and_application_name_apply_to_every_connection() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let mut cfg = sb.config();
    cfg.application_name = Some("brocanth-db-test".into());
    let db = Db::connect(&cfg).await.unwrap();

    // Several connections, so this is not one lucky connection.
    let mut handles = Vec::new();
    for _ in 0..3 {
        let db = db.clone();
        handles.push(tokio::spawn(async move {
            let mut tx = db.begin().await.unwrap();
            let rows = tx
                .raw_json(
                    "SELECT current_schema() AS schema, \
                     current_setting('application_name') AS app",
                    Values(vec![]),
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(50)).await;
            rows
        }));
    }
    for h in handles {
        let rows = h.await.unwrap();
        assert_eq!(
            rows,
            [serde_json::json!({"schema": sb.schema, "app": "brocanth-db-test"})]
        );
    }

    sb.drop().await;
}

#[tokio::test]
async fn read_only_rejects_a_write() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let mut cfg = sb.config();
    cfg.read_only = true;
    let mut db = Db::connect(&cfg).await.unwrap();

    assert_eq!(count(&mut db).await, 0);
    let err = db.execute(&insert("nope")).await.unwrap_err();
    assert_eq!(err.code(), Some("25006"), "{err}");
    let text = err.to_string();
    assert!(text.contains("read-only"), "{text}");
    assert!(
        text.contains("help: this connection is read-only"),
        "{text}"
    );

    sb.drop().await;
}

#[tokio::test]
async fn statement_timeout_fires() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let mut cfg = sb.config();
    cfg.statement_timeout = Some(Duration::from_millis(100));
    let mut db = Db::connect(&cfg).await.unwrap();

    let err = db
        .raw_json("SELECT pg_sleep(2)", Values(vec![]))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("57014"), "{err}");
    assert!(
        err.to_string().contains("help: the statement ran longer"),
        "{err}"
    );

    sb.drop().await;
}

#[tokio::test]
async fn ping_and_a_bad_password() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let db = sb.db().await;
    db.ping().await.unwrap();

    // Replace the password with a wrong one; the error must not echo it.
    // A URL with no credentials (trust auth) has nothing to check here.
    let Some((head, tail)) = sb.url.split_once('@') else {
        sb.drop().await;
        return;
    };
    let user = head.rsplit_once(':').map(|(u, _)| u).unwrap_or(head);
    let mut cfg = DbConfig::new(format!("{user}:wrong-hunter2@{tail}"));
    cfg.acquire_timeout = Duration::from_secs(5);
    let err = Db::connect(&cfg).await.unwrap_err();
    let text = err.to_string();
    assert!(!text.contains("wrong-hunter2"), "{text}");
    assert!(text.contains("help:"), "{text}");

    sb.drop().await;
}

#[tokio::test]
async fn connect_lazy_opens_on_first_use() {
    let Some(sb) = Sandbox::new().await else {
        return;
    };
    let mut db = Db::connect_lazy(&sb.config()).unwrap();
    assert_eq!(db.pool().size(), 0);
    assert_eq!(count(&mut db).await, 0);
    sb.drop().await;
}
