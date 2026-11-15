//! [`PgSeen`]: idempotency keys in the database the `db` module opened.
//!
//! One table, keyed by `(source, id)`. A claim is one `INSERT … ON CONFLICT`,
//! which the primary key makes atomic across every instance: two copies of
//! one delivery that reach two servers at once get one row and one
//! [`Claim::New`]. `done` tells a run in progress from a finished one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use brocanth_db::sqlx;
use brocanth_db::Db;

use crate::seen::{Claim, Seen, SeenError, MAX_KEEP};

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
    done    boolean     NOT NULL DEFAULT true,
    PRIMARY KEY (source, id)
);
ALTER TABLE brocanth_webhooks_seen ADD COLUMN IF NOT EXISTS done boolean NOT NULL DEFAULT true;
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

/// `keep` in seconds, clamped to [`MAX_KEEP`]: `make_interval` refuses a
/// value past its range, which would fail every claim.
fn seconds(keep: Duration) -> f64 {
    keep.min(MAX_KEEP).as_secs_f64()
}

#[async_trait]
impl Seen for PgSeen {
    async fn claim(&self, source: &str, id: &str, lease: Duration) -> Result<Claim, SeenError> {
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
        // An expired row is taken over in the same statement: the update
        // runs only when the held claim has run out.
        let claimed = sqlx::query(
            "INSERT INTO brocanth_webhooks_seen (source, id, expires, done) \
             VALUES ($1, $2, now() + make_interval(secs => $3), false) \
             ON CONFLICT (source, id) DO UPDATE SET expires = EXCLUDED.expires, done = false \
             WHERE brocanth_webhooks_seen.expires < now()",
        )
        .bind(source)
        .bind(id)
        .bind(seconds(lease))
        .execute(self.db.pool())
        .await
        .map_err(failed)?;
        if claimed.rows_affected() == 1 {
            return Ok(Claim::New);
        }
        let done: Option<bool> = sqlx::query_scalar(
            "SELECT done FROM brocanth_webhooks_seen WHERE source = $1 AND id = $2",
        )
        .bind(source)
        .bind(id)
        .fetch_optional(self.db.pool())
        .await
        .map_err(failed)?;
        // No row: it was released between the two statements. The copy is
        // retried, and its retry claims it.
        Ok(if done == Some(true) {
            Claim::Done
        } else {
            Claim::Running
        })
    }

    async fn confirm(&self, source: &str, id: &str, keep: Duration) -> Result<(), SeenError> {
        sqlx::query(
            "UPDATE brocanth_webhooks_seen \
             SET expires = now() + make_interval(secs => $3), done = true \
             WHERE source = $1 AND id = $2",
        )
        .bind(source)
        .bind(id)
        .bind(seconds(keep))
        .execute(self.db.pool())
        .await
        .map_err(failed)?;
        Ok(())
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
