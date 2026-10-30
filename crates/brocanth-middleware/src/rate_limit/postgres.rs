//! The counters in Postgres, shared by every process.
//!
//! One `UNLOGGED` table, one row per key:
//!
//! ```sql
//! CREATE UNLOGGED TABLE brocanth_rate_limits (
//!     key        text   PRIMARY KEY,
//!     window_end bigint NOT NULL,
//!     hits       bigint NOT NULL
//! );
//! ```
//!
//! `UNLOGGED` because a counter is not worth a write-ahead log entry. A
//! crash empties the table, and every caller starts a fresh window: that
//! is the right failure for a rate limit.
//!
//! One statement per count. It inserts the key, or adds one to it when
//! the stored window is still the current one, or starts it again at 1
//! when the window has ended. The window comes from the database's
//! clock, not the application's, so two processes with a skewed clock
//! still agree on where a window starts:
//!
//! ```sql
//! INSERT INTO brocanth_rate_limits AS r (key, window_end, hits)
//! VALUES ($1, (floor(extract(epoch FROM now()))::bigint / $2 + 1) * $2, 1)
//! ON CONFLICT (key) DO UPDATE SET
//!     hits = CASE WHEN r.window_end = EXCLUDED.window_end THEN r.hits + 1 ELSE 1 END,
//!     window_end = EXCLUDED.window_end
//! RETURNING hits, window_end - floor(extract(epoch FROM now()))::bigint
//! ```
//!
//! The row lock that `ON CONFLICT DO UPDATE` takes is the whole of the
//! concurrency story: two counts of one key in the same instant run one
//! after the other, and neither is lost.
//!
//! An ended window stays in the table until its key comes back or
//! [`PgStore::sweep`] deletes it. Run the sweep from a job, every few
//! minutes.

use std::time::Duration;

use brocanth::plugin_routes::BoxFuture;
use sqlx::PgPool;

use super::{Hit, Store, StoreError};

/// The default table name.
pub const TABLE: &str = "brocanth_rate_limits";

/// Rate limit counters in one `UNLOGGED` Postgres table.
#[derive(Clone)]
pub struct PgStore {
    pool: PgPool,
    table: String,
    hit_sql: String,
}

impl PgStore {
    /// A store on this pool, in the table `brocanth_rate_limits`.
    pub fn new(pool: PgPool) -> Self {
        Self::build(pool, TABLE.to_string())
    }

    /// The same store in another table, optionally schema-qualified:
    /// `limits` or `app.limits`.
    ///
    /// # Errors
    ///
    /// A name that is not letters, digits and `_`, in at most two parts.
    /// It goes into SQL as written, so nothing else is accepted.
    pub fn in_table(pool: PgPool, table: impl Into<String>) -> Result<Self, StoreError> {
        let table = table.into();
        let parts: Vec<&str> = table.split('.').collect();
        let ok = parts.len() <= 2
            && parts.iter().all(|p| {
                !p.is_empty()
                    && !p.starts_with(|c: char| c.is_ascii_digit())
                    && p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            });
        if !ok {
            return Err(StoreError(format!(
                "`{table}` is not a table name this store writes into SQL. help: use \
                 letters, digits and `_`, as `limits` or `app.limits`"
            )));
        }
        Ok(Self::build(pool, table))
    }

    /// Every statement here is `AssertSqlSafe` for one reason: the only
    /// text spliced in is the table name, and [`PgStore::in_table`]
    /// refuses any name that is not an identifier. Keys and windows are
    /// bound parameters.
    fn build(pool: PgPool, table: String) -> Self {
        let hit_sql = format!(
            "INSERT INTO {table} AS r (key, window_end, hits) \
             VALUES ($1, (floor(extract(epoch FROM now()))::bigint / $2 + 1) * $2, 1) \
             ON CONFLICT (key) DO UPDATE SET \
                 hits = CASE WHEN r.window_end = EXCLUDED.window_end THEN r.hits + 1 ELSE 1 END, \
                 window_end = EXCLUDED.window_end \
             RETURNING hits, window_end - floor(extract(epoch FROM now()))::bigint"
        );
        Self {
            pool,
            table,
            hit_sql,
        }
    }

    /// The `CREATE` statement, for a migration.
    pub fn create_sql(&self) -> String {
        format!(
            "CREATE UNLOGGED TABLE IF NOT EXISTS {} (\
                 key text PRIMARY KEY, \
                 window_end bigint NOT NULL, \
                 hits bigint NOT NULL)",
            self.table
        )
    }

    /// Create the table if it is not there.
    ///
    /// # Errors
    ///
    /// The database's error.
    pub async fn create_table(&self) -> Result<(), StoreError> {
        sqlx::query(sqlx::AssertSqlSafe(self.create_sql()))
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(failed)
    }

    /// Delete every window that has ended. Returns how many.
    ///
    /// # Errors
    ///
    /// The database's error.
    pub async fn sweep(&self) -> Result<u64, StoreError> {
        let sql = format!(
            "DELETE FROM {} WHERE window_end <= floor(extract(epoch FROM now()))::bigint",
            self.table
        );
        sqlx::query(sqlx::AssertSqlSafe(sql))
            .execute(&self.pool)
            .await
            .map(|done| done.rows_affected())
            .map_err(failed)
    }
}

fn failed(e: sqlx::Error) -> StoreError {
    StoreError(format!("postgres: {e}"))
}

impl Store for PgStore {
    fn hit<'a>(&'a self, key: &'a str, window: Duration) -> BoxFuture<'a, Result<Hit, StoreError>> {
        Box::pin(async move {
            let size = i64::try_from(window.as_secs().max(1))
                .map_err(|_| StoreError("the window does not fit a bigint".into()))?;
            let (hits, left): (i64, i64) =
                sqlx::query_as(sqlx::AssertSqlSafe(self.hit_sql.as_str()))
                    .bind(key)
                    .bind(size)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(failed)?;
            Ok(Hit {
                count: u64::try_from(hits).unwrap_or(0),
                resets_in: Duration::from_secs(u64::try_from(left).unwrap_or(0)),
            })
        })
    }
}
