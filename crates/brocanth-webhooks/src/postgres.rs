//! [`PgSeen`]: idempotency keys in the database the `db` module opened.
//!
//! One table, keyed by `(source, id)`. A claim is one `INSERT … ON CONFLICT
//! DO NOTHING`, which the primary key makes atomic across every instance:
//! two copies of one delivery that reach two servers at once get one row
//! and one `true`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use brocanth_db::sqlx;
use brocanth_db::Db;

use crate::seen::{Seen, SeenError};

/// Idempotency keys in Postgres.
#[derive(Debug)]
pub struct PgSeen {
    db: Db,
    claims: AtomicU64,
}

impl PgSeen {
    /// The table, idempotent.
    pub const SCHEMA: &'static str = "\
CREATE TABLE IF NOT EXISTS brocanth_webhooks_seen (
    source  text        NOT NULL,
    id      text        NOT NULL,
    expires timestamptz NOT NULL,
    PRIMARY KEY (source, id)
);
CREATE INDEX IF NOT EXISTS brocanth_webhooks_seen_expires
    ON brocanth_webhooks_seen (expires)";

    /// A store over `db`. Creates nothing: see [`PgSeen::migrate`].
    pub fn new(db: Db) -> Self {
        Self {
            db,
            claims: AtomicU64::new(0),
        }
    }

    /// Runs [`PgSeen::SCHEMA`].
    ///
    /// # Errors
    /// The database refused a statement.
    pub async fn migrate(&self) -> Result<(), SeenError> {
        run_schema(&self.db, Self::SCHEMA).await.map_err(failed)
    }
}

/// Runs each statement of a schema, in order.
pub(crate) async fn run_schema(db: &Db, schema: &str) -> Result<(), sqlx::Error> {
    for statement in schema.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        sqlx::query(sqlx::AssertSqlSafe(statement.to_string()))
            .execute(db.pool())
            .await?;
    }
    Ok(())
}

fn failed(e: sqlx::Error) -> SeenError {
    SeenError(e.to_string())
}

#[async_trait]
impl Seen for PgSeen {
    async fn claim(&self, source: &str, id: &str, keep: Duration) -> Result<bool, SeenError> {
        // Every thousandth claim drops the expired keys, so the table stays
        // the size of one window without a timer.
        if self
            .claims
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(1_000)
        {
            sqlx::query("DELETE FROM brocanth_webhooks_seen WHERE expires < now()")
                .execute(self.db.pool())
                .await
                .map_err(failed)?;
        }
        let seconds = i64::try_from(keep.as_secs()).unwrap_or(i64::MAX);
        // An expired row is taken over in the same statement: the update
        // runs only when the held claim has run out.
        let claimed = sqlx::query(
            "INSERT INTO brocanth_webhooks_seen (source, id, expires) \
             VALUES ($1, $2, now() + make_interval(secs => $3)) \
             ON CONFLICT (source, id) DO UPDATE SET expires = EXCLUDED.expires \
             WHERE brocanth_webhooks_seen.expires < now()",
        )
        .bind(source)
        .bind(id)
        .bind(seconds as f64)
        .execute(self.db.pool())
        .await
        .map_err(failed)?;
        Ok(claimed.rows_affected() == 1)
    }

    async fn release(&self, source: &str, id: &str) -> Result<(), SeenError> {
        sqlx::query("DELETE FROM brocanth_webhooks_seen WHERE source = $1 AND id = $2")
            .bind(source)
            .bind(id)
            .execute(self.db.pool())
            .await
            .map_err(failed)?;
        Ok(())
    }
}
