//! A real PostgreSQL database behind `agreement::Oracle`, for the `posts`
//! table of `tests/agreement.rs`.
//!
//! The harness asks the application for "a database with exactly these
//! rows". This is that function, written the way an application writes it:
//! create a schema of its own, create the table, insert the rows, answer
//! queries, and drop the schema at the end.
//!
//! The database comes from `BROCANTH_TEST_DATABASE_URL`. Without it,
//! [`or_skip`] prints `!! SKIPPED` and hands the harness no subjects, so
//! it checks no rule and asks [`posts`] nothing.

use std::str::FromStr as _;
use std::sync::atomic::{AtomicU32, Ordering};

use brocanth_policy::agreement::Oracle;
use brocanth_policy::{Column, Entity, MapRow, Query, RowView, Value};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{
    PgArgumentBuffer, PgConnectOptions, PgPool, PgPoolOptions, PgTypeInfo, Postgres,
};
use sqlx::{AssertSqlSafe, Row as _};

const URL: &str = "BROCANTH_TEST_DATABASE_URL";

const SCHEMA: &str = "CREATE TABLE posts (
    id        BIGINT PRIMARY KEY,
    author_id BIGINT,
    status    TEXT NOT NULL,
    score     BIGINT NOT NULL
)";

fn url() -> Option<String> {
    std::env::var(URL).ok().filter(|u| !u.trim().is_empty())
}

/// `subjects` when a database is set. Otherwise prints the skip line and
/// returns no subjects, so the harness has no rule to check.
pub fn or_skip<T, const N: usize>(subjects: [T; N]) -> Vec<T> {
    if url().is_some() {
        return subjects.into();
    }
    skipped();
    Vec::new()
}

/// Whether a database is set. Prints the skip line when it is not.
pub fn connected() -> bool {
    let set = url().is_some();
    if !set {
        skipped();
    }
    set
}

fn skipped() {
    use std::io::Write as _;
    let name = std::thread::current().name().unwrap_or("a test").to_owned();
    let line = format!("!! SKIPPED {name}: set {URL} to run it against PostgreSQL\n");
    if let Ok(mut fd2) = std::fs::OpenOptions::new().write(true).open("/dev/stderr") {
        if fd2.write_all(line.as_bytes()).is_ok() {
            return;
        }
    }
    eprint!("{line}");
}

pub struct Posts {
    /// `None` when no database is set: every query then fails.
    db: Option<(PgPool, String)>,
    runtime: tokio::runtime::Runtime,
}

/// A fresh schema whose `posts` table holds exactly `rows`.
pub fn posts<C: Column>(rows: &[MapRow<C>]) -> Posts {
    static N: AtomicU32 = AtomicU32::new(0);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let Some(url) = url() else {
        return Posts { db: None, runtime };
    };
    let schema = format!(
        "brocanth_policy_macros_t{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let pool = runtime.block_on(async {
        let options = PgConnectOptions::from_str(&url)
            .expect("a PostgreSQL URL")
            .options([("search_path", schema.as_str())]);
        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap_or_else(|e| panic!("connect to {URL}: {e}"));
        sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
            .execute(&pool)
            .await
            .expect("the schema is created");
        sqlx::query(SCHEMA)
            .execute(&pool)
            .await
            .expect("the table is created");
        let names: Vec<&str> = C::ALL.iter().map(|c| c.name()).collect();
        let marks: Vec<String> = (1..=names.len()).map(|n| format!("${n}")).collect();
        let insert = format!(
            "INSERT INTO posts ({}) VALUES ({})",
            names.join(", "),
            marks.join(", ")
        );
        for row in rows {
            let values: Vec<Value> = C::ALL.iter().map(|c| row.get(*c)).collect();
            bind_all(sqlx::query(AssertSqlSafe(insert.clone())), &values)
                .execute(&pool)
                .await
                .expect("the row inserts");
        }
        pool
    });
    Posts {
        db: Some((pool, schema)),
        runtime,
    }
}

impl Drop for Posts {
    fn drop(&mut self) {
        if let Some((pool, schema)) = &self.db {
            let sql = format!("DROP SCHEMA IF EXISTS {schema} CASCADE");
            self.runtime.block_on(async {
                let _ = sqlx::query(AssertSqlSafe(sql)).execute(pool).await;
                pool.close().await;
            });
        }
    }
}

/// A NULL with no type of its own. `Option::<i64>::None` binds as `int8`,
/// and PostgreSQL then refuses `text_column = $1`. OID 0 lets the server
/// infer the type from the column.
struct UntypedNull;

impl sqlx::Type<Postgres> for UntypedNull {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, Postgres> for UntypedNull {
    fn encode_by_ref(&self, _buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        Ok(IsNull::Yes)
    }
}

type PgQuery = sqlx::query::Query<'static, Postgres, sqlx::postgres::PgArguments>;

fn bind_all(mut query: PgQuery, binds: &[Value]) -> PgQuery {
    for value in binds {
        query = match value {
            Value::Null => query.bind(UntypedNull),
            Value::Bool(b) => query.bind(*b),
            Value::Int(i) => query.bind(*i),
            Value::Float(f) => query.bind(*f),
            Value::Text(t) => query.bind(t.clone()),
            Value::Bytes(b) => query.bind(b.clone()),
        };
    }
    query
}

impl<E: Entity> Oracle<E> for Posts {
    fn matching(&mut self, query: &Query) -> Result<Vec<Value>, String> {
        let Some((pool, _)) = &self.db else {
            return Err(format!("no database: set {URL}"));
        };
        self.runtime.block_on(async {
            let rows = bind_all(
                sqlx::query(AssertSqlSafe(query.sql().to_owned())),
                query.binds(),
            )
            .fetch_all(pool)
            .await
            .map_err(|e| format!("{e} -- {}", query.sql()))?;
            rows.iter()
                .map(|row| {
                    row.try_get::<i64, _>(0)
                        .map(Value::Int)
                        .map_err(|e| e.to_string())
                })
                .collect()
        })
    }
}
