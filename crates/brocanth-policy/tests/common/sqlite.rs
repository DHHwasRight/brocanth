//! A real SQLite database behind [`brocanth_policy::agreement::Oracle`].
//!
//! The harness asks the caller to run the query because this crate has no
//! driver and will not grow one. A test that answered from a second SQL
//! implementation written here would prove only that two of our own guesses
//! agree, so the answer comes from the engine.

#![allow(dead_code)]

use brocanth_policy::agreement::Oracle;
use brocanth_policy::{Entity, Query, Value};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::AssertSqlSafe;
use sqlx::Row as _;

pub struct Sqlite {
    pool: SqlitePool,
    runtime: tokio::runtime::Runtime,
}

impl Sqlite {
    /// An in-memory database with the schema applied.
    ///
    /// One connection, because `sqlite::memory:` gives each connection its own
    /// database and a pool of two would hold two empty halves of the fixture.
    pub fn new(schema: &str) -> Self {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        let pool = runtime.block_on(async {
            let pool = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .expect("an in-memory database");
            for statement in schema.split(';').filter(|s| !s.trim().is_empty()) {
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
        Sqlite { pool, runtime }
    }

    /// Run a statement with binds, for fixtures.
    pub fn execute(&self, sql: &str, binds: &[Value]) -> u64 {
        self.runtime.block_on(async {
            bind_all(sqlx::query(AssertSqlSafe(sql.to_owned())), binds)
                .execute(&self.pool)
                .await
                .expect("the statement runs")
                .rows_affected()
        })
    }
}

type SqliteQuery = sqlx::query::Query<'static, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>;

fn bind_all(mut query: SqliteQuery, binds: &[Value]) -> SqliteQuery {
    for value in binds {
        query = match value {
            // A typed `None`, because an untyped NULL has no binding.
            Value::Null => query.bind(Option::<i64>::None),
            Value::Bool(b) => query.bind(*b),
            Value::Int(i) => query.bind(*i),
            Value::Float(f) => query.bind(*f),
            Value::Text(t) => query.bind(t.clone()),
            Value::Bytes(b) => query.bind(b.clone()),
        };
    }
    query
}

impl<E: Entity> Oracle<E> for Sqlite {
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
                    row.try_get::<i64, _>(0)
                        .map(Value::Int)
                        .map_err(|e| e.to_string())
                })
                .collect()
        })
    }
}
