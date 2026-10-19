//! Every statement this crate runs, written out in both dialects.
//!
//! Written out, rather than generated from one template by swapping `$1` for
//! `?`: the two dialects differ in more than their placeholders — `BIGSERIAL`
//! against `INTEGER PRIMARY KEY AUTOINCREMENT`, `= ANY($3)` against a built
//! `IN (?, ?, ?)`, `FOR UPDATE SKIP LOCKED` against nothing at all — and a
//! template that papers over the placeholders while hiding those is worse than
//! two honest strings. The duplication is the point: both halves are readable
//! SQL that can be pasted into `psql` or `sqlite3` when something is wrong.
//!
//! Postgres statements repeat `$1` where a value is needed twice. SQLite ones
//! use plain `?` and bind the value twice, because a repeated `?NNN` makes the
//! parameter count depend on a driver detail and this is not the place to be
//! clever.

#![allow(dead_code)]

use super::Dialect;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// The three tables, in Postgres.
#[cfg(feature = "postgres")]
pub(crate) const SCHEMA_PG: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS brocanth_jobs (
        id           BIGSERIAL PRIMARY KEY,
        queue        TEXT   NOT NULL,
        job_name     TEXT   NOT NULL,
        payload      TEXT   NOT NULL,
        priority     BIGINT NOT NULL DEFAULT 0,
        state        TEXT   NOT NULL DEFAULT 'pending',
        attempts     BIGINT NOT NULL DEFAULT 0,
        max_attempts BIGINT NOT NULL,
        run_at       BIGINT NOT NULL,
        created_at   BIGINT NOT NULL,
        unique_key   TEXT,
        locked_by    TEXT,
        locked_at    BIGINT,
        heartbeat_at BIGINT,
        last_error   TEXT
    )"#,
    // The claim's index. Partial on `state`, because a queue that is doing its
    // job is almost entirely `running` and `pending` rows are the few that get
    // looked at.
    r#"CREATE INDEX IF NOT EXISTS brocanth_jobs_claim
       ON brocanth_jobs (queue, priority, run_at, id) WHERE state = 'pending'"#,
    // Uniqueness, enforced by the database rather than by a check-then-insert
    // that races with itself.
    r#"CREATE UNIQUE INDEX IF NOT EXISTS brocanth_jobs_unique_key
       ON brocanth_jobs (unique_key) WHERE unique_key IS NOT NULL"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_jobs_heartbeat
       ON brocanth_jobs (heartbeat_at) WHERE state = 'running'"#,
    r#"CREATE TABLE IF NOT EXISTS brocanth_dead_jobs (
        id           BIGSERIAL PRIMARY KEY,
        job_id       BIGINT NOT NULL,
        queue        TEXT   NOT NULL,
        job_name     TEXT   NOT NULL,
        payload      TEXT   NOT NULL,
        priority     BIGINT NOT NULL,
        attempts     BIGINT NOT NULL,
        max_attempts BIGINT NOT NULL,
        unique_key   TEXT,
        error        TEXT   NOT NULL,
        failure_kind TEXT   NOT NULL,
        backtrace    TEXT,
        enqueued_at  BIGINT NOT NULL,
        failed_at    BIGINT NOT NULL
    )"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_dead_jobs_failed_at
       ON brocanth_dead_jobs (failed_at DESC, id DESC)"#,
    r#"CREATE TABLE IF NOT EXISTS brocanth_job_schedules (
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
    )"#,
];

/// The same three tables, in SQLite.
#[cfg(feature = "sqlite")]
pub(crate) const SCHEMA_SQLITE: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS brocanth_jobs (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        queue        TEXT    NOT NULL,
        job_name     TEXT    NOT NULL,
        payload      TEXT    NOT NULL,
        priority     INTEGER NOT NULL DEFAULT 0,
        state        TEXT    NOT NULL DEFAULT 'pending',
        attempts     INTEGER NOT NULL DEFAULT 0,
        max_attempts INTEGER NOT NULL,
        run_at       INTEGER NOT NULL,
        created_at   INTEGER NOT NULL,
        unique_key   TEXT,
        locked_by    TEXT,
        locked_at    INTEGER,
        heartbeat_at INTEGER,
        last_error   TEXT
    )"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_jobs_claim
       ON brocanth_jobs (queue, priority, run_at, id) WHERE state = 'pending'"#,
    r#"CREATE UNIQUE INDEX IF NOT EXISTS brocanth_jobs_unique_key
       ON brocanth_jobs (unique_key) WHERE unique_key IS NOT NULL"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_jobs_heartbeat
       ON brocanth_jobs (heartbeat_at) WHERE state = 'running'"#,
    r#"CREATE TABLE IF NOT EXISTS brocanth_dead_jobs (
        id           INTEGER PRIMARY KEY AUTOINCREMENT,
        job_id       INTEGER NOT NULL,
        queue        TEXT    NOT NULL,
        job_name     TEXT    NOT NULL,
        payload      TEXT    NOT NULL,
        priority     INTEGER NOT NULL,
        attempts     INTEGER NOT NULL,
        max_attempts INTEGER NOT NULL,
        unique_key   TEXT,
        error        TEXT    NOT NULL,
        failure_kind TEXT    NOT NULL,
        backtrace    TEXT,
        enqueued_at  INTEGER NOT NULL,
        failed_at    INTEGER NOT NULL
    )"#,
    r#"CREATE INDEX IF NOT EXISTS brocanth_dead_jobs_failed_at
       ON brocanth_dead_jobs (failed_at DESC, id DESC)"#,
    r#"CREATE TABLE IF NOT EXISTS brocanth_job_schedules (
        name             TEXT    PRIMARY KEY,
        cron             TEXT    NOT NULL,
        job_name         TEXT    NOT NULL,
        queue            TEXT    NOT NULL,
        payload          TEXT    NOT NULL,
        priority         INTEGER NOT NULL,
        max_attempts     INTEGER NOT NULL,
        next_run_at      INTEGER NOT NULL,
        last_enqueued_at INTEGER,
        missed           INTEGER NOT NULL DEFAULT 0,
        misfire          TEXT    NOT NULL,
        grace_ms         INTEGER NOT NULL
    )"#,
];

// ---------------------------------------------------------------------------
// Enqueue
// ---------------------------------------------------------------------------

/// The channel every worker listens on. One channel and not one per queue:
/// a worker claims from a set of queues, `LISTEN` takes a literal name, and
/// a worker on eight queues would otherwise hold eight subscriptions to say
/// the same thing. The queue name rides as the payload, so a listener that
/// cares can filter without a second channel.
pub(crate) const CHANNEL: &str = "brocanth_jobs";

/// The notify is in the statement and not a second round trip.
///
/// `pg_notify` inside the insert is delivered when the insert commits, so a
/// job is never announced before it is visible, and an enqueue that rolls
/// back announces nothing. A separate `NOTIFY` after the insert would be two
/// chances to get that wrong.
///
/// It fires whatever `run_at` says, including for a job scheduled an hour
/// out. That is deliberate: a worker sleeps until the soonest job is due
/// (see [`NEXT_RUN_AT_PG`]), so a job that lands sooner than that deadline
/// has to wake it up to shorten the sleep.
#[cfg(feature = "postgres")]
pub(crate) const INSERT_PG: &str = r#"
WITH new_job AS (
    INSERT INTO brocanth_jobs (queue, job_name, payload, priority, max_attempts, run_at, created_at, state, attempts)
    VALUES ($1, $2, $3, $4, $5, $6, $7, 'pending', 0)
    RETURNING id, queue
)
SELECT id, pg_notify('brocanth_jobs', queue) FROM new_job"#;

#[cfg(feature = "sqlite")]
pub(crate) const INSERT_SQLITE: &str = r#"
INSERT INTO brocanth_jobs (queue, job_name, payload, priority, max_attempts, run_at, created_at, state, attempts)
VALUES (?, ?, ?, ?, ?, ?, ?, 'pending', 0)
RETURNING id"#;

/// The same, and the conflict case keeps working: `DO NOTHING` leaves the
/// CTE empty, so the outer select returns no row and nothing is announced.
/// A duplicate enqueue wakes nobody, which is what a duplicate should cost.
#[cfg(feature = "postgres")]
pub(crate) const INSERT_UNIQUE_PG: &str = r#"
WITH new_job AS (
    INSERT INTO brocanth_jobs (queue, job_name, payload, priority, max_attempts, run_at, created_at, unique_key, state, attempts)
    VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', 0)
    ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING
    RETURNING id, queue
)
SELECT id, pg_notify('brocanth_jobs', queue) FROM new_job"#;

#[cfg(feature = "sqlite")]
pub(crate) const INSERT_UNIQUE_SQLITE: &str = r#"
INSERT INTO brocanth_jobs (queue, job_name, payload, priority, max_attempts, run_at, created_at, unique_key, state, attempts)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', 0)
ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING
RETURNING id"#;

#[cfg(feature = "postgres")]
pub(crate) const FIND_BY_KEY_PG: &str =
    "SELECT id FROM brocanth_jobs WHERE unique_key = $1 LIMIT 1";

#[cfg(feature = "sqlite")]
pub(crate) const FIND_BY_KEY_SQLITE: &str =
    "SELECT id FROM brocanth_jobs WHERE unique_key = ? LIMIT 1";

// ---------------------------------------------------------------------------
// When to wake up
// ---------------------------------------------------------------------------

/// The soonest a pending job on these queues becomes runnable.
///
/// This is what turns the poll into a backstop. A worker that finds nothing
/// asks when there will be something and sleeps until then, so a job
/// scheduled for 09:00 is claimed at 09:00 rather than at the next tick
/// after it. Without this the safety poll would also be the latency floor
/// for every retry and every scheduled job.
///
/// It reads the claim index -- same columns, same partial predicate -- so it
/// costs an index scan that stops at the first row.
///
/// `$1` queues.
#[cfg(feature = "postgres")]
pub(crate) const NEXT_RUN_AT_PG: &str =
    "SELECT min(run_at) FROM brocanth_jobs WHERE state = 'pending' AND queue = ANY($1)";

/// The same, with the queue list expanded into placeholders by the caller:
/// SQLite has no array type to bind.
#[cfg(feature = "sqlite")]
pub(crate) fn next_run_at_sqlite(queues: usize) -> String {
    let marks = vec!["?"; queues.max(1)].join(", ");
    format!("SELECT min(run_at) FROM brocanth_jobs WHERE state = 'pending' AND queue IN ({marks})")
}

// ---------------------------------------------------------------------------
// Claim
// ---------------------------------------------------------------------------

/// `$1` worker, `$2` now, `$3` queues, `$4` limit.
#[cfg(feature = "postgres")]
pub(crate) const CLAIM_PG: &str = r#"
UPDATE brocanth_jobs AS j
SET state = 'running',
    attempts = j.attempts + 1,
    locked_by = $1,
    locked_at = $2,
    heartbeat_at = $2,
    last_error = NULL
WHERE j.id IN (
    SELECT id FROM brocanth_jobs
    WHERE state = 'pending' AND run_at <= $2 AND queue = ANY($3)
    ORDER BY priority ASC, run_at ASC, id ASC
    LIMIT $4
    FOR UPDATE SKIP LOCKED
)
RETURNING j.id, j.queue, j.job_name, j.payload, j.attempts, j.max_attempts, j.created_at"#;

/// The same claim for SQLite, with one `?` per queue name.
///
/// Built rather than constant because SQLite has no array type, so the `IN`
/// list has to be the right length. Called inside `BEGIN IMMEDIATE`; see the
/// module documentation on [`crate::store`] for why that matters.
#[cfg(feature = "sqlite")]
pub(crate) fn claim_sqlite(queue_count: usize) -> String {
    let placeholders = std::iter::repeat_n("?", queue_count)
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        r#"
UPDATE brocanth_jobs
SET state = 'running',
    attempts = attempts + 1,
    locked_by = ?,
    locked_at = ?,
    heartbeat_at = ?,
    last_error = NULL
WHERE id IN (
    SELECT id FROM brocanth_jobs
    WHERE state = 'pending' AND run_at <= ? AND queue IN ({placeholders})
    ORDER BY priority ASC, run_at ASC, id ASC
    LIMIT ?
)
RETURNING id, queue, job_name, payload, attempts, max_attempts, created_at"#
    )
}

// ---------------------------------------------------------------------------
// The end of an attempt
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
pub(crate) const COMPLETE_PG: &str = "DELETE FROM brocanth_jobs WHERE id = $1 AND locked_by = $2";
#[cfg(feature = "sqlite")]
pub(crate) const COMPLETE_SQLITE: &str = "DELETE FROM brocanth_jobs WHERE id = ? AND locked_by = ?";

#[cfg(feature = "postgres")]
pub(crate) const RETRY_PG: &str = r#"
UPDATE brocanth_jobs
SET state = 'pending', run_at = $1, last_error = $2,
    locked_by = NULL, locked_at = NULL, heartbeat_at = NULL
WHERE id = $3 AND locked_by = $4"#;

#[cfg(feature = "sqlite")]
pub(crate) const RETRY_SQLITE: &str = r#"
UPDATE brocanth_jobs
SET state = 'pending', run_at = ?, last_error = ?,
    locked_by = NULL, locked_at = NULL, heartbeat_at = NULL
WHERE id = ? AND locked_by = ?"#;

/// `$1` message, `$2` kind, `$3` backtrace, `$4` now, `$5` id.
#[cfg(feature = "postgres")]
const DEAD_INSERT_PG: &str = r#"
INSERT INTO brocanth_dead_jobs
    (job_id, queue, job_name, payload, priority, attempts, max_attempts, unique_key,
     error, failure_kind, backtrace, enqueued_at, failed_at)
SELECT id, queue, job_name, payload, priority, attempts, max_attempts, unique_key,
       $1, $2, $3, created_at, $4
FROM brocanth_jobs WHERE id = $5"#;

#[cfg(feature = "sqlite")]
const DEAD_INSERT_SQLITE: &str = r#"
INSERT INTO brocanth_dead_jobs
    (job_id, queue, job_name, payload, priority, attempts, max_attempts, unique_key,
     error, failure_kind, backtrace, enqueued_at, failed_at)
SELECT id, queue, job_name, payload, priority, attempts, max_attempts, unique_key,
       ?, ?, ?, created_at, ?
FROM brocanth_jobs WHERE id = ?"#;

pub(crate) fn dead_insert(dialect: Dialect) -> &'static str {
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => DEAD_INSERT_PG,
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => DEAD_INSERT_SQLITE,
    }
}

pub(crate) fn delete_one(dialect: Dialect) -> &'static str {
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => "DELETE FROM brocanth_jobs WHERE id = $1",
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => "DELETE FROM brocanth_jobs WHERE id = ?",
    }
}

#[cfg(feature = "postgres")]
pub(crate) const RELEASE_PG: &str = r#"
UPDATE brocanth_jobs
SET state = 'pending', run_at = $1,
    attempts = CASE WHEN attempts > 0 THEN attempts - 1 ELSE 0 END,
    locked_by = NULL, locked_at = NULL, heartbeat_at = NULL,
    last_error = 'returned to the queue by a worker shutting down'
WHERE locked_by = $2 AND state = 'running'"#;

#[cfg(feature = "sqlite")]
pub(crate) const RELEASE_SQLITE: &str = r#"
UPDATE brocanth_jobs
SET state = 'pending', run_at = ?,
    attempts = CASE WHEN attempts > 0 THEN attempts - 1 ELSE 0 END,
    locked_by = NULL, locked_at = NULL, heartbeat_at = NULL,
    last_error = 'returned to the queue by a worker shutting down'
WHERE locked_by = ? AND state = 'running'"#;

#[cfg(feature = "postgres")]
pub(crate) const HEARTBEAT_PG: &str =
    "UPDATE brocanth_jobs SET heartbeat_at = $1 WHERE locked_by = $2 AND state = 'running'";
#[cfg(feature = "sqlite")]
pub(crate) const HEARTBEAT_SQLITE: &str =
    "UPDATE brocanth_jobs SET heartbeat_at = ? WHERE locked_by = ? AND state = 'running'";

// ---------------------------------------------------------------------------
// Orphans
// ---------------------------------------------------------------------------

/// `$1` message, `$2` kind, `$3` now, `$4` stale_before.
#[cfg(feature = "postgres")]
const ORPHAN_DEAD_PG: &str = r#"
INSERT INTO brocanth_dead_jobs
    (job_id, queue, job_name, payload, priority, attempts, max_attempts, unique_key,
     error, failure_kind, backtrace, enqueued_at, failed_at)
SELECT id, queue, job_name, payload, priority, attempts, max_attempts, unique_key,
       $1, $2, NULL, created_at, $3
FROM brocanth_jobs
WHERE state = 'running' AND heartbeat_at IS NOT NULL AND heartbeat_at < $4
  AND attempts >= max_attempts"#;

#[cfg(feature = "sqlite")]
const ORPHAN_DEAD_SQLITE: &str = r#"
INSERT INTO brocanth_dead_jobs
    (job_id, queue, job_name, payload, priority, attempts, max_attempts, unique_key,
     error, failure_kind, backtrace, enqueued_at, failed_at)
SELECT id, queue, job_name, payload, priority, attempts, max_attempts, unique_key,
       ?, ?, NULL, created_at, ?
FROM brocanth_jobs
WHERE state = 'running' AND heartbeat_at IS NOT NULL AND heartbeat_at < ?
  AND attempts >= max_attempts"#;

pub(crate) fn orphan_dead_insert(dialect: Dialect) -> &'static str {
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => ORPHAN_DEAD_PG,
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => ORPHAN_DEAD_SQLITE,
    }
}

pub(crate) fn orphan_delete(dialect: Dialect) -> &'static str {
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => {
            "DELETE FROM brocanth_jobs WHERE state = 'running' AND heartbeat_at IS NOT NULL \
             AND heartbeat_at < $1 AND attempts >= max_attempts"
        }
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => {
            "DELETE FROM brocanth_jobs WHERE state = 'running' AND heartbeat_at IS NOT NULL \
             AND heartbeat_at < ? AND attempts >= max_attempts"
        }
    }
}

/// Put every orphan back, and say so on the way out.
///
/// `RETURNING queue` rather than `rows_affected`, because the caller needs
/// both the count and, on Postgres, a notification per row. Putting
/// `pg_notify` in the `RETURNING` list is what keeps this one statement: it
/// runs once per row the update already locked, so it adds no lock and
/// cannot deadlock against a concurrent claim. A separate `SELECT ... FROM
/// brocanth_jobs` in the same transaction can, and did.
///
/// The caller reads column 0 in both dialects, so the extra column on
/// Postgres costs the SQLite path nothing.
pub(crate) fn orphan_requeue(dialect: Dialect) -> &'static str {
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => {
            "UPDATE brocanth_jobs SET state = 'pending', run_at = $1, last_error = $2, \
             locked_by = NULL, locked_at = NULL, heartbeat_at = NULL \
             WHERE state = 'running' AND heartbeat_at IS NOT NULL AND heartbeat_at < $3 \
             RETURNING queue, pg_notify('brocanth_jobs', queue)"
        }
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => {
            "UPDATE brocanth_jobs SET state = 'pending', run_at = ?, last_error = ?, \
             locked_by = NULL, locked_at = NULL, heartbeat_at = NULL \
             WHERE state = 'running' AND heartbeat_at IS NOT NULL AND heartbeat_at < ? \
             RETURNING queue"
        }
    }
}

// ---------------------------------------------------------------------------
// Looking at what is waiting
// ---------------------------------------------------------------------------
//
// Three reads an operator wants and a worker never does. They are here and
// not on the claim path, so none of them takes a lock or orders by anything
// the claim index cares about.
//
// Retries and scheduled jobs split on `attempts`, which is the whole reason
// this table needs no fourth state. A job waiting for its first run has
// attempts = 0; a job waiting after a failure has more. Sidekiq needs two
// sorted sets to tell those apart because Redis has no columns to ask.

/// Jobs that ran, failed and are waiting for their backoff. `$1` now,
/// `$2` limit, `$3` offset.
#[cfg(feature = "postgres")]
pub(crate) const RETRIES_PG: &str =
    "SELECT id, queue, job_name, payload, attempts, max_attempts, run_at, created_at, last_error \
     FROM brocanth_jobs WHERE state = 'pending' AND attempts > 0 AND run_at > $1 \
     ORDER BY run_at, id LIMIT $2 OFFSET $3";

#[cfg(feature = "sqlite")]
pub(crate) const RETRIES_SQLITE: &str =
    "SELECT id, queue, job_name, payload, attempts, max_attempts, run_at, created_at, last_error \
     FROM brocanth_jobs WHERE state = 'pending' AND attempts > 0 AND run_at > ? \
     ORDER BY run_at, id LIMIT ? OFFSET ?";

/// Jobs that have never run and are waiting for their time.
#[cfg(feature = "postgres")]
pub(crate) const SCHEDULED_PG: &str =
    "SELECT id, queue, job_name, payload, attempts, max_attempts, run_at, created_at, last_error \
     FROM brocanth_jobs WHERE state = 'pending' AND attempts = 0 AND run_at > $1 \
     ORDER BY run_at, id LIMIT $2 OFFSET $3";

#[cfg(feature = "sqlite")]
pub(crate) const SCHEDULED_SQLITE: &str =
    "SELECT id, queue, job_name, payload, attempts, max_attempts, run_at, created_at, last_error \
     FROM brocanth_jobs WHERE state = 'pending' AND attempts = 0 AND run_at > ? \
     ORDER BY run_at, id LIMIT ? OFFSET ?";

/// What every worker is holding right now, oldest claim first.
///
/// `locked_by` and not a worker table: a worker is only ever known by the
/// jobs it holds, so the jobs are where the answer is. A worker with
/// nothing in flight is invisible here, which is correct -- there is
/// nothing to show about it and nothing an operator can do to it.
#[cfg(feature = "postgres")]
pub(crate) const BUSY_PG: &str =
    "SELECT id, queue, job_name, payload, attempts, max_attempts, locked_by, locked_at, \
     heartbeat_at FROM brocanth_jobs WHERE state = 'running' ORDER BY locked_at, id \
     LIMIT $1 OFFSET $2";

#[cfg(feature = "sqlite")]
pub(crate) const BUSY_SQLITE: &str =
    "SELECT id, queue, job_name, payload, attempts, max_attempts, locked_by, locked_at, \
     heartbeat_at FROM brocanth_jobs WHERE state = 'running' ORDER BY locked_at, id \
     LIMIT ? OFFSET ?";

/// Make a waiting job runnable now. `$1` now, `$2` id.
///
/// Scoped to `state = 'pending'` so the button cannot touch a job a worker
/// is holding: that one is already running, and moving its `run_at` would
/// change nothing and look like it had.
///
/// It announces, through the same `RETURNING` trick the reclaim uses. The
/// point of the button is that the job runs *now*, and a worker asleep
/// until its next due job would otherwise make "now" mean "in five
/// seconds".
#[cfg(feature = "postgres")]
pub(crate) const RUN_NOW_PG: &str =
    "UPDATE brocanth_jobs SET run_at = $1 WHERE id = $2 AND state = 'pending' \
     RETURNING queue, pg_notify('brocanth_jobs', queue)";

#[cfg(feature = "sqlite")]
pub(crate) const RUN_NOW_SQLITE: &str =
    "UPDATE brocanth_jobs SET run_at = ? WHERE id = ? AND state = 'pending' RETURNING queue";

/// Throw a waiting job away. `$1` id.
///
/// Pending only, for the same reason, and no dead-letter row: this is
/// somebody deciding the job should not happen, which is a different event
/// from a job that ran out of attempts. The dead letter is a record of
/// failure and must not fill up with deliberate deletions.
#[cfg(feature = "postgres")]
pub(crate) const DISCARD_PG: &str = "DELETE FROM brocanth_jobs WHERE id = $1 AND state = 'pending'";

#[cfg(feature = "sqlite")]
pub(crate) const DISCARD_SQLITE: &str =
    "DELETE FROM brocanth_jobs WHERE id = ? AND state = 'pending'";

// ---------------------------------------------------------------------------
// The dead letter
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
pub(crate) const DEAD_LIST_PG: &str = "SELECT id, job_id, queue, job_name, payload, attempts, max_attempts, unique_key, error, failure_kind, backtrace, enqueued_at, failed_at FROM brocanth_dead_jobs ORDER BY failed_at DESC, id DESC LIMIT $1 OFFSET $2";
#[cfg(feature = "sqlite")]
pub(crate) const DEAD_LIST_SQLITE: &str = "SELECT id, job_id, queue, job_name, payload, attempts, max_attempts, unique_key, error, failure_kind, backtrace, enqueued_at, failed_at FROM brocanth_dead_jobs ORDER BY failed_at DESC, id DESC LIMIT ? OFFSET ?";

#[cfg(feature = "postgres")]
pub(crate) const DEAD_ONE_PG: &str = "SELECT id, job_id, queue, job_name, payload, attempts, max_attempts, unique_key, error, failure_kind, backtrace, enqueued_at, failed_at FROM brocanth_dead_jobs WHERE id = $1";
#[cfg(feature = "sqlite")]
pub(crate) const DEAD_ONE_SQLITE: &str = "SELECT id, job_id, queue, job_name, payload, attempts, max_attempts, unique_key, error, failure_kind, backtrace, enqueued_at, failed_at FROM brocanth_dead_jobs WHERE id = ?";

#[cfg(feature = "postgres")]
pub(crate) const DEAD_DELETE_PG: &str = "DELETE FROM brocanth_dead_jobs WHERE id = $1";
#[cfg(feature = "sqlite")]
pub(crate) const DEAD_DELETE_SQLITE: &str = "DELETE FROM brocanth_dead_jobs WHERE id = ?";

pub(crate) fn dead_delete(dialect: Dialect) -> &'static str {
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => DEAD_DELETE_PG,
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => DEAD_DELETE_SQLITE,
    }
}

/// `$1` run_at, `$2` created_at, `$3` dead id. Attempts restart at zero: the
/// point of a requeue is that somebody looked at the failure and decided it is
/// worth another five tries.
pub(crate) fn dead_requeue(dialect: Dialect) -> &'static str {
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => {
            r#"
INSERT INTO brocanth_jobs
    (queue, job_name, payload, priority, max_attempts, run_at, created_at, unique_key, state, attempts)
SELECT queue, job_name, payload, priority, max_attempts, $1, $2, unique_key, 'pending', 0
FROM brocanth_dead_jobs WHERE id = $3
ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING
RETURNING id"#
        }
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => {
            r#"
INSERT INTO brocanth_jobs
    (queue, job_name, payload, priority, max_attempts, run_at, created_at, unique_key, state, attempts)
SELECT queue, job_name, payload, priority, max_attempts, ?, ?, unique_key, 'pending', 0
FROM brocanth_dead_jobs WHERE id = ?
ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING
RETURNING id"#
        }
    }
}

// ---------------------------------------------------------------------------
// Observability
// ---------------------------------------------------------------------------

/// Counts and the oldest runnable `run_at`, per queue, in one pass.
#[cfg(feature = "postgres")]
pub(crate) const STATS_PG: &str = r#"
SELECT queue,
       COALESCE(SUM(CASE WHEN state = 'pending' AND run_at <= $1 THEN 1 ELSE 0 END), 0) AS ready,
       COALESCE(SUM(CASE WHEN state = 'pending' AND run_at >  $1 THEN 1 ELSE 0 END), 0) AS scheduled,
       COALESCE(SUM(CASE WHEN state = 'running' THEN 1 ELSE 0 END), 0) AS running,
       MIN(CASE WHEN state = 'pending' AND run_at <= $1 THEN run_at ELSE NULL END) AS oldest_ready
FROM brocanth_jobs
GROUP BY queue
ORDER BY queue"#;

#[cfg(feature = "sqlite")]
pub(crate) const STATS_SQLITE: &str = r#"
SELECT queue,
       COALESCE(SUM(CASE WHEN state = 'pending' AND run_at <= ? THEN 1 ELSE 0 END), 0) AS ready,
       COALESCE(SUM(CASE WHEN state = 'pending' AND run_at >  ? THEN 1 ELSE 0 END), 0) AS scheduled,
       COALESCE(SUM(CASE WHEN state = 'running' THEN 1 ELSE 0 END), 0) AS running,
       MIN(CASE WHEN state = 'pending' AND run_at <= ? THEN run_at ELSE NULL END) AS oldest_ready
FROM brocanth_jobs
GROUP BY queue
ORDER BY queue"#;

pub(crate) const DEAD_COUNT: &str = "SELECT COUNT(*) FROM brocanth_dead_jobs";

#[cfg(feature = "postgres")]
pub(crate) const JOB_STATE_PG: &str =
    "SELECT state, attempts, run_at FROM brocanth_jobs WHERE id = $1";
#[cfg(feature = "sqlite")]
pub(crate) const JOB_STATE_SQLITE: &str =
    "SELECT state, attempts, run_at FROM brocanth_jobs WHERE id = ?";

// ---------------------------------------------------------------------------
// Schedules
// ---------------------------------------------------------------------------

/// Re-installing a schedule whose cron is unchanged keeps its `next_run_at`,
/// so restarting the application does not push every recurring job back by a
/// whole period. A changed cron resets it, because the old next time was
/// computed from an expression nobody means any more.
#[cfg(feature = "postgres")]
pub(crate) const SCHEDULE_UPSERT_PG: &str = r#"
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

#[cfg(feature = "sqlite")]
pub(crate) const SCHEDULE_UPSERT_SQLITE: &str = r#"
INSERT INTO brocanth_job_schedules
    (name, cron, job_name, queue, payload, priority, max_attempts, next_run_at, misfire, grace_ms, missed)
VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)
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
pub(crate) const SCHEDULE_DUE_PG: &str = "SELECT name, cron, job_name, queue, payload, priority, max_attempts, next_run_at, misfire, grace_ms FROM brocanth_job_schedules WHERE next_run_at <= $1 ORDER BY name";
#[cfg(feature = "sqlite")]
pub(crate) const SCHEDULE_DUE_SQLITE: &str = "SELECT name, cron, job_name, queue, payload, priority, max_attempts, next_run_at, misfire, grace_ms FROM brocanth_job_schedules WHERE next_run_at <= ? ORDER BY name";

#[cfg(feature = "postgres")]
pub(crate) const SCHEDULE_ADVANCE_PG: &str = r#"
UPDATE brocanth_job_schedules
SET next_run_at = $1,
    last_enqueued_at = COALESCE($2, last_enqueued_at),
    missed = missed + $3
WHERE name = $4"#;

#[cfg(feature = "sqlite")]
pub(crate) const SCHEDULE_ADVANCE_SQLITE: &str = r#"
UPDATE brocanth_job_schedules
SET next_run_at = ?,
    last_enqueued_at = COALESCE(?, last_enqueued_at),
    missed = missed + ?
WHERE name = ?"#;

#[cfg(feature = "postgres")]
pub(crate) const SCHEDULE_DELETE_PG: &str = "DELETE FROM brocanth_job_schedules WHERE name = $1";
#[cfg(feature = "sqlite")]
pub(crate) const SCHEDULE_DELETE_SQLITE: &str = "DELETE FROM brocanth_job_schedules WHERE name = ?";

#[cfg(test)]
mod tests {
    #[cfg(feature = "sqlite")]
    #[test]
    fn the_sqlite_claim_has_one_placeholder_per_queue() {
        let sql = super::claim_sqlite(3);
        assert!(sql.contains("queue IN (?, ?, ?)"));
        // worker, locked_at, heartbeat_at, run_at, three queues, limit.
        assert_eq!(sql.matches('?').count(), 8);
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn the_sqlite_claim_does_not_claim_to_skip_locked() {
        // SQLite has no row locks, so a copy-paste of the Postgres statement
        // would be a syntax error at best and a lie at worst.
        assert!(!super::claim_sqlite(1).contains("SKIP LOCKED"));
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn the_postgres_claim_skips_locked_rows() {
        assert!(super::CLAIM_PG.contains("FOR UPDATE SKIP LOCKED"));
    }
}
