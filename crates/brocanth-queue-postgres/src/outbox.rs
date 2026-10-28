//! The outbox, read side.

use std::time::Duration;

use brocanth_queue::{NewJob, Outbox, OutboxError, OutboxRow, UniquenessKind};
use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};

fn oops(e: sqlx::Error) -> OutboxError {
    match &e {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::PoolClosed => {
            OutboxError::Unavailable(e.to_string())
        }
        _ => OutboxError::Other(e.to_string()),
    }
}

fn uniqueness(text: &str, _key: Option<&str>) -> UniquenessKind {
    match text {
        "until_success" => UniquenessKind::UntilSuccess,
        // A window's length is not kept on the row: the window was opened in
        // `queue_dedup` at the enqueue, which is where it belongs, because
        // it has to outlive both the row and the relay.
        "window" => UniquenessKind::Window(0),
        _ => UniquenessKind::None,
    }
}

/// The outbox in Postgres.
#[derive(Debug, Clone)]
pub struct PgOutbox {
    pool: PgPool,
}

impl PgOutbox {
    /// Read the outbox through this pool. It should be the control pool.
    pub fn new(pool: PgPool) -> Self {
        PgOutbox { pool }
    }
}

impl Outbox for PgOutbox {
    async fn fetch(&self, limit: usize, lease: Duration) -> Result<Vec<OutboxRow>, OutboxError> {
        // The same claim as the queue, for the same reason: several relays
        // may run, and `SKIP LOCKED` is what keeps them from serialising
        // behind each other. `available_at` carries the relay's lease, so a
        // relay that dies strands nothing.
        let rows = sqlx::query(
            "WITH picked AS ( \
                 SELECT id FROM queue_outbox \
                  WHERE available_at <= now() \
                  ORDER BY id \
                  FOR UPDATE SKIP LOCKED \
                  LIMIT $1 \
             ) \
             UPDATE queue_outbox o \
                SET available_at = now() + make_interval(secs => $2), \
                    attempts = o.attempts + 1 \
               FROM picked \
              WHERE o.id = picked.id \
          RETURNING o.id, o.kind, o.queue, o.payload, o.run_at, o.max_attempts, \
                    o.unique_key, o.uniqueness, o.attempts",
        )
        .bind(limit as i64)
        .bind(lease.as_secs_f64())
        .fetch_all(&self.pool)
        .await
        .map_err(oops)?;

        Ok(rows
            .into_iter()
            .map(|r| {
                let key: Option<String> = r.get("unique_key");
                let text: String = r.get("uniqueness");
                OutboxRow {
                    id: r.get::<i64, _>("id").to_string(),
                    attempts: r.get::<i32, _>("attempts") as u32,
                    job: NewJob {
                        kind: r.get("kind"),
                        queue: r.get("queue"),
                        payload: r.get("payload"),
                        run_at: r
                            .get::<Option<DateTime<Utc>>, _>("run_at")
                            .map(std::time::SystemTime::from),
                        max_attempts: r.get::<i32, _>("max_attempts") as u32,
                        uniqueness: uniqueness(&text, key.as_deref()),
                        unique_key: key,
                    },
                }
            })
            .collect())
    }

    async fn delete(&self, ids: &[String]) -> Result<(), OutboxError> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = ids.iter().filter_map(|s| s.parse().ok()).collect();
        sqlx::query("DELETE FROM queue_outbox WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&self.pool)
            .await
            .map_err(oops)?;
        Ok(())
    }

    async fn defer(
        &self,
        ids: &[String],
        not_before: Duration,
        error: &str,
    ) -> Result<(), OutboxError> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = ids.iter().filter_map(|s| s.parse().ok()).collect();
        sqlx::query(
            "UPDATE queue_outbox \
                SET available_at = now() + make_interval(secs => $2), last_error = $3 \
              WHERE id = ANY($1)",
        )
        .bind(&ids)
        .bind(not_before.as_secs_f64())
        .bind(error)
        .execute(&self.pool)
        .await
        .map_err(oops)?;
        Ok(())
    }

    async fn pending(&self) -> Result<u64, OutboxError> {
        let row = sqlx::query("SELECT count(*) AS n FROM queue_outbox")
            .fetch_one(&self.pool)
            .await
            .map_err(oops)?;
        Ok(row.get::<i64, _>("n") as u64)
    }
}
