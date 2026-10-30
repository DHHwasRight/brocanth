//! The outbox, read side.

use std::time::Duration;

use brocanth_queue::{
    DeadId, DeadRow, NewJob, Outbox, OutboxError, OutboxInspect, OutboxRow, Page, UniquenessKind,
};
use chrono::{DateTime, Utc};
// The only dynamic fragment is `DEAD_COLUMNS`, a constant in this file.
use sqlx::{AssertSqlSafe, PgPool, Row};

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

    async fn release(
        &self,
        ids: &[String],
        not_before: Duration,
        error: &str,
    ) -> Result<(), OutboxError> {
        if ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = ids.iter().filter_map(|s| s.parse().ok()).collect();
        // The fetch added one to `attempts`; an outage gives it back.
        sqlx::query(
            "UPDATE queue_outbox \
                SET available_at = now() + make_interval(secs => $2), last_error = $3, \
                    attempts = greatest(attempts - 1, 0) \
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

    async fn bury(&self, id: &str, failure: &str) -> Result<(), OutboxError> {
        let Ok(id) = id.parse::<i64>() else {
            return Ok(());
        };
        // One statement: the row leaves the outbox and enters the dead
        // letter in the same commit, so it is never in both or in neither.
        sqlx::query(
            "WITH gone AS ( \
                 DELETE FROM queue_outbox WHERE id = $1 \
                 RETURNING id, kind, queue, payload, run_at, max_attempts, unique_key, \
                           uniqueness, attempts, enqueued_at \
             ) \
             INSERT INTO queue_outbox_dead \
                 (outbox_id, kind, queue, payload, run_at, max_attempts, unique_key, \
                  uniqueness, attempts, failure, enqueued_at) \
             SELECT id, kind, queue, payload, run_at, max_attempts, unique_key, \
                    uniqueness, attempts, $2, enqueued_at \
               FROM gone",
        )
        .bind(id)
        .bind(failure)
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

fn dead_row(r: &sqlx::postgres::PgRow) -> DeadRow {
    DeadRow {
        id: DeadId(r.get::<i64, _>("id").to_string()),
        // The row never became a job, so it has no job id. `outbox_id` is
        // in the table for a person reading it with SQL.
        job_id: None,
        kind: r.get("kind"),
        queue: r.get("queue"),
        payload: r.get("payload"),
        attempts: r.get::<i32, _>("attempts") as u32,
        max_attempts: Some(r.get::<i32, _>("max_attempts") as u32),
        unique_key: r.get("unique_key"),
        failure: r.get("failure"),
        enqueued_at: r.get::<DateTime<Utc>, _>("enqueued_at").into(),
        died_at: r.get::<DateTime<Utc>, _>("died_at").into(),
    }
}

const DEAD_COLUMNS: &str = "id, kind, queue, payload, attempts, max_attempts, unique_key, \
                            failure, enqueued_at, died_at";

impl OutboxInspect for PgOutbox {
    async fn dead(&self, page: Page) -> Result<Vec<DeadRow>, OutboxError> {
        let limit = page.limit.min(Page::MAX_LIMIT) as i64;
        let sql = format!(
            "SELECT {DEAD_COLUMNS} FROM queue_outbox_dead \
              ORDER BY died_at DESC, id DESC LIMIT $1 OFFSET $2"
        );
        Ok(sqlx::query(AssertSqlSafe(sql))
            .bind(limit)
            .bind(page.offset as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(oops)?
            .iter()
            .map(dead_row)
            .collect())
    }

    async fn dead_one(&self, id: &DeadId) -> Result<Option<DeadRow>, OutboxError> {
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(None);
        };
        let sql = format!("SELECT {DEAD_COLUMNS} FROM queue_outbox_dead WHERE id = $1");
        Ok(sqlx::query(AssertSqlSafe(sql))
            .bind(n)
            .fetch_optional(&self.pool)
            .await
            .map_err(oops)?
            .as_ref()
            .map(dead_row))
    }

    async fn requeue(&self, id: &DeadId) -> Result<Option<String>, OutboxError> {
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(None);
        };
        // One CTE, where the job requeue needs two statements. The job
        // requeue can collide with a live unique key and must then keep the
        // corpse. `queue_outbox` has no unique index, so this insert always
        // writes, and the delete and the insert commit together.
        let new_id: Option<i64> = sqlx::query_scalar(
            "WITH gone AS ( \
                 DELETE FROM queue_outbox_dead WHERE id = $1 \
                 RETURNING kind, queue, payload, run_at, max_attempts, unique_key, uniqueness \
             ) \
             INSERT INTO queue_outbox \
                 (kind, queue, payload, run_at, max_attempts, unique_key, uniqueness) \
             SELECT kind, queue, payload, run_at, max_attempts, unique_key, uniqueness \
               FROM gone \
             RETURNING id",
        )
        .bind(n)
        .fetch_optional(&self.pool)
        .await
        .map_err(oops)?;
        Ok(new_id.map(|id| id.to_string()))
    }

    async fn forget(&self, id: &DeadId) -> Result<bool, OutboxError> {
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(false);
        };
        Ok(sqlx::query("DELETE FROM queue_outbox_dead WHERE id = $1")
            .bind(n)
            .execute(&self.pool)
            .await
            .map_err(oops)?
            .rows_affected()
            > 0)
    }
}
