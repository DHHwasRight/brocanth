//! A real SQLite database behind `agreement::Oracle`, for the `posts` table
//! of `tests/agreement.rs`.
//!
//! The harness asks the application for "a database with exactly these
//! rows". This is that function, written the way an application writes it:
//! create the table, insert the rows, answer queries.

use brocanth_policy::agreement::Oracle;
use brocanth_policy::{Column, Entity, MapRow, Query, RowView, Value};
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};
use sqlx::{AssertSqlSafe, Row as _};

const SCHEMA: &str = "CREATE TABLE posts (
    id        INTEGER PRIMARY KEY,
    author_id INTEGER,
    status    TEXT NOT NULL,
    score     INTEGER NOT NULL
)";

pub struct Sqlite {
    pool: SqlitePool,
    runtime: tokio::runtime::Runtime,
}

/// A fresh in-memory database that holds exactly `rows`.
pub fn posts<C: Column>(rows: &[MapRow<C>]) -> Sqlite {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let pool = runtime.block_on(async {
        // One connection: each `sqlite::memory:` connection is its own
        // database.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("an in-memory database");
        sqlx::query(SCHEMA)
            .execute(&pool)
            .await
            .expect("the schema applies");
        let names: Vec<&str> = C::ALL.iter().map(|c| c.name()).collect();
        let marks = vec!["?"; names.len()].join(", ");
        let insert = format!("INSERT INTO posts ({}) VALUES ({marks})", names.join(", "));
        for row in rows {
            let values: Vec<Value> = C::ALL.iter().map(|c| row.get(*c)).collect();
            bind_all(sqlx::query(AssertSqlSafe(insert.clone())), &values)
                .execute(&pool)
                .await
                .expect("the row inserts");
        }
        pool
    });
    Sqlite { pool, runtime }
}

type SqliteQuery = sqlx::query::Query<'static, sqlx::Sqlite, sqlx::sqlite::SqliteArguments>;

fn bind_all(mut query: SqliteQuery, binds: &[Value]) -> SqliteQuery {
    for value in binds {
        query = match value {
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
