//! A real PostgreSQL database behind [`brocanth_policy::agreement::Oracle`].
//!
//! The harness asks the caller to run the query because this crate has no
//! driver and will not grow one. A test that answered from a second SQL
//! implementation written here would prove only that two of our own guesses
//! agree, so the answer comes from the engine.
//!
//! Each [`Postgres`] creates a schema of its own, points its pool's
//! `search_path` at it, and drops it when it drops. Tests run in parallel and
//! never share a table.
//!
//! ```sh
//! BROCANTH_TEST_DATABASE_URL=postgres://postgres:pw@localhost:5432/postgres \
//!   cargo test -p brocanth-policy
//! ```
//!
//! Without a URL, [`Postgres::new`] prints `!! SKIPPED` on the real stderr and
//! returns `None`, and the test returns.

#![allow(dead_code)]

use std::str::FromStr as _;
use std::sync::atomic::{AtomicU32, Ordering};

use brocanth_policy::agreement::Oracle;
use brocanth_policy::{Entity, Query, Value};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::postgres::{
    PgArgumentBuffer, PgConnectOptions, PgPool, PgPoolOptions, PgTypeInfo, Postgres as Pg,
};
use sqlx::AssertSqlSafe;
use sqlx::Row as _;

/// The variable that names the test database.
pub const URL: &str = "BROCANTH_TEST_DATABASE_URL";

pub struct Postgres {
    pool: PgPool,
    schema: String,
    runtime: tokio::runtime::Runtime,
}

impl Postgres {
    /// A fresh schema with `schema_sql` applied, or `None` when no database is
    /// set. `None` has already printed the skip line.
    pub fn new(schema_sql: &str) -> Option<Self> {
        let Some(url) = std::env::var(URL).ok().filter(|u| !u.trim().is_empty()) else {
            skipped(&format!("set {URL} to run it against PostgreSQL"));
            return None;
        };
        static N: AtomicU32 = AtomicU32::new(0);
        let schema = format!(
            "brocanth_policy_t{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let pool = runtime.block_on(async {
            // `search_path` may name a schema that does not exist yet, so the
            // pool can create its own schema and then use it.
            let options = PgConnectOptions::from_str(&url)
                .expect("a PostgreSQL URL")
                .options([("search_path", schema.as_str())]);
            let pool = PgPoolOptions::new()
                .max_connections(2)
                .connect_with(options)
                .await
                .unwrap_or_else(|e| panic!("connect to {URL}: {e}"));
            sqlx::query(AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
                .execute(&pool)
                .await
                .expect("the schema is created");
            for statement in schema_sql.split(';').filter(|s| !s.trim().is_empty()) {
                // `AssertSqlSafe` is sqlx's own version of the rule this crate
                // enforces with `&'static str`: SQL text is program text, and
                // a borrowed one has to be vouched for.
                sqlx::query(AssertSqlSafe(statement.to_owned()))
                    .execute(&pool)
                    .await
                    .expect("the schema applies");
            }
            pool
        });
        Some(Postgres {
            pool,
            schema,
            runtime,
        })
    }

    /// Run a statement with binds, for fixtures.
    pub fn execute(&self, sql: &str, binds: &[Value]) -> u64 {
        self.runtime.block_on(async {
            bind_all(sqlx::query(AssertSqlSafe(sql.to_owned())), binds)
                .execute(&self.pool)
                .await
                .unwrap_or_else(|e| panic!("{sql}: {e}"))
                .rows_affected()
        })
    }
}

impl Drop for Postgres {
    fn drop(&mut self) {
        let sql = format!("DROP SCHEMA IF EXISTS {} CASCADE", self.schema);
        self.runtime.block_on(async {
            let _ = sqlx::query(AssertSqlSafe(sql)).execute(&self.pool).await;
            self.pool.close().await;
        });
    }
}

/// Print a skip line that cargo's output capture cannot swallow.
pub fn skipped(reason: &str) {
    use std::io::Write as _;
    let name = std::thread::current().name().unwrap_or("a test").to_owned();
    let line = format!("!! SKIPPED {name}: {reason}\n");
    if let Ok(mut fd2) = std::fs::OpenOptions::new().write(true).open("/dev/stderr") {
        if fd2.write_all(line.as_bytes()).is_ok() {
            return;
        }
    }
    eprint!("{line}");
}

/// A NULL with no type of its own.
///
/// `Option::<i64>::None` binds as `int8`, and PostgreSQL then refuses
/// `text_column = $1`. OID 0 leaves the type unspecified, so the server infers
/// it from the column, as it does for a literal `NULL`.
struct UntypedNull;

impl sqlx::Type<Pg> for UntypedNull {
    fn type_info() -> PgTypeInfo {
        PgTypeInfo::with_oid(sqlx::postgres::types::Oid(0))
    }
}

impl sqlx::Encode<'_, Pg> for UntypedNull {
    fn encode_by_ref(&self, _buf: &mut PgArgumentBuffer) -> Result<IsNull, BoxDynError> {
        Ok(IsNull::Yes)
    }
}

type PgQuery = sqlx::query::Query<'static, Pg, sqlx::postgres::PgArguments>;

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

impl<E: Entity> Oracle<E> for Postgres {
    fn matching(&mut self, query: &Query) -> Result<Vec<Value>, String> {
        self.runtime.block_on(async {
            let rows = bind_all(
                sqlx::query(AssertSqlSafe(query.sql().to_owned())),
                query.binds(),
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| format!("{e} -- {}", query.sql()))?;
            rows.iter()
                .map(|row| {
                    // A `BIGINT` key reads as `i64`. The admission query's
                    // `SELECT 1` is an `int4`.
                    row.try_get::<i64, _>(0)
                        .or_else(|_| row.try_get::<i32, _>(0).map(i64::from))
                        .map(Value::Int)
                        .map_err(|e| e.to_string())
                })
                .collect()
        })
    }
}
