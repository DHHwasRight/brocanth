//! Where recurring schedules are kept: the one table this crate still owns.
//!
//! Everything about a job once it is enqueued is the engine's. A schedule is
//! not a job: it is a row that says when to enqueue one. So it stays here,
//! in the table it always had (`brocanth_job_schedules`), and the memory
//! backend keeps the same rows in a map.
//!
//! Times are epoch milliseconds from the queue's [`Clock`](crate::Clock),
//! as before, so a test can drive a schedule with a manual clock.
//!
//! # Two tickers, one job
//!
//! Every worker ticks the schedules. Two of them can read the same due row.
//! Two things keep that to one job:
//!
//! * the job carries the unique key `"<schedule>@<occurrence millis>"`,
//!   held for a day by the engine whatever happens to the job, so a second
//!   enqueue of one occurrence is a duplicate even after the first one ran;
//! * the advance is a compare-and-set on `next_run_at`, so only one ticker
//!   moves the schedule and counts what it missed.

#[cfg(feature = "memory")]
use std::collections::BTreeMap;
#[cfg(feature = "memory")]
use std::sync::Mutex;

use crate::error::Result;

/// One schedule, as a ticker reads it.
#[derive(Clone, Debug)]
pub(crate) struct ScheduleRow {
    pub(crate) name: String,
    pub(crate) cron: String,
    pub(crate) job_name: String,
    pub(crate) queue: String,
    pub(crate) payload: String,
    /// Kept in the table for the first queue's rows; the memory store
    /// never reads it.
    #[cfg_attr(not(feature = "postgres"), allow(dead_code))]
    pub(crate) priority: i64,
    pub(crate) max_attempts: i64,
    pub(crate) next_run_at: i64,
    pub(crate) misfire: String,
    pub(crate) grace_ms: i64,
}

/// The schedule store of one queue.
pub(crate) enum Schedules {
    /// `brocanth_job_schedules`, in the queue's database.
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    /// A map, for the memory backend. Gone at restart, like its jobs.
    #[cfg(feature = "memory")]
    Memory(Mutex<BTreeMap<String, MemoryRow>>),
}

/// A schedule in the memory store, with the two counters the table keeps.
#[cfg(feature = "memory")]
#[derive(Clone, Debug)]
pub(crate) struct MemoryRow {
    row: ScheduleRow,
    last_enqueued_at: Option<i64>,
    missed: i64,
}

/// The advisory lock the schedule table's migration holds: the eight ASCII
/// bytes of `brocjobs`, read as a big-endian `i64`, so the row in
/// `pg_locks` reads back as text.
#[cfg(feature = "postgres")]
const MIGRATION_LOCK: i64 = i64::from_be_bytes(*b"brocjobs");

/// The table, unchanged from the first queue, so an existing schedule keeps
/// its next run across the move to the engine.
#[cfg(feature = "postgres")]
pub(crate) const SCHEMA: &str = r#"CREATE TABLE IF NOT EXISTS brocanth_job_schedules (
        name             TEXT   PRIMARY KEY,
        cron             TEXT   NOT NULL,
        job_name         TEXT   NOT NULL,
        queue            TEXT   NOT NULL,
        payload          TEXT   NOT NULL,
        priority         BIGINT NOT NULL,
        max_attempts     BIGINT NOT NULL,
        next_run_at      BIGINT NOT NULL,
        last_enqueued_at BIGINT,
        missed           BIGINT NOT NULL DEFAULT 0,
        misfire          TEXT   NOT NULL,
        grace_ms         BIGINT NOT NULL
    )"#;

/// Re-installing a schedule whose cron is unchanged keeps its `next_run_at`,
/// so restarting the application does not push every recurring job back by a
/// whole period. A changed cron resets it, because the old next time was
/// computed from an expression nobody means any more.
#[cfg(feature = "postgres")]
const UPSERT: &str = r#"
INSERT INTO brocanth_job_schedules
    (name, cron, job_name, queue, payload, priority, max_attempts, next_run_at, misfire, grace_ms, missed)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 0)
ON CONFLICT (name) DO UPDATE SET
    cron = excluded.cron,
    job_name = excluded.job_name,
    queue = excluded.queue,
    payload = excluded.payload,
    priority = excluded.priority,
    max_attempts = excluded.max_attempts,
    misfire = excluded.misfire,
    grace_ms = excluded.grace_ms,
    next_run_at = CASE WHEN brocanth_job_schedules.cron = excluded.cron
                       THEN brocanth_job_schedules.next_run_at
                       ELSE excluded.next_run_at END"#;

#[cfg(feature = "postgres")]
const DUE: &str = "SELECT name, cron, job_name, queue, payload, priority, max_attempts, \
                   next_run_at, misfire, grace_ms FROM brocanth_job_schedules \
                   WHERE next_run_at <= $1 ORDER BY name";

/// The compare-and-set: only the ticker that read `next_run_at = $5` moves it.
#[cfg(feature = "postgres")]
const ADVANCE: &str = r#"
UPDATE brocanth_job_schedules
SET next_run_at = $1,
    last_enqueued_at = COALESCE($2, last_enqueued_at),
    missed = missed + $3
WHERE name = $4 AND next_run_at = $5"#;

#[cfg(feature = "postgres")]
const DELETE: &str = "DELETE FROM brocanth_job_schedules WHERE name = $1";

impl Schedules {
    /// Creates the table. The memory store has nothing to create, and
    /// [`Queue::migrate`](crate::Queue::migrate) does not call this for it.
    #[cfg(feature = "postgres")]
    pub(crate) async fn migrate(&self) -> Result<()> {
        match self {
            Schedules::Postgres(pool) => {
                // Under an advisory lock, in a transaction: two sessions
                // that both run `CREATE TABLE IF NOT EXISTS` on an empty
                // schema both create, and the loser fails.
                let mut tx = pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock($1)")
                    .bind(MIGRATION_LOCK)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(SCHEMA).execute(&mut *tx).await?;
                tx.commit().await?;
                Ok(())
            }
            #[cfg(feature = "memory")]
            Schedules::Memory(_) => Ok(()),
        }
    }

    /// Writes a schedule, or updates one with the same name.
    pub(crate) async fn upsert(&self, row: ScheduleRow) -> Result<()> {
        match self {
            #[cfg(feature = "postgres")]
            Schedules::Postgres(pool) => {
                sqlx::query(UPSERT)
                    .bind(&row.name)
                    .bind(&row.cron)
                    .bind(&row.job_name)
                    .bind(&row.queue)
                    .bind(&row.payload)
                    .bind(row.priority)
                    .bind(row.max_attempts)
                    .bind(row.next_run_at)
                    .bind(&row.misfire)
                    .bind(row.grace_ms)
                    .execute(pool)
                    .await?;
                Ok(())
            }
            #[cfg(feature = "memory")]
            Schedules::Memory(map) => {
                let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
                match map.get_mut(&row.name) {
                    Some(old) => {
                        let next = if old.row.cron == row.cron {
                            old.row.next_run_at
                        } else {
                            row.next_run_at
                        };
                        old.row = ScheduleRow {
                            next_run_at: next,
                            ..row
                        };
                    }
                    None => {
                        map.insert(
                            row.name.clone(),
                            MemoryRow {
                                row,
                                last_enqueued_at: None,
                                missed: 0,
                            },
                        );
                    }
                }
                Ok(())
            }
        }
    }

    /// Removes a schedule. Answers how many rows went.
    pub(crate) async fn remove(&self, name: &str) -> Result<u64> {
        match self {
            #[cfg(feature = "postgres")]
            Schedules::Postgres(pool) => Ok(sqlx::query(DELETE)
                .bind(name)
                .execute(pool)
                .await?
                .rows_affected()),
            #[cfg(feature = "memory")]
            Schedules::Memory(map) => {
                let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
                Ok(u64::from(map.remove(name).is_some()))
            }
        }
    }

    /// Every schedule whose next run is at or before `now`, by name.
    pub(crate) async fn due(&self, now: i64) -> Result<Vec<ScheduleRow>> {
        match self {
            #[cfg(feature = "postgres")]
            Schedules::Postgres(pool) => {
                use sqlx::Row;
                let rows = sqlx::query(DUE).bind(now).fetch_all(pool).await?;
                Ok(rows
                    .into_iter()
                    .map(|r| ScheduleRow {
                        name: r.get("name"),
                        cron: r.get("cron"),
                        job_name: r.get("job_name"),
                        queue: r.get("queue"),
                        payload: r.get("payload"),
                        priority: r.get("priority"),
                        max_attempts: r.get("max_attempts"),
                        next_run_at: r.get("next_run_at"),
                        misfire: r.get("misfire"),
                        grace_ms: r.get("grace_ms"),
                    })
                    .collect())
            }
            #[cfg(feature = "memory")]
            Schedules::Memory(map) => {
                let map = map.lock().unwrap_or_else(|e| e.into_inner());
                Ok(map
                    .values()
                    .filter(|m| m.row.next_run_at <= now)
                    .map(|m| m.row.clone())
                    .collect())
            }
        }
    }

    /// Moves a schedule from `from` to `to`, if it is still at `from`.
    /// `false` means another ticker moved it first.
    pub(crate) async fn advance(
        &self,
        name: &str,
        from: i64,
        to: i64,
        fired_at: Option<i64>,
        missed: i64,
    ) -> Result<bool> {
        match self {
            #[cfg(feature = "postgres")]
            Schedules::Postgres(pool) => Ok(sqlx::query(ADVANCE)
                .bind(to)
                .bind(fired_at)
                .bind(missed)
                .bind(name)
                .bind(from)
                .execute(pool)
                .await?
                .rows_affected()
                == 1),
            #[cfg(feature = "memory")]
            Schedules::Memory(map) => {
                let mut map = map.lock().unwrap_or_else(|e| e.into_inner());
                match map.get_mut(name) {
                    Some(m) if m.row.next_run_at == from => {
                        m.row.next_run_at = to;
                        if fired_at.is_some() {
                            m.last_enqueued_at = fired_at;
                        }
                        m.missed += missed;
                        Ok(true)
                    }
                    _ => Ok(false),
                }
            }
        }
    }
}
