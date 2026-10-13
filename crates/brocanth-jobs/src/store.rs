//! The database. Three tables, two dialects, and the claim that is the whole
//! reason a database queue works.
//!
//! # `FOR UPDATE SKIP LOCKED`, and what SQLite does instead
//!
//! On **Postgres** a worker claims work with
//!
//! ```sql
//! UPDATE brocanth_jobs SET state = 'running', ...
//! WHERE id IN (
//!     SELECT id FROM brocanth_jobs
//!     WHERE state = 'pending' AND run_at <= $2 AND queue = ANY($3)
//!     ORDER BY priority ASC, run_at ASC, id ASC
//!     LIMIT $4 FOR UPDATE SKIP LOCKED
//! ) RETURNING ...
//! ```
//!
//! `FOR UPDATE` takes a row lock inside the transaction; `SKIP LOCKED` tells
//! the second worker to walk past the rows the first one is holding instead of
//! blocking on them. Two workers therefore never see the same row, and twenty
//! workers do not serialise behind one. This is the pattern that makes "just
//! use your database" the right answer for almost everybody, and it is the one
//! piece of this crate that is not an opinion.
//!
//! **SQLite has no `SKIP LOCKED`**, and it has no row locks to skip: the
//! database takes one write lock for the whole file. So the strategy is
//! different, and the difference is worth stating precisely rather than
//! hand-waving.
//!
//! The claim runs as `BEGIN IMMEDIATE` — which takes the write lock up front
//! rather than discovering halfway through that it needs one — followed by the
//! same `UPDATE ... WHERE id IN (SELECT ... LIMIT n) RETURNING ...`. A second
//! worker claiming at the same moment blocks on the write lock, waits out its
//! `busy_timeout`, and then runs its own subquery against the *committed*
//! result of the first. It cannot see a row the first worker took, because by
//! the time it can read at all, that row says `running`.
//!
//! So the correctness guarantee is identical — **no job is ever claimed
//! twice** — and the test that proves it runs on SQLite. What is not identical
//! is throughput: claims serialise, where on Postgres they overlap. A handful
//! of workers on one machine will not notice; thirty workers on a network
//! filesystem will, and the answer there is Postgres, which is the answer
//! anyway.
//!
//! `BEGIN IMMEDIATE` is also what makes the `busy_timeout` effective at all. A
//! deferred transaction that reads first and writes second can fail with
//! `SQLITE_BUSY_SNAPSHOT`, which no busy handler can retry, and that is the
//! classic way a SQLite queue produces mystery errors under load.
//!
//! # Time is a `BIGINT`
//!
//! Every instant is epoch milliseconds in both dialects. See [`crate::clock`].

use std::time::Duration;

use sqlx::Row as _;

use crate::clock::from_millis;
use crate::error::{Error, Result};
use crate::job::{FailureKind, JobFailure, JobId};

/// Which SQL dialect a statement is being written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Dialect {
    /// Postgres: `$1` placeholders, `FOR UPDATE SKIP LOCKED`, `BIGSERIAL`.
    #[cfg(feature = "postgres")]
    Postgres,
    /// SQLite: `?` placeholders, one writer, `INTEGER PRIMARY KEY`.
    #[cfg(feature = "sqlite")]
    Sqlite,
}

/// A pool, of whichever kind was compiled in.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Backend {
    /// A Postgres pool.
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    /// A SQLite pool.
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::SqlitePool),
}

impl Backend {
    /// Which dialect this pool speaks.
    pub(crate) fn dialect(&self) -> Dialect {
        match self {
            #[cfg(feature = "postgres")]
            Backend::Postgres(_) => Dialect::Postgres,
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(_) => Dialect::Sqlite,
        }
    }

    /// The name of the backend, for a message a human reads.
    pub fn name(&self) -> &'static str {
        match self {
            #[cfg(feature = "postgres")]
            Backend::Postgres(_) => "postgres",
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(_) => "sqlite",
        }
    }

    /// Closes the pool.
    pub async fn close(&self) {
        match self {
            #[cfg(feature = "postgres")]
            Backend::Postgres(p) => p.close().await,
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(p) => p.close().await,
        }
    }
}

#[cfg(feature = "postgres")]
impl From<sqlx::PgPool> for Backend {
    fn from(pool: sqlx::PgPool) -> Self {
        Backend::Postgres(pool)
    }
}

#[cfg(feature = "sqlite")]
impl From<sqlx::SqlitePool> for Backend {
    fn from(pool: sqlx::SqlitePool) -> Self {
        Backend::Sqlite(pool)
    }
}

// ---------------------------------------------------------------------------
// The two statement shapes that repeat, so the dialect match is written once
// ---------------------------------------------------------------------------

/// `execute`, in whichever dialect, returning the number of rows affected.
///
/// Each arm is expanded separately and type-checked separately, which is how
/// one bind list serves two different `Arguments` types. Only one arm ever
/// runs.
macro_rules! execute {
    ($backend:expr, $pg:expr, $lite:expr $(, $arg:expr)* $(,)?) => {{
        match $backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => sqlx::query($pg)$(.bind($arg))*
                .execute(pool).await?.rows_affected(),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => sqlx::query($lite)$(.bind($arg))*
                .execute(pool).await?.rows_affected(),
        }
    }};
}

/// Multi-statement work, in a transaction that is `IMMEDIATE` on SQLite.
macro_rules! transaction {
    ($backend:expr, |$tx:ident, $dialect:ident| $body:block) => {{
        match $backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => {
                #[allow(unused_mut)]
                let mut $tx = pool.begin().await?;
                let $dialect = Dialect::Postgres;
                let out = $body;
                $tx.commit().await?;
                out
            }
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => {
                // Not `begin()`: a deferred transaction that reads before it
                // writes can fail with SQLITE_BUSY_SNAPSHOT, which `busy_timeout`
                // cannot retry.
                #[allow(unused_mut)]
                let mut $tx = pool.begin_with("BEGIN IMMEDIATE").await?;
                let $dialect = Dialect::Sqlite;
                let out = $body;
                $tx.commit().await?;
                out
            }
        }
    }};
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/// A claimed job, as it comes out of the database.
#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct JobRow {
    pub(crate) id: i64,
    pub(crate) queue: String,
    pub(crate) job_name: String,
    pub(crate) payload: String,
    pub(crate) attempts: i64,
    pub(crate) max_attempts: i64,
    pub(crate) created_at: i64,
}

/// A job that used up every attempt.
///
/// This is the record Loco.rs does not keep and the reason its own
/// documentation has to say that a failed job is terminal: once a job is gone,
/// "why did this not happen" is a question with no answer. Everything needed to
/// answer it is here, and [`Queue::requeue_dead`](crate::Queue::requeue_dead)
/// puts the job back.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct DeadJob {
    /// The dead letter's own id, which is what you requeue or discard by.
    pub id: i64,
    /// The id the job had while it was live.
    pub job_id: i64,
    /// Which queue it was on.
    pub queue: String,
    /// Its [`Job::NAME`](crate::Job::NAME).
    pub job_name: String,
    /// The payload, verbatim. Readable, editable, and what a requeue replays.
    pub payload: String,
    /// How many attempts it used.
    pub attempts: i64,
    /// How many it was allowed.
    pub max_attempts: i64,
    /// Its unique key, if it had one.
    pub unique_key: Option<String>,
    /// The last error's text.
    pub error: String,
    /// What kind of failure it was: returned, panicked, timed out, undecodable,
    /// orphaned.
    pub failure_kind: String,
    /// A backtrace, when one was captured. See
    /// [`capture_backtraces`](crate::capture_backtraces).
    pub backtrace: Option<String>,
    /// When the job was first enqueued, in epoch milliseconds.
    pub enqueued_at: i64,
    /// When it was given up on, in epoch milliseconds.
    pub failed_at: i64,
}

impl DeadJob {
    /// When the job was first enqueued.
    pub fn enqueued_at(&self) -> chrono::DateTime<chrono::Utc> {
        from_millis(self.enqueued_at)
    }

    /// When it was given up on.
    pub fn failed_at(&self) -> chrono::DateTime<chrono::Utc> {
        from_millis(self.failed_at)
    }
}

/// A row from `brocanth_job_schedules`.
#[derive(Clone, Debug, sqlx::FromRow)]
pub(crate) struct ScheduleRow {
    pub(crate) name: String,
    pub(crate) cron: String,
    pub(crate) job_name: String,
    pub(crate) queue: String,
    pub(crate) payload: String,
    pub(crate) priority: i64,
    pub(crate) max_attempts: i64,
    pub(crate) next_run_at: i64,
    pub(crate) misfire: String,
    pub(crate) grace_ms: i64,
}

/// What a new row needs.
#[derive(Clone, Debug)]
pub(crate) struct NewJob {
    pub(crate) queue: String,
    pub(crate) job_name: String,
    pub(crate) payload: String,
    pub(crate) priority: i64,
    pub(crate) max_attempts: i64,
    pub(crate) run_at: i64,
    pub(crate) unique_key: Option<String>,
}

/// What happened to an enqueue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enqueued {
    /// A new row.
    Inserted(JobId),
    /// A row with this unique key was already queued; nothing was inserted and
    /// this is the id of the one that was already there.
    Duplicate(JobId),
}

impl Enqueued {
    /// The row this enqueue refers to, inserted or not.
    pub fn id(self) -> JobId {
        match self {
            Enqueued::Inserted(id) | Enqueued::Duplicate(id) => id,
        }
    }

    /// Whether the enqueue was folded into an existing row.
    pub fn is_duplicate(self) -> bool {
        matches!(self, Enqueued::Duplicate(_))
    }
}

/// How far behind one queue is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueDepth {
    /// The queue's name.
    pub queue: String,
    /// Jobs that could run right now.
    pub ready: u64,
    /// Jobs waiting for their `run_at`.
    pub scheduled: u64,
    /// Jobs a worker is holding.
    pub running: u64,
    /// How long the oldest runnable job has been runnable.
    pub oldest_ready_age: Option<Duration>,
}

/// The state of the whole queue.
///
/// `oldest_ready_age` is the number to put on a dashboard. Counts go up and
/// down for reasons that are not problems — a burst of work is not an incident
/// — but a *ready* job that has been ready for four minutes means no worker got
/// to it, and that is the only number that distinguishes a busy queue from a
/// queue that is falling behind or has stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stats {
    /// Jobs that could run right now, across every queue.
    pub ready: u64,
    /// Jobs waiting for their `run_at`.
    pub scheduled: u64,
    /// Jobs a worker is holding.
    pub running: u64,
    /// Rows in the dead letter.
    pub dead: u64,
    /// How long the oldest runnable job, anywhere, has been runnable.
    pub oldest_ready_age: Option<Duration>,
    /// The same figures per queue, sorted by name.
    pub queues: Vec<QueueDepth>,
}

impl Stats {
    /// Everything that exists and is not dead.
    pub fn total(&self) -> u64 {
        self.ready + self.scheduled + self.running
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) struct Store {
    backend: Backend,
}

impl Store {
    pub(crate) fn new(backend: Backend) -> Self {
        Self { backend }
    }

    pub(crate) fn backend(&self) -> &Backend {
        &self.backend
    }

    // -- schema -------------------------------------------------------------

    /// Creates the three tables and their indexes if they are not there.
    ///
    /// Idempotent, and deliberately not a migration framework: §4.3 of the plan
    /// says migrations are files a developer reads, edits and commits, and this
    /// crate is not going to invent a second mechanism for its own three
    /// tables. [`Queue::schema_sql`](crate::Queue::schema_sql) hands the same
    /// statements back as text, so an application that keeps every change in
    /// `migrations/` can paste them into one.
    ///
    /// On Postgres it runs under an advisory lock. `CREATE TABLE IF NOT
    /// EXISTS` is **not** safe to run concurrently there: two of them pass the
    /// existence check together and then collide in the catalogue with
    /// `duplicate key value violates unique constraint
    /// "pg_class_relname_nsp_index"`. Two workers booting at the same moment
    /// is exactly that race, and so is a parallel test suite against one
    /// server. `pg_advisory_xact_lock` serialises them, and Postgres rolls DDL
    /// back, so a half-finished schema cannot be left behind. SQLite needs
    /// none of this: it takes one write lock for the whole file anyway.
    pub(crate) async fn migrate(&self) -> Result<()> {
        match &self.backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => {
                let mut tx = pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock($1)")
                    .bind(MIGRATION_LOCK)
                    .execute(&mut *tx)
                    .await?;
                for statement in schema(Dialect::Postgres) {
                    sqlx::query(*statement).execute(&mut *tx).await?;
                }
                tx.commit().await?;
            }
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => {
                for statement in schema(Dialect::Sqlite) {
                    sqlx::query(*statement).execute(pool).await?;
                }
            }
        }
        Ok(())
    }

    // -- enqueue ------------------------------------------------------------

    pub(crate) async fn enqueue(&self, job: &NewJob, now: i64) -> Result<Enqueued> {
        match &job.unique_key {
            None => {
                let id = match &self.backend {
                    #[cfg(feature = "postgres")]
                    Backend::Postgres(pool) => sqlx::query(sql::INSERT_PG)
                        .bind(&job.queue)
                        .bind(&job.job_name)
                        .bind(&job.payload)
                        .bind(job.priority)
                        .bind(job.max_attempts)
                        .bind(job.run_at)
                        .bind(now)
                        .fetch_one(pool)
                        .await?
                        .try_get::<i64, _>(0)?,
                    #[cfg(feature = "sqlite")]
                    Backend::Sqlite(pool) => sqlx::query(sql::INSERT_SQLITE)
                        .bind(&job.queue)
                        .bind(&job.job_name)
                        .bind(&job.payload)
                        .bind(job.priority)
                        .bind(job.max_attempts)
                        .bind(job.run_at)
                        .bind(now)
                        .fetch_one(pool)
                        .await?
                        .try_get::<i64, _>(0)?,
                };
                Ok(Enqueued::Inserted(JobId(id)))
            }
            Some(key) => {
                // `ON CONFLICT ... DO NOTHING RETURNING id` returns no row when
                // the key is taken, which is the whole mechanism: the uniqueness
                // is a partial unique index the database enforces, not a check
                // this code does first and then races with itself.
                let inserted: Option<i64> = match &self.backend {
                    #[cfg(feature = "postgres")]
                    Backend::Postgres(pool) => sqlx::query(sql::INSERT_UNIQUE_PG)
                        .bind(&job.queue)
                        .bind(&job.job_name)
                        .bind(&job.payload)
                        .bind(job.priority)
                        .bind(job.max_attempts)
                        .bind(job.run_at)
                        .bind(now)
                        .bind(key)
                        .fetch_optional(pool)
                        .await?
                        .map(|r| r.try_get::<i64, _>(0))
                        .transpose()?,
                    #[cfg(feature = "sqlite")]
                    Backend::Sqlite(pool) => sqlx::query(sql::INSERT_UNIQUE_SQLITE)
                        .bind(&job.queue)
                        .bind(&job.job_name)
                        .bind(&job.payload)
                        .bind(job.priority)
                        .bind(job.max_attempts)
                        .bind(job.run_at)
                        .bind(now)
                        .bind(key)
                        .fetch_optional(pool)
                        .await?
                        .map(|r| r.try_get::<i64, _>(0))
                        .transpose()?,
                };
                match inserted {
                    Some(id) => Ok(Enqueued::Inserted(JobId(id))),
                    None => {
                        let existing: Option<i64> = match &self.backend {
                            #[cfg(feature = "postgres")]
                            Backend::Postgres(pool) => sqlx::query(sql::FIND_BY_KEY_PG)
                                .bind(key)
                                .fetch_optional(pool)
                                .await?
                                .map(|r| r.try_get::<i64, _>(0))
                                .transpose()?,
                            #[cfg(feature = "sqlite")]
                            Backend::Sqlite(pool) => sqlx::query(sql::FIND_BY_KEY_SQLITE)
                                .bind(key)
                                .fetch_optional(pool)
                                .await?
                                .map(|r| r.try_get::<i64, _>(0))
                                .transpose()?,
                        };
                        match existing {
                            Some(id) => Ok(Enqueued::Duplicate(JobId(id))),
                            // The holder finished between the failed insert and
                            // this lookup. Rare, and the right answer is to try
                            // once more rather than to report a duplicate of
                            // something that is no longer there.
                            None => Box::pin(self.enqueue(job, now)).await,
                        }
                    }
                }
            }
        }
    }

    // -- claim --------------------------------------------------------------

    /// Takes up to `limit` runnable jobs for `worker`, counting an attempt on
    /// each.
    ///
    /// The attempt is counted here, at the start, not at the failure. A job
    /// whose worker is killed without a word has still used one, which is what
    /// stops a job that crashes its process from killing every worker in turn
    /// forever.
    pub(crate) async fn claim(
        &self,
        worker: &str,
        queues: &[String],
        limit: i64,
        now: i64,
    ) -> Result<Vec<JobRow>> {
        if queues.is_empty() || limit <= 0 {
            return Ok(Vec::new());
        }
        match &self.backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => Ok(sqlx::query_as::<_, JobRow>(sql::CLAIM_PG)
                .bind(worker)
                .bind(now)
                .bind(queues)
                .bind(limit)
                .fetch_all(pool)
                .await?),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => {
                let statement = sql::claim_sqlite(queues.len());
                let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
                // Four values before the queue list: locked_by, locked_at,
                // heartbeat_at and the `run_at <= ?` the subquery filters on.
                let mut q = sqlx::query_as::<_, JobRow>(sqlx::AssertSqlSafe(statement))
                    .bind(worker)
                    .bind(now)
                    .bind(now)
                    .bind(now);
                for queue in queues {
                    q = q.bind(queue);
                }
                let rows = q.bind(limit).fetch_all(&mut *tx).await?;
                tx.commit().await?;
                Ok(rows)
            }
        }
    }

    // -- the end of an attempt ----------------------------------------------

    /// A job returned `Ok`. The row goes away.
    ///
    /// Finished jobs are deleted rather than kept in a `finished` state. A
    /// queue table that only grows needs a reaper, the reaper needs
    /// configuration, and the configuration is the thing nobody sets. The
    /// durable record of a job that mattered is the dead letter, which only
    /// holds failures and is therefore small.
    pub(crate) async fn complete(&self, id: JobId, worker: &str) -> Result<u64> {
        Ok(execute!(
            &self.backend,
            sql::COMPLETE_PG,
            sql::COMPLETE_SQLITE,
            id.0,
            worker
        ))
    }

    /// A job failed and has attempts left. It goes back to `pending` with a
    /// later `run_at`.
    pub(crate) async fn retry_at(
        &self,
        id: JobId,
        worker: &str,
        run_at: i64,
        error: &str,
    ) -> Result<u64> {
        Ok(execute!(
            &self.backend,
            sql::RETRY_PG,
            sql::RETRY_SQLITE,
            run_at,
            error,
            id.0,
            worker
        ))
    }

    /// A job is out of attempts. The row moves to the dead letter.
    pub(crate) async fn dead_letter(
        &self,
        id: JobId,
        failure: &JobFailure,
        now: i64,
    ) -> Result<bool> {
        let moved = transaction!(&self.backend, |tx, dialect| {
            let inserted = sqlx::query(sql::dead_insert(dialect))
                .bind(&failure.message)
                .bind(failure.kind.as_str())
                .bind(failure.backtrace.as_deref())
                .bind(now)
                .bind(id.0)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if inserted > 0 {
                sqlx::query(sql::delete_one(dialect))
                    .bind(id.0)
                    .execute(&mut *tx)
                    .await?;
            }
            inserted > 0
        });
        Ok(moved)
    }

    /// Shutdown reached its deadline with these jobs still running. They go
    /// back, and the attempt is given back with them.
    ///
    /// Giving the attempt back is the point. The worker chose to stop; the job
    /// did not fail, and burning one of its five attempts every time somebody
    /// deploys is how a job that takes four minutes ends up in the dead letter
    /// after a busy afternoon of releases.
    pub(crate) async fn release(&self, worker: &str, now: i64) -> Result<u64> {
        Ok(execute!(
            &self.backend,
            sql::RELEASE_PG,
            sql::RELEASE_SQLITE,
            now,
            worker
        ))
    }

    // -- orphans ------------------------------------------------------------

    /// Puts back every running job whose worker stopped heartbeating.
    ///
    /// Returns `(requeued, dead_lettered)`. A job that has no attempts left
    /// goes to the dead letter with `orphaned` on it rather than back onto the
    /// queue, because a job that has now killed five workers will kill the
    /// sixth.
    ///
    /// Requeued jobs become runnable immediately, with no backoff. An orphan is
    /// nearly always a deploy or a killed container rather than a poison
    /// payload, and the attempt counter is already the guard against the poison
    /// case.
    pub(crate) async fn reclaim_orphans(&self, now: i64, stale_before: i64) -> Result<(u64, u64)> {
        let failure = JobFailure::of(
            FailureKind::Orphaned,
            "the worker holding this job stopped sending heartbeats",
        );
        let out = transaction!(&self.backend, |tx, dialect| {
            let dead = sqlx::query(sql::orphan_dead_insert(dialect))
                .bind(&failure.message)
                .bind(failure.kind.as_str())
                .bind(now)
                .bind(stale_before)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            if dead > 0 {
                sqlx::query(sql::orphan_delete(dialect))
                    .bind(stale_before)
                    .execute(&mut *tx)
                    .await?;
            }
            let requeued = sqlx::query(sql::orphan_requeue(dialect))
                .bind(now)
                .bind(&failure.message)
                .bind(stale_before)
                .execute(&mut *tx)
                .await?
                .rows_affected();
            (requeued, dead)
        });
        Ok(out)
    }

    /// Marks every job this worker holds as still alive.
    pub(crate) async fn heartbeat(&self, worker: &str, now: i64) -> Result<u64> {
        Ok(execute!(
            &self.backend,
            sql::HEARTBEAT_PG,
            sql::HEARTBEAT_SQLITE,
            now,
            worker
        ))
    }

    // -- the dead letter ----------------------------------------------------

    pub(crate) async fn dead_jobs(&self, limit: i64, offset: i64) -> Result<Vec<DeadJob>> {
        match &self.backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => Ok(sqlx::query_as::<_, DeadJob>(sql::DEAD_LIST_PG)
                .bind(limit)
                .bind(offset)
                .fetch_all(pool)
                .await?),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => Ok(sqlx::query_as::<_, DeadJob>(sql::DEAD_LIST_SQLITE)
                .bind(limit)
                .bind(offset)
                .fetch_all(pool)
                .await?),
        }
    }

    pub(crate) async fn dead_job(&self, id: i64) -> Result<Option<DeadJob>> {
        match &self.backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => Ok(sqlx::query_as::<_, DeadJob>(sql::DEAD_ONE_PG)
                .bind(id)
                .fetch_optional(pool)
                .await?),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => Ok(sqlx::query_as::<_, DeadJob>(sql::DEAD_ONE_SQLITE)
                .bind(id)
                .fetch_optional(pool)
                .await?),
        }
    }

    /// Puts a dead job back on its queue with a fresh attempt count.
    ///
    /// Returns `None` when the dead row is gone, or when a live job already
    /// holds its unique key — in which case the dead row stays where it is,
    /// because deleting it would lose the record without replaying it.
    pub(crate) async fn requeue_dead(&self, dead_id: i64, now: i64) -> Result<Option<JobId>> {
        let out = transaction!(&self.backend, |tx, dialect| {
            let id: Option<i64> = sqlx::query(sql::dead_requeue(dialect))
                .bind(now)
                .bind(now)
                .bind(dead_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(|r| r.try_get::<i64, _>(0))
                .transpose()?;
            if id.is_some() {
                sqlx::query(sql::dead_delete(dialect))
                    .bind(dead_id)
                    .execute(&mut *tx)
                    .await?;
            }
            id.map(JobId)
        });
        Ok(out)
    }

    pub(crate) async fn discard_dead(&self, dead_id: i64) -> Result<u64> {
        Ok(execute!(
            &self.backend,
            sql::DEAD_DELETE_PG,
            sql::DEAD_DELETE_SQLITE,
            dead_id
        ))
    }

    // -- observability ------------------------------------------------------

    pub(crate) async fn stats(&self, now: i64) -> Result<Stats> {
        // Both arms reduce to the same tuple here rather than handing back a
        // row: a `PgRow` and a `SqliteRow` are different types, so a `match`
        // that yields rows does not compile once both drivers are enabled.
        type StatTuple = (String, i64, i64, i64, Option<i64>);
        let rows: Vec<StatTuple> = match &self.backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => sqlx::query(sql::STATS_PG)
                .bind(now)
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(|r| {
                    Ok::<StatTuple, Error>((
                        r.try_get("queue")?,
                        r.try_get("ready")?,
                        r.try_get("scheduled")?,
                        r.try_get("running")?,
                        r.try_get("oldest_ready")?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?,
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => sqlx::query(sql::STATS_SQLITE)
                .bind(now)
                .bind(now)
                .bind(now)
                .fetch_all(pool)
                .await?
                .into_iter()
                .map(|r| {
                    Ok::<StatTuple, Error>((
                        r.try_get("queue")?,
                        r.try_get("ready")?,
                        r.try_get("scheduled")?,
                        r.try_get("running")?,
                        r.try_get("oldest_ready")?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?,
        };

        let mut stats = Stats {
            ready: 0,
            scheduled: 0,
            running: 0,
            dead: 0,
            oldest_ready_age: None,
            queues: Vec::with_capacity(rows.len()),
        };
        for (queue, ready, scheduled, running, oldest) in rows {
            let age = oldest.map(|at| Duration::from_millis(now.saturating_sub(at).max(0) as u64));
            stats.ready += ready.max(0) as u64;
            stats.scheduled += scheduled.max(0) as u64;
            stats.running += running.max(0) as u64;
            stats.oldest_ready_age = match (stats.oldest_ready_age, age) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            stats.queues.push(QueueDepth {
                queue,
                ready: ready.max(0) as u64,
                scheduled: scheduled.max(0) as u64,
                running: running.max(0) as u64,
                oldest_ready_age: age,
            });
        }

        let dead: i64 = match &self.backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => sqlx::query(sql::DEAD_COUNT)
                .fetch_one(pool)
                .await?
                .try_get(0)?,
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => sqlx::query(sql::DEAD_COUNT)
                .fetch_one(pool)
                .await?
                .try_get(0)?,
        };
        stats.dead = dead.max(0) as u64;
        Ok(stats)
    }

    /// One job's row, for a test or an admin screen. `None` once it has
    /// finished.
    pub(crate) async fn job_state(&self, id: JobId) -> Result<Option<(String, i64, i64)>> {
        match &self.backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => sqlx::query(sql::JOB_STATE_PG)
                .bind(id.0)
                .fetch_optional(pool)
                .await?
                .map(|r| {
                    Ok::<_, Error>((
                        r.try_get::<String, _>("state")?,
                        r.try_get::<i64, _>("attempts")?,
                        r.try_get::<i64, _>("run_at")?,
                    ))
                })
                .transpose(),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => sqlx::query(sql::JOB_STATE_SQLITE)
                .bind(id.0)
                .fetch_optional(pool)
                .await?
                .map(|r| {
                    Ok::<_, Error>((
                        r.try_get::<String, _>("state")?,
                        r.try_get::<i64, _>("attempts")?,
                        r.try_get::<i64, _>("run_at")?,
                    ))
                })
                .transpose(),
        }
    }

    // -- schedules ----------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn upsert_schedule(
        &self,
        name: &str,
        cron: &str,
        job_name: &str,
        queue: &str,
        payload: &str,
        priority: i64,
        max_attempts: i64,
        next_run_at: i64,
        misfire: &str,
        grace_ms: i64,
    ) -> Result<()> {
        execute!(
            &self.backend,
            sql::SCHEDULE_UPSERT_PG,
            sql::SCHEDULE_UPSERT_SQLITE,
            name,
            cron,
            job_name,
            queue,
            payload,
            priority,
            max_attempts,
            next_run_at,
            misfire,
            grace_ms
        );
        Ok(())
    }

    pub(crate) async fn due_schedules(&self, now: i64) -> Result<Vec<ScheduleRow>> {
        match &self.backend {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => Ok(sqlx::query_as::<_, ScheduleRow>(sql::SCHEDULE_DUE_PG)
                .bind(now)
                .fetch_all(pool)
                .await?),
            #[cfg(feature = "sqlite")]
            Backend::Sqlite(pool) => Ok(sqlx::query_as::<_, ScheduleRow>(sql::SCHEDULE_DUE_SQLITE)
                .bind(now)
                .fetch_all(pool)
                .await?),
        }
    }

    pub(crate) async fn advance_schedule(
        &self,
        name: &str,
        next_run_at: i64,
        fired_at: Option<i64>,
        missed: i64,
    ) -> Result<()> {
        execute!(
            &self.backend,
            sql::SCHEDULE_ADVANCE_PG,
            sql::SCHEDULE_ADVANCE_SQLITE,
            next_run_at,
            fired_at,
            missed,
            name
        );
        Ok(())
    }

    pub(crate) async fn remove_schedule(&self, name: &str) -> Result<u64> {
        Ok(execute!(
            &self.backend,
            sql::SCHEDULE_DELETE_PG,
            sql::SCHEDULE_DELETE_SQLITE,
            name
        ))
    }
}

/// The DDL, which [`crate::Queue::schema_sql`] also hands out as text.
/// The advisory-lock key [`Store::migrate`] holds on Postgres. Arbitrary, and
/// fixed so that every process creating these tables picks the same one.
#[cfg(feature = "postgres")]
const MIGRATION_LOCK: i64 = 0x6272_6f6a_6f62; // "brojob"

pub(crate) fn schema(dialect: Dialect) -> &'static [&'static str] {
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => sql::SCHEMA_PG,
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => sql::SCHEMA_SQLITE,
    }
}

pub(crate) mod sql;
