//! The move from the first queue's tables to the engine's.
//!
//! Before `brocanth-queue` was the engine, this crate kept its jobs in
//! `brocanth_jobs` and its dead letter in `brocanth_dead_jobs`, with times in
//! epoch milliseconds. [`move_rows`] moves what those tables hold into
//! `queue_bulk` and `queue_dead`, in one transaction, and runs from
//! [`Queue::migrate`](crate::Queue::migrate) on every boot.
//!
//! The old tables stay, empty. A worker of the old version that still runs
//! during a rolling deploy keeps writing to them, and the next boot moves
//! what it wrote. Drop them by hand once no old worker is left.

use sqlx::PgPool;

use crate::error::Result;

/// Every waiting or running job. A running row becomes ready at once: its
/// worker ran the old code. Its attempts stay spent, so a row that had none
/// left is not claimed and the sweep buries it, as the old reclaim did. A
/// unique key that `queue_bulk` already holds is not written twice.
const MOVE_JOBS: &str = r#"
WITH moved AS (
    DELETE FROM brocanth_jobs RETURNING *
)
INSERT INTO queue_bulk
    (kind, queue, payload, available_at, attempts, max_attempts,
     unique_key, uniqueness, enqueued_at, last_error)
SELECT job_name,
       queue,
       convert_to(payload, 'UTF8'),
       CASE WHEN state = 'running' THEN now()
            ELSE to_timestamp(run_at / 1000.0) END,
       attempts::integer,
       max_attempts::integer,
       unique_key,
       CASE WHEN unique_key IS NULL THEN 'none' ELSE 'until_success' END,
       to_timestamp(created_at / 1000.0),
       last_error
  FROM moved
ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING
"#;

/// Every dead job, with its failure kind, message and backtrace in the one
/// `failure` column the engine keeps.
const MOVE_DEAD: &str = r#"
WITH moved AS (
    DELETE FROM brocanth_dead_jobs RETURNING *
)
INSERT INTO queue_dead
    (job_id, kind, queue, payload, attempts, max_attempts, unique_key,
     failure, enqueued_at, died_at)
SELECT job_id,
       job_name,
       queue,
       convert_to(payload, 'UTF8'),
       attempts::integer,
       max_attempts::integer,
       unique_key,
       failure_kind || ': ' || error
           || COALESCE(E'\n\nbacktrace:\n' || backtrace, ''),
       to_timestamp(enqueued_at / 1000.0),
       to_timestamp(failed_at / 1000.0)
  FROM moved
"#;

/// Moves the old tables' rows, if the old tables exist. Answers how many
/// jobs and how many dead jobs moved.
pub(crate) async fn move_rows(pool: &PgPool) -> Result<(u64, u64)> {
    let (jobs, dead): (bool, bool) = sqlx::query_as(
        "SELECT to_regclass('brocanth_jobs') IS NOT NULL, \
                to_regclass('brocanth_dead_jobs') IS NOT NULL",
    )
    .fetch_one(pool)
    .await?;
    if !jobs && !dead {
        return Ok((0, 0));
    }
    let mut tx = pool.begin().await?;
    let moved_jobs = if jobs {
        sqlx::query(MOVE_JOBS)
            .execute(&mut *tx)
            .await?
            .rows_affected()
    } else {
        0
    };
    let moved_dead = if dead {
        sqlx::query(MOVE_DEAD)
            .execute(&mut *tx)
            .await?
            .rows_affected()
    } else {
        0
    };
    tx.commit().await?;
    Ok((moved_jobs, moved_dead))
}
