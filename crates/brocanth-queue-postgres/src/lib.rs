//! The Postgres backend.
//!
//! Postgres can do things SQS and JetStream cannot: join against application
//! tables, hold an unbounded unique key, delete a pending job by id, count
//! exactly. This adapter exposes those through [`Capabilities`] and nowhere
//! else, so an application that depends on one is told at boot that it
//! cannot move, rather than finding out during the migration.
//!
//! The schema is in `schema.sql` and every storage parameter in it is
//! argued for in a comment beside it.

#![forbid(unsafe_code)]

mod inspect;
mod outbox;
mod partitions;
mod tx;

pub use outbox::PgOutbox;
pub use partitions::{drop_archive_partitions_before, ensure_archive_partitions};
pub use tx::Tx;

use std::time::{Duration, SystemTime};

use brocanth_queue::{
    Backend, BackendError, Capabilities, ClaimRequest, Claimed, JobFailure, JobId, LeaseRef,
    LeaseState, NewJob, Pushed, Settle, UniquenessKind, UniquenessSupport,
};
use chrono::{DateTime, Utc};
// Every dynamic fragment in this file is a table name from the closed
// `Table` enum, which returns a `&'static str`. Nothing a caller supplies
// ever reaches a statement as text: the arguments are all bind parameters.
// `AssertSqlSafe` records that audit where sqlx asks for it.
use sqlx::{AssertSqlSafe, PgPool, Row};

/// The DDL. Apply it with [`migrate`].
pub const SCHEMA: &str = include_str!("schema.sql");

/// How many days of archive partitions to keep ahead of today.
///
/// The margin has to be more than one day. A partition is created by the
/// sweep, the sweep runs in a worker, and a worker can be stopped for a
/// deploy at midnight; two spare days means an insert still has somewhere to
/// go after a day with no sweep at all.
const PARTITIONS_AHEAD: i64 = 2;

/// Create the tables if they are not there.
pub async fn migrate(pool: &PgPool) -> Result<(), sqlx::Error> {
    for statement in SCHEMA.split(";\n") {
        let s = statement.trim();
        if s.is_empty() || s.lines().all(|l| l.trim_start().starts_with("--")) {
            continue;
        }
        sqlx::query(s).execute(pool).await?;
    }
    partitions::ensure_archive_partitions(pool, PARTITIONS_AHEAD).await?;
    Ok(())
}

pub(crate) fn err(e: sqlx::Error) -> BackendError {
    match &e {
        sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) | sqlx::Error::PoolClosed => {
            BackendError::Unavailable(e.to_string())
        }
        _ => BackendError::Other(e.to_string()),
    }
}

pub(crate) fn uniqueness_text(u: UniquenessKind) -> &'static str {
    match u {
        UniquenessKind::None => "none",
        UniquenessKind::UntilSuccess => "until_success",
        UniquenessKind::Window(_) => "window",
    }
}

/// How far in the future an instant is, from the caller's own clock.
///
/// Every statement in this adapter writes `now() + make_interval(..)` and
/// never a timestamp the client computed. The two clocks are not the same
/// clock: workers run on different machines, and a server that is a few
/// milliseconds behind makes a job that was enqueued with no delay
/// invisible until the difference passes. The symptom is a queue that is
/// mysteriously slow to pick up new work and perfectly fine under load.
///
/// So a client instant becomes a duration here, and the server turns it
/// back into an instant on its own clock. Nothing compares the two.
fn delay_secs(t: SystemTime) -> f64 {
    t.duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
}

/// Which table a [`PgQueue`] works on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Table {
    /// `queue_bulk`, the lane that can move to another backend.
    Bulk,
    /// `queue_tx`, the lane that never moves. A worker reads it with the
    /// same code; only the enqueue differs, and that is the whole point.
    Transactional,
}

impl Table {
    fn name(self) -> &'static str {
        match self {
            Table::Bulk => "queue_bulk",
            Table::Transactional => "queue_tx",
        }
    }
}

/// How the adapter behaves.
#[derive(Debug, Clone)]
pub struct PgConfig {
    /// Which table to work.
    pub table: Table,
    /// Keep a row in `queue_archive` when a job succeeds.
    ///
    /// On by default. "Did that job run?" cannot be answered backwards, and
    /// nobody turns a history on before the incident that needs it. The cost
    /// is one statement either way: the archive write is a CTE around the
    /// `DELETE` that already happens, into a partition with no index, so it
    /// adds a round trip to nothing and an index write to nothing.
    ///
    /// It records **settles, not executions**. A job that ran, had its
    /// effect and then lost its lease is retried and leaves one row, not
    /// two. Under at-least-once delivery the archive is a lower bound on
    /// what ran, never a count of it.
    pub archive: bool,
    /// How long an archived row is kept. [`Backend::sweep`] drops every
    /// partition that ends before the cutoff, which is one `DROP TABLE` per
    /// day and no dead tuples.
    ///
    /// Seven days and not one: a job that fails quietly on a Friday evening
    /// is asked about on Monday, and a one-day window has already dropped
    /// the answer. Seven days covers a business week plus the time it takes
    /// someone to escalate.
    pub archive_retention: Duration,
    /// A job due further out than this goes to `queue_scheduled` instead of
    /// sitting in the claim index.
    pub schedule_after: Duration,
    /// How far ahead the promoter moves a scheduled job into the queue.
    /// It must be longer than the sweep interval, or a job is late by one
    /// sweep.
    pub promote_ahead: Duration,
    /// Send `pg_notify` on every enqueue, so a listening worker wakes
    /// without waiting for its poll. A lost notification costs latency and
    /// never a job.
    pub notify: bool,
    /// The most rows one `claim` may take.
    ///
    /// The worker already asks for no more than it has free slots for, so
    /// this is a ceiling on a caller that drives the backend directly. It is
    /// small on purpose. One claim writes one `available_at` for the whole
    /// batch, so every row in it shares a deadline: a batch larger than what
    /// the caller can start inside one lease is a batch that expires while
    /// it is still held, with an attempt already spent on each row.
    ///
    /// River, Oban, `solid_queue` and `que` all take only what they can run
    /// now; SQS caps a receive at 10.
    pub max_claim_batch: usize,
}

impl Default for PgConfig {
    fn default() -> Self {
        PgConfig {
            table: Table::Bulk,
            archive: true,
            archive_retention: Duration::from_secs(7 * 24 * 60 * 60),
            schedule_after: Duration::from_secs(300),
            promote_ahead: Duration::from_secs(60),
            notify: true,
            max_claim_batch: 64,
        }
    }
}

/// The channel [`PgConfig::notify`] sends on.
pub const NOTIFY_CHANNEL: &str = "brocanth_queue";

/// A queue in Postgres.
#[derive(Debug, Clone)]
pub struct PgQueue {
    pub(crate) pool: PgPool,
    pub(crate) config: PgConfig,
}

impl PgQueue {
    /// The bulk queue.
    pub fn bulk(pool: PgPool) -> Self {
        PgQueue {
            pool,
            config: PgConfig::default(),
        }
    }

    /// The transactional queue, read side. The write side is [`Tx`].
    pub fn transactional(pool: PgPool) -> Self {
        PgQueue {
            pool,
            config: PgConfig {
                table: Table::Transactional,
                schedule_after: Duration::MAX,
                ..PgConfig::default()
            },
        }
    }

    /// Replace the configuration.
    pub fn with_config(mut self, config: PgConfig) -> Self {
        self.config = config;
        self
    }

    /// The pool. This must be the **control** pool, not the pool job bodies
    /// draw on. See [`brocanth_queue::Budget`].
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    async fn claim_window(&self, key: &str, secs: f64) -> Result<bool, BackendError> {
        // A window key has to outlive the row, so it does not live in the
        // partial unique index. `DO UPDATE ... WHERE` refreshes an expired
        // key and leaves a live one alone, in one statement and one lock.
        let row = sqlx::query(
            "INSERT INTO queue_dedup (key, expires_at) \
             VALUES ($1, now() + make_interval(secs => $2)) \
             ON CONFLICT (key) DO UPDATE SET expires_at = EXCLUDED.expires_at \
             WHERE queue_dedup.expires_at <= now() \
             RETURNING key",
        )
        .bind(key)
        .bind(secs)
        .fetch_optional(&self.pool)
        .await
        .map_err(err)?;
        Ok(row.is_some())
    }

    async fn insert_scheduled(&self, job: &NewJob, secs: f64) -> Result<Pushed, BackendError> {
        let row = sqlx::query(
            "INSERT INTO queue_scheduled \
             (kind, queue, payload, run_at, max_attempts, unique_key, uniqueness) \
             VALUES ($1, $2, $3, now() + make_interval(secs => $4), $5, $6, $7) \
             RETURNING id",
        )
        .bind(&job.kind)
        .bind(&job.queue)
        .bind(&job.payload)
        .bind(secs)
        .bind(job.max_attempts as i32)
        .bind(job.unique_key.as_deref())
        .bind(uniqueness_text(job.uniqueness))
        .fetch_one(&self.pool)
        .await
        .map_err(err)?;
        let id: i64 = row.get("id");
        Ok(Pushed::Enqueued(JobId(format!("s{id}"))))
    }
}

impl Backend for PgQueue {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // The payload is a `bytea`. The practical limit is the one the
            // application should impose, not one this adapter invents.
            max_payload_bytes: None,
            // The promoter holds a row until it is nearly due, so there is
            // no ceiling at all.
            max_delay: None,
            max_backoff: None,
            filter_by_kind: true,
            uniqueness: UniquenessSupport::Unbounded,
            cancel_pending: true,
            exact_depth: true,
            max_in_flight: None,
            max_claim_batch: self.config.max_claim_batch,
            ordered: false,
        }
    }

    async fn push(&self, batch: Vec<NewJob>) -> Result<Vec<Pushed>, BackendError> {
        let now = SystemTime::now();
        let table = self.config.table.name();
        let insert = format!(
            "INSERT INTO {table} \
             (kind, queue, payload, available_at, max_attempts, unique_key, uniqueness) \
             VALUES ($1, $2, $3, now() + make_interval(secs => $4), $5, $6, $7) \
             ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING \
             RETURNING id"
        );
        let mut out = Vec::with_capacity(batch.len());
        let mut woke = false;
        for job in batch {
            if let (Some(key), UniquenessKind::Window(ms)) =
                (job.unique_key.as_deref(), job.uniqueness)
            {
                if !self
                    .claim_window(key, Duration::from_millis(ms).as_secs_f64())
                    .await?
                {
                    out.push(Pushed::Duplicate);
                    continue;
                }
            }
            let run_at = job.run_at.unwrap_or(now);
            let ahead = run_at.duration_since(now).unwrap_or(Duration::ZERO);
            if ahead > self.config.schedule_after {
                out.push(self.insert_scheduled(&job, ahead.as_secs_f64()).await?);
                continue;
            }
            // A window key is held by `queue_dedup`, not by the row, so the
            // row must not also take the partial unique index.
            let row_key = match job.uniqueness {
                UniquenessKind::Window(_) | UniquenessKind::None => None,
                _ => job.unique_key.as_deref(),
            };
            let row = sqlx::query(AssertSqlSafe(insert.clone()))
                .bind(&job.kind)
                .bind(&job.queue)
                .bind(&job.payload)
                .bind(ahead.as_secs_f64())
                .bind(job.max_attempts as i32)
                .bind(row_key)
                .bind(uniqueness_text(job.uniqueness))
                .fetch_optional(&self.pool)
                .await
                .map_err(err)?;
            match row {
                Some(r) => {
                    let id: i64 = r.get("id");
                    out.push(Pushed::Enqueued(JobId(id.to_string())));
                    woke = true;
                }
                None => out.push(Pushed::Duplicate),
            }
        }
        if woke && self.config.notify {
            // Best effort. A missed wake costs one poll interval.
            let _ = sqlx::query("SELECT pg_notify($1, $2)")
                .bind(NOTIFY_CHANNEL)
                .bind(table)
                .execute(&self.pool)
                .await;
        }
        Ok(out)
    }

    async fn claim(&self, req: &ClaimRequest) -> Result<Vec<Claimed>, BackendError> {
        let table = self.config.table.name();
        // `FOR UPDATE SKIP LOCKED` inside a CTE picks the rows, and the
        // UPDATE takes them. Without SKIP LOCKED every worker queues behind
        // the first one's row lock and the claim serialises.
        //
        // `attempts < max_attempts` is here and not only in the sweep: a job
        // whose worker dies every time still spends an attempt on each
        // claim, and this is what stops it looping for ever.
        //
        // The UPDATE sets `available_at` to the lease end, so the lease and
        // the visibility are one column. Nothing has to expire a lease.
        let sql = format!(
            "WITH picked AS ( \
                 SELECT id FROM {table} \
                  WHERE queue = $1 \
                    AND available_at <= now() \
                    AND attempts < max_attempts \
                    AND ($4::text[] IS NULL OR kind = ANY($4)) \
                  ORDER BY available_at \
                  FOR UPDATE SKIP LOCKED \
                  LIMIT $2 \
             ) \
             UPDATE {table} j \
                SET lease_token = gen_random_uuid(), \
                    available_at = now() + make_interval(secs => $3), \
                    attempts = j.attempts + 1, \
                    claimed_by = $5, \
                    claimed_at = now() \
               FROM picked \
              WHERE j.id = picked.id \
          RETURNING j.id, j.kind, j.queue, j.payload, j.attempts, j.max_attempts, \
                    j.lease_token, j.available_at"
        );
        let max = req.max.min(self.capabilities().max_claim_batch) as i64;
        let rows = sqlx::query(AssertSqlSafe(sql.clone()))
            .bind(&req.queue)
            .bind(max)
            .bind(req.lease.as_secs_f64())
            .bind(req.kinds.as_deref())
            .bind(&req.worker)
            .fetch_all(&self.pool)
            .await
            .map_err(err)?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let id: i64 = r.get("id");
                let token: uuid::Uuid = r.get("lease_token");
                let expires: DateTime<Utc> = r.get("available_at");
                Claimed {
                    lease: LeaseRef {
                        id: JobId(id.to_string()),
                        token: token.to_string(),
                        expires_at: SystemTime::from(expires),
                    },
                    kind: r.get("kind"),
                    queue: r.get("queue"),
                    payload: r.get("payload"),
                    attempt: r.get::<i32, _>("attempts") as u32,
                    max_attempts: r.get::<i32, _>("max_attempts") as u32,
                }
            })
            .collect())
    }

    async fn heartbeat(
        &self,
        lease: &LeaseRef,
        extend: Duration,
    ) -> Result<LeaseState, BackendError> {
        let Some((id, token)) = parse(lease) else {
            return Ok(LeaseState::Lost);
        };
        let table = self.config.table.name();
        let sql = format!(
            "UPDATE {table} SET available_at = now() + make_interval(secs => $3) \
              WHERE id = $1 AND lease_token = $2"
        );
        let done = sqlx::query(AssertSqlSafe(sql.clone()))
            .bind(id)
            .bind(token)
            .bind(extend.as_secs_f64())
            .execute(&self.pool)
            .await
            .map_err(err)?;
        Ok(if done.rows_affected() == 1 {
            LeaseState::Held
        } else {
            LeaseState::Lost
        })
    }

    async fn settle(&self, lease: &LeaseRef, outcome: Settle) -> Result<LeaseState, BackendError> {
        let Some((id, token)) = parse(lease) else {
            return Ok(LeaseState::Lost);
        };
        let table = self.config.table.name();
        // Every statement carries the token. This is the fence: a worker
        // that stalled past its lease and woke up changes nothing, because
        // the row it remembers now has another worker's token.
        let affected = match outcome {
            Settle::Done if self.config.archive => {
                let sql = format!(
                    "WITH gone AS ( \
                         DELETE FROM {table} WHERE id = $1 AND lease_token = $2 \
                         RETURNING id, kind, queue, attempts, enqueued_at \
                     ) \
                     INSERT INTO queue_archive \
                         (id, kind, queue, attempts, enqueued_at, finished_at) \
                     SELECT id, kind, queue, attempts, enqueued_at, now() FROM gone"
                );
                sqlx::query(AssertSqlSafe(sql.clone()))
                    .bind(id)
                    .bind(token)
                    .execute(&self.pool)
                    .await
                    .map_err(err)?
                    .rows_affected()
            }
            Settle::Done => {
                let sql = format!("DELETE FROM {table} WHERE id = $1 AND lease_token = $2");
                sqlx::query(AssertSqlSafe(sql.clone()))
                    .bind(id)
                    .bind(token)
                    .execute(&self.pool)
                    .await
                    .map_err(err)?
                    .rows_affected()
            }
            Settle::Dead { failure } => {
                let sql = format!(
                    "WITH gone AS ( \
                         DELETE FROM {table} WHERE id = $1 AND lease_token = $2 \
                         RETURNING id, kind, queue, payload, attempts, \
                                   max_attempts, unique_key, enqueued_at \
                     ) \
                     INSERT INTO queue_dead \
                         (job_id, kind, queue, payload, attempts, max_attempts, \
                          unique_key, failure, enqueued_at) \
                     SELECT id, kind, queue, payload, attempts, max_attempts, \
                            unique_key, $3, enqueued_at FROM gone"
                );
                sqlx::query(AssertSqlSafe(sql.clone()))
                    .bind(id)
                    .bind(token)
                    .bind(failure.to_string())
                    .execute(&self.pool)
                    .await
                    .map_err(err)?
                    .rows_affected()
            }
            Settle::Retry { at, failure } => {
                let sql = format!(
                    "UPDATE {table} \
                        SET lease_token = NULL, \
                            claimed_by = NULL, \
                            claimed_at = NULL, \
                            available_at = now() + make_interval(secs => $3), \
                            last_error = $4 \
                      WHERE id = $1 AND lease_token = $2"
                );
                sqlx::query(AssertSqlSafe(sql.clone()))
                    .bind(id)
                    .bind(token)
                    .bind(delay_secs(at))
                    .bind(failure.to_string())
                    .execute(&self.pool)
                    .await
                    .map_err(err)?
                    .rows_affected()
            }
            Settle::Release { not_before, reason } => {
                // `attempts - 1` undoes what the claim added. An old worker
                // that meets a job kind it was built before must not spend
                // the retry budget of a job the new workers can run.
                let sql = format!(
                    "UPDATE {table} \
                        SET lease_token = NULL, \
                            claimed_by = NULL, \
                            claimed_at = NULL, \
                            available_at = now() + make_interval(secs => $3), \
                            attempts = greatest(attempts - 1, 0), \
                            last_error = $4 \
                      WHERE id = $1 AND lease_token = $2"
                );
                sqlx::query(AssertSqlSafe(sql.clone()))
                    .bind(id)
                    .bind(token)
                    .bind(delay_secs(not_before))
                    .bind(format!("released: {reason:?}"))
                    .execute(&self.pool)
                    .await
                    .map_err(err)?
                    .rows_affected()
            }
        };
        Ok(if affected >= 1 {
            LeaseState::Held
        } else {
            LeaseState::Lost
        })
    }

    async fn approx_depth(&self, queue: &str) -> Result<u64, BackendError> {
        let table = self.config.table.name();
        let sql = format!(
            "SELECT count(*) AS n FROM {table} \
              WHERE queue = $1 AND available_at <= now() AND attempts < max_attempts"
        );
        let row = sqlx::query(AssertSqlSafe(sql.clone()))
            .bind(queue)
            .fetch_one(&self.pool)
            .await
            .map_err(err)?;
        Ok(row.get::<i64, _>("n") as u64)
    }

    async fn cancel(&self, id: &JobId) -> Result<bool, BackendError> {
        let table = self.config.table.name();
        if let Some(sched) = id.as_str().strip_prefix('s') {
            let Ok(n) = sched.parse::<i64>() else {
                return Ok(false);
            };
            let done = sqlx::query("DELETE FROM queue_scheduled WHERE id = $1")
                .bind(n)
                .execute(&self.pool)
                .await
                .map_err(err)?;
            return Ok(done.rows_affected() == 1);
        }
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(false);
        };
        // "Has not started" means no live lease. A job whose worker died
        // has a token and an expiry in the past, and it is cancellable.
        let sql = format!(
            "DELETE FROM {table} \
              WHERE id = $1 AND (lease_token IS NULL OR available_at <= now())"
        );
        let done = sqlx::query(AssertSqlSafe(sql.clone()))
            .bind(n)
            .execute(&self.pool)
            .await
            .map_err(err)?;
        Ok(done.rows_affected() == 1)
    }

    async fn purge(&self, queue: &str) -> Result<u64, BackendError> {
        let table = self.config.table.name();
        let a = sqlx::query(AssertSqlSafe(format!(
            "DELETE FROM {table} WHERE queue = $1"
        )))
        .bind(queue)
        .execute(&self.pool)
        .await
        .map_err(err)?;
        let b = sqlx::query("DELETE FROM queue_scheduled WHERE queue = $1")
            .bind(queue)
            .execute(&self.pool)
            .await
            .map_err(err)?;
        Ok(a.rows_affected() + b.rows_affected())
    }

    async fn sweep(&self) -> Result<(), BackendError> {
        let table = self.config.table.name();

        // 1. Promote scheduled work that is nearly due. This is what makes
        //    an unbounded delay work, here and on a backend with a ceiling.
        if self.config.table == Table::Bulk {
            let sql = format!(
                "WITH due AS ( \
                     DELETE FROM queue_scheduled \
                      WHERE run_at <= now() + make_interval(secs => $1) \
                     RETURNING kind, queue, payload, run_at, max_attempts, \
                               unique_key, uniqueness, enqueued_at \
                 ) \
                 INSERT INTO {table} \
                     (kind, queue, payload, available_at, max_attempts, \
                      unique_key, uniqueness, enqueued_at) \
                 SELECT kind, queue, payload, run_at, max_attempts, \
                        CASE WHEN uniqueness = 'until_success' \
                             THEN unique_key ELSE NULL END, \
                        uniqueness, enqueued_at \
                   FROM due \
                 ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING"
            );
            sqlx::query(AssertSqlSafe(sql.clone()))
                .bind(self.config.promote_ahead.as_secs_f64())
                .execute(&self.pool)
                .await
                .map_err(err)?;
        }

        // 2. A job whose worker died on every attempt never reaches
        //    `settle`, so nothing has moved it to the dead letter. The claim
        //    already refuses it; this is what stops it sitting there.
        //
        //    The condition is `available_at <= now()` and not
        //    `lease_token IS NULL`. An expired lease still carries the token
        //    of the worker that died: nothing clears it, because expiry is
        //    `available_at` passing and not an event anybody runs. Asking
        //    for a null token here would leave every such job in the table
        //    for ever.
        let sql = format!(
            "WITH gone AS ( \
                 DELETE FROM {table} \
                  WHERE attempts >= max_attempts \
                    AND available_at <= now() \
                 RETURNING id, kind, queue, payload, attempts, max_attempts, \
                           unique_key, last_error, enqueued_at \
             ) \
             INSERT INTO queue_dead \
                 (job_id, kind, queue, payload, attempts, max_attempts, \
                  unique_key, failure, enqueued_at) \
             SELECT id, kind, queue, payload, attempts, max_attempts, unique_key, \
                    coalesce(last_error, 'the worker died on every attempt'), enqueued_at \
               FROM gone"
        );
        sqlx::query(AssertSqlSafe(sql.clone()))
            .execute(&self.pool)
            .await
            .map_err(err)?;

        // 3. Expired deduplication windows.
        sqlx::query("DELETE FROM queue_dedup WHERE expires_at <= now()")
            .execute(&self.pool)
            .await
            .map_err(err)?;

        // 4. The archive's partitions, extended ahead and dropped behind.
        //
        //    This has to run on a timer and not only at boot. `migrate`
        //    creates today's partition and the next two days'; a process
        //    that stays up longer than that reaches a day with no partition
        //    to insert into, and then every archiving settle fails. A settle
        //    that does not land leaves the job to expire and run twice, so
        //    the missing partition would have turned a retention detail into
        //    duplicate execution across the whole queue.
        //
        //    Only one table does this, or two queues on one database would
        //    race to create and drop the same partitions.
        if self.config.archive && self.config.table == Table::Bulk {
            partitions::ensure_archive_partitions(&self.pool, PARTITIONS_AHEAD)
                .await
                .map_err(err)?;
            let keep_days = self.config.archive_retention.as_secs() / 86_400;
            let cutoff = Utc::now().date_naive() - chrono::Duration::days(keep_days as i64);
            partitions::drop_archive_partitions_before(&self.pool, cutoff)
                .await
                .map_err(err)?;
        }
        Ok(())
    }
}

fn parse(lease: &LeaseRef) -> Option<(i64, uuid::Uuid)> {
    let id = lease.id.as_str().parse::<i64>().ok()?;
    let token = lease.token.parse::<uuid::Uuid>().ok()?;
    Some((id, token))
}

/// Everything in the dead letter, newest first.
pub async fn dead_letter(pool: &PgPool, limit: i64) -> Result<Vec<DeadRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT kind, queue, attempts, failure, died_at FROM queue_dead \
         ORDER BY died_at DESC LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| DeadRow {
            kind: r.get("kind"),
            queue: r.get("queue"),
            attempts: r.get::<i32, _>("attempts") as u32,
            failure: r.get("failure"),
            died_at: SystemTime::from(r.get::<DateTime<Utc>, _>("died_at")),
        })
        .collect())
}

/// One row of the dead letter.
#[derive(Debug, Clone)]
pub struct DeadRow {
    /// Its kind.
    pub kind: String,
    /// Its queue.
    pub queue: String,
    /// How many times it ran.
    pub attempts: u32,
    /// The last failure.
    pub failure: String,
    /// When it was given up on.
    pub died_at: SystemTime,
}

/// Wake a worker when a job is enqueued.
///
/// `PgListener` holds a connection for its whole life, so count it in
/// [`brocanth_queue::Budget`]. A notification sent while the listener is
/// reconnecting is lost, which is why this only ever shortens a poll and is
/// never the way work is found.
pub async fn wakeups(pool: &PgPool) -> Result<tokio::sync::watch::Receiver<u64>, sqlx::Error> {
    let mut listener = sqlx::postgres::PgListener::connect_with(pool).await?;
    listener.listen(NOTIFY_CHANNEL).await?;
    let (tx, rx) = tokio::sync::watch::channel(0u64);
    tokio::spawn(async move {
        let mut n = 0u64;
        loop {
            match listener.recv().await {
                Ok(_) => {
                    n += 1;
                    if tx.send(n).is_err() {
                        return;
                    }
                }
                Err(_) => {
                    // The connection dropped. `recv` reconnects on the next
                    // call, and whatever was sent in between is gone. The
                    // poll interval is the backstop.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });
    Ok(rx)
}

/// Record a failure string the way the dead letter does.
pub fn failure_text(f: &JobFailure) -> String {
    f.to_string()
}
