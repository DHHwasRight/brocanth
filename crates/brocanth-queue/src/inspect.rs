//! The read side, for a person rather than a worker.
//!
//! [`Backend`](crate::Backend) has eight methods and not one of them reads.
//! That is on purpose: the contract is written against SQS, which cannot
//! list its own messages, cannot count them exactly and cannot show you the
//! one that keeps failing. Putting "list the queue" in `Backend` would make
//! every adapter answer a question the weakest one has no way to answer.
//!
//! So the screens get their own trait. An adapter that can serve them
//! implements it; one that cannot simply does not, and an application with
//! an SQS backend has no queue panel rather than a broken one. The check is
//! `Option<Arc<dyn DynInspect>>` at the call site, which is a compile-time
//! question and not a capability flag read at request time.
//!
//! # Four states, derived and not stored
//!
//! There is no `state` column anywhere in this crate, and adding one is the
//! mistake the Postgres schema exists to avoid. A job's state is read off
//! the two columns that already decide whether a worker can take it:
//!
//! | [`View`] | `available_at` | `lease_token` | `attempts` |
//! | --- | --- | --- | --- |
//! | [`Ready`](View::Ready) | past | — | under the limit |
//! | [`Running`](View::Running) | future | set | — |
//! | [`Retrying`](View::Retrying) | future | null | above zero |
//! | [`Scheduled`](View::Scheduled) | future | null | zero |
//!
//! [`View::Scheduled`] is the one that is not a single query. A delay
//! shorter than the adapter's promotion horizon stays in the live table with
//! `available_at` in the future; a longer one is held in a separate table
//! until it is nearly due. Both are "scheduled" to the person looking at the
//! screen, so the adapter reads both and the caller never learns there were
//! two.
//!
//! # Every answer is a query
//!
//! Nothing here caches and nothing here is session state. Two processes
//! serving the same screens give the same answers because both read the same
//! rows, and a panel mounted in three replicas behind a load balancer needs
//! no sticky sessions. That is what makes the screens safe to mount inside
//! an application that already scales horizontally.

use std::fmt;
use std::future::Future;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::backend::{BackendError, BoxFuture};
use crate::job::JobId;

/// Which of the four derived states to list.
///
/// See the table in the [module docs](self) for how each one is recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum View {
    /// Claimable right now. A non-zero count here that does not fall is the
    /// only count on the screen that means something is wrong: work is
    /// waiting and no worker took it.
    Ready,
    /// A worker holds a lease. The row carries
    /// [`claimed_by`](JobRow::claimed_by), so this is also the worker
    /// roster: a worker with nothing in flight is not listed, because a
    /// worker that holds no job is not a fact anybody acts on.
    Running,
    /// Failed at least once and waiting out its backoff.
    Retrying,
    /// Has never run and is not due yet.
    Scheduled,
}

impl View {
    /// Every view, in the order the screens are usually read.
    pub const ALL: [View; 4] = [View::Ready, View::Running, View::Retrying, View::Scheduled];

    /// The lowercase name, for a URL segment or a tab key.
    pub fn as_str(self) -> &'static str {
        match self {
            View::Ready => "ready",
            View::Running => "running",
            View::Retrying => "retrying",
            View::Scheduled => "scheduled",
        }
    }
}

impl fmt::Display for View {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The dead letter's own identity.
///
/// Deliberately not a [`JobId`]. The dead letter numbers its rows for
/// itself, so the same integer means a live job in one table and an
/// unrelated corpse in the other. Everything that acts on a dead row takes
/// this type and everything that acts on a live one takes [`JobId`], which
/// makes passing the wrong one a compile error rather than an action on
/// somebody else's job.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeadId(pub String);

impl DeadId {
    /// Borrow the identity as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeadId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// How much of a list to read.
///
/// Offset paging, on tables that change while they are read. That is the
/// right trade here and not a shortcut: every list is ordered so the rows
/// worth reading are first, a person reads the first page, and a row that
/// moves between page four and page five of a dead letter costs nobody
/// anything. Keyset paging would need a different cursor per [`View`],
/// because each one sorts on a different column, and it would buy accuracy
/// on the pages nobody opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    /// How many rows to return. An adapter clamps this; it never returns
    /// more.
    pub limit: u32,
    /// How many to skip.
    pub offset: u64,
}

impl Page {
    /// The largest `limit` an adapter will honour. A screen that wants more
    /// than this wants an export, which is a different tool.
    pub const MAX_LIMIT: u32 = 500;

    /// The first page, `n` rows long.
    pub fn first(n: u32) -> Self {
        Page {
            limit: n.min(Self::MAX_LIMIT),
            offset: 0,
        }
    }

    /// The `n`-row page at `index`, counting from zero.
    pub fn at(index: u64, n: u32) -> Self {
        Page {
            limit: n.min(Self::MAX_LIMIT),
            offset: index * n.min(Self::MAX_LIMIT) as u64,
        }
    }
}

impl Default for Page {
    fn default() -> Self {
        Page::first(50)
    }
}

/// One live job, in whichever state it is.
///
/// One struct for all four [`View`]s rather than one per view. The screens
/// show different columns of it, and a row that moves from `Running` to
/// `Retrying` between two refreshes should not change type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRow {
    /// The job.
    pub id: JobId,
    /// The registered kind.
    pub kind: String,
    /// The queue it is on.
    pub queue: String,
    /// The serialized job, verbatim. This is what makes the dead letter and
    /// the retry set readable instead of opaque, so it is not elided.
    pub payload: Vec<u8>,
    /// Attempts spent. Zero means it has never run.
    pub attempt: u32,
    /// The attempt limit recorded at enqueue.
    pub max_attempts: u32,
    /// When it was first enqueued.
    pub enqueued_at: SystemTime,
    /// When it becomes claimable. For a [`View::Running`] row this is the
    /// lease's expiry, because the lease and the visibility are one column.
    pub available_at: SystemTime,
    /// Which worker holds it, from
    /// [`ClaimRequest::worker`](crate::ClaimRequest::worker). Set only on a
    /// [`View::Running`] row, and `None` on a backend that does not record
    /// it.
    pub claimed_by: Option<String>,
    /// When it was claimed, which is how long it has been running.
    pub claimed_at: Option<SystemTime>,
    /// Why the last attempt failed.
    pub last_error: Option<String>,
}

impl JobRow {
    /// How long this job has been running, from `claimed_at` to now.
    ///
    /// `None` only when the job is not running, or when the backend does
    /// not record the claim time. A claim time in the future reads as zero
    /// rather than as nothing: the backend stamps it with the server's
    /// clock and this reads the caller's, and the two are not the same
    /// clock. Answering `None` there would hide the column on a job that
    /// was claimed a millisecond ago, on any machine running a little
    /// behind the database.
    pub fn running_for(&self) -> Option<Duration> {
        let at = self.claimed_at?;
        Some(
            SystemTime::now()
                .duration_since(at)
                .unwrap_or(Duration::ZERO),
        )
    }
}

/// One job that used up every attempt.
///
/// This is the record that answers "why did that not happen", and it is
/// kept rather than logged because a log line cannot be put back on the
/// queue. [`DynInspect::requeue`] is why `max_attempts` and `unique_key`
/// are here: without them the row describes a job that cannot be rebuilt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadRow {
    /// The dead letter's own id, which is what a requeue or a discard names.
    pub id: DeadId,
    /// The id the job had while it was live, for matching against a log
    /// line. `None` on a row written before the column existed.
    pub job_id: Option<JobId>,
    /// The registered kind.
    pub kind: String,
    /// The queue it was on.
    pub queue: String,
    /// The serialized job. Readable, and what a requeue replays.
    pub payload: Vec<u8>,
    /// How many attempts it used.
    pub attempts: u32,
    /// How many it was allowed. `None` on a row written before the column
    /// existed; a requeue of one of those uses the kind's registered
    /// default.
    pub max_attempts: Option<u32>,
    /// Its deduplication key, if it had one.
    pub unique_key: Option<String>,
    /// The last failure, as text.
    pub failure: String,
    /// When the job was first enqueued.
    pub enqueued_at: SystemTime,
    /// When it was given up on.
    pub died_at: SystemTime,
}

/// The whole queue in one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Claimable right now.
    pub ready: u64,
    /// Held by a worker.
    pub running: u64,
    /// Waiting out a backoff.
    pub retrying: u64,
    /// Not due yet, in either table.
    pub scheduled: u64,
    /// Rows in the dead letter.
    pub dead: u64,
    /// How long the oldest claimable job has been claimable.
    ///
    /// The number to put on a dashboard, and the only one here that
    /// distinguishes a busy queue from a stopped one. Counts rise and fall
    /// for reasons that are not incidents — a burst of work is not a
    /// problem — but a job that has been *ready* for four minutes means no
    /// worker came for it.
    pub oldest_ready: Option<Duration>,
}

/// One queue's share of [`Stats`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueDepth {
    /// The queue's name.
    pub queue: String,
    /// Claimable right now.
    pub ready: u64,
    /// Held by a worker.
    pub running: u64,
    /// Waiting out a backoff.
    pub retrying: u64,
    /// Not due yet.
    pub scheduled: u64,
    /// How long the oldest claimable job on this queue has been claimable.
    pub oldest_ready: Option<Duration>,
}

/// How wide a bucket [`DynInspect::finished`] counts into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    /// One bucket per hour.
    Hour,
    /// One bucket per day.
    Day,
}

impl Bucket {
    /// How many seconds wide this bucket is.
    ///
    /// A width and not a `date_trunc` keyword, because `date_trunc`
    /// truncates in the session's time zone. A caller has to put the
    /// buckets it got back on an axis it generated itself, and the two
    /// only line up if both floor the same number. Seconds since the epoch
    /// is a number both ends agree on; "the start of the hour" is not, in
    /// a session set to a zone offset by half an hour.
    pub fn seconds(self) -> u64 {
        match self {
            Bucket::Hour => 3_600,
            Bucket::Day => 86_400,
        }
    }

    /// The lowercase name, for a URL segment or a label.
    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Hour => "hour",
            Bucket::Day => "day",
        }
    }

    /// Floor an instant to the start of the bucket that contains it.
    ///
    /// The same arithmetic an adapter does in SQL, so a caller can build
    /// the axis for a window that has no rows in it. A chart whose empty
    /// hours are missing rather than zero reads as a quieter day than it
    /// was.
    pub fn floor(self, t: SystemTime) -> SystemTime {
        let secs = t
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs();
        let w = self.seconds();
        std::time::UNIX_EPOCH + Duration::from_secs(secs / w * w)
    }
}

/// How many jobs of one kind finished inside one bucket.
///
/// **A count of settles, not of executions.** Delivery is at least once, so
/// a job that ran, had its effect, then lost its lease and was retried
/// leaves one row. Read it as a lower bound on what ran. The wrong reading
/// — "no row means it did not run" — is exactly the one somebody reaches
/// for during the incident this history exists for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    /// The start of the bucket.
    pub at: SystemTime,
    /// The registered kind.
    pub kind: String,
    /// The queue.
    pub queue: String,
    /// How many finished.
    pub count: u64,
}

/// The ergonomic form of the read side. Adapters implement this.
///
/// The same split as [`Backend`](crate::Backend) and
/// [`DynBackend`](crate::DynBackend), for the same reason: `async fn` in a
/// trait is not object safe, and a panel holds whatever the application
/// chose at boot. A blanket impl gives every implementor the boxed mirror
/// [`DynInspect`] for free.
///
/// Importing this trait and `DynInspect` into one scope makes every call
/// ambiguous. Import one.
pub trait Inspect: Send + Sync + 'static {
    /// Whether [`finished`](Inspect::finished) returns anything.
    ///
    /// Read once, when the screens are built, so a missing history hides a
    /// chart rather than drawing an empty one. It is false on an adapter
    /// that deletes a job when it succeeds, which is most of them: a
    /// history has to be kept on purpose.
    fn has_history(&self) -> bool {
        false
    }

    /// Count the whole queue. Required.
    fn stats(&self) -> impl Future<Output = Result<Stats, BackendError>> + Send;

    /// Count each queue separately. Required.
    ///
    /// Separate from [`stats`](Inspect::stats) because it is a different
    /// statement with a different cost. A header that wants four numbers
    /// should not pay for a grouping over every queue in the table.
    fn depths(&self) -> impl Future<Output = Result<Vec<QueueDepth>, BackendError>> + Send;

    /// List live jobs in one state, newest or soonest first. Required.
    fn list(
        &self,
        view: View,
        page: Page,
    ) -> impl Future<Output = Result<Vec<JobRow>, BackendError>> + Send;

    /// Read one live job. Required.
    fn job(&self, id: &JobId) -> impl Future<Output = Result<Option<JobRow>, BackendError>> + Send;

    /// List the dead letter, most recently dead first. Required.
    fn dead(&self, page: Page) -> impl Future<Output = Result<Vec<DeadRow>, BackendError>> + Send;

    /// Read one dead row. Required.
    fn dead_one(
        &self,
        id: &DeadId,
    ) -> impl Future<Output = Result<Option<DeadRow>, BackendError>> + Send;

    /// Make a waiting job claimable now. Required.
    ///
    /// Refuses a job a worker is holding, and answers `false`: the lease is
    /// what stops two workers running one job, and a button that can clear
    /// it is a button that causes the thing the lease prevents. A job whose
    /// lease has expired is not held, and this does move it.
    ///
    /// It does not reset `attempts`. The retry budget is the operator's to
    /// spend, and an impatient click should not buy another one.
    fn run_now(&self, id: &JobId) -> impl Future<Output = Result<bool, BackendError>> + Send;

    /// Delete a waiting job without running it. Required.
    ///
    /// Refuses a held job, for the reason [`run_now`](Inspect::run_now)
    /// does: deleting a row from under a running worker leaves the worker
    /// to settle a job that is not there.
    fn discard(&self, id: &JobId) -> impl Future<Output = Result<bool, BackendError>> + Send;

    /// Put a dead job back on its queue with a fresh attempt budget.
    /// Required.
    ///
    /// The dead row is removed. The new job's id is returned, and is not the
    /// old one: this is a new job carrying the old payload, which is what
    /// makes a requeue safe to press twice by accident — the second press
    /// finds nothing.
    fn requeue(
        &self,
        id: &DeadId,
    ) -> impl Future<Output = Result<Option<JobId>, BackendError>> + Send;

    /// Delete a dead job for good. Required.
    fn forget(&self, id: &DeadId) -> impl Future<Output = Result<bool, BackendError>> + Send;

    /// How many jobs finished per bucket since `since`. Optional.
    ///
    /// Answers only when [`has_history`](Inspect::has_history) is on.
    /// Default: [`BackendError::Unsupported`].
    fn finished(
        &self,
        _since: SystemTime,
        _bucket: Bucket,
    ) -> impl Future<Output = Result<Vec<Finished>, BackendError>> + Send {
        async { Err(BackendError::Unsupported("finished")) }
    }
}

/// The object-safe mirror of [`Inspect`]. Do not implement it by hand.
pub trait DynInspect: Send + Sync + 'static {
    /// See [`Inspect::has_history`].
    fn has_history(&self) -> bool;
    /// See [`Inspect::stats`].
    fn stats(&self) -> BoxFuture<'_, Result<Stats, BackendError>>;
    /// See [`Inspect::depths`].
    fn depths(&self) -> BoxFuture<'_, Result<Vec<QueueDepth>, BackendError>>;
    /// See [`Inspect::list`].
    fn list(&self, view: View, page: Page) -> BoxFuture<'_, Result<Vec<JobRow>, BackendError>>;
    /// See [`Inspect::job`].
    fn job<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Option<JobRow>, BackendError>>;
    /// See [`Inspect::dead`].
    fn dead(&self, page: Page) -> BoxFuture<'_, Result<Vec<DeadRow>, BackendError>>;
    /// See [`Inspect::dead_one`].
    fn dead_one<'a>(
        &'a self,
        id: &'a DeadId,
    ) -> BoxFuture<'a, Result<Option<DeadRow>, BackendError>>;
    /// See [`Inspect::run_now`].
    fn run_now<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<bool, BackendError>>;
    /// See [`Inspect::discard`].
    fn discard<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<bool, BackendError>>;
    /// See [`Inspect::requeue`].
    fn requeue<'a>(&'a self, id: &'a DeadId) -> BoxFuture<'a, Result<Option<JobId>, BackendError>>;
    /// See [`Inspect::forget`].
    fn forget<'a>(&'a self, id: &'a DeadId) -> BoxFuture<'a, Result<bool, BackendError>>;
    /// See [`Inspect::finished`].
    fn finished(
        &self,
        since: SystemTime,
        bucket: Bucket,
    ) -> BoxFuture<'_, Result<Vec<Finished>, BackendError>>;
}

impl<T: Inspect> DynInspect for T {
    fn has_history(&self) -> bool {
        Inspect::has_history(self)
    }

    fn stats(&self) -> BoxFuture<'_, Result<Stats, BackendError>> {
        Box::pin(Inspect::stats(self))
    }

    fn depths(&self) -> BoxFuture<'_, Result<Vec<QueueDepth>, BackendError>> {
        Box::pin(Inspect::depths(self))
    }

    fn list(&self, view: View, page: Page) -> BoxFuture<'_, Result<Vec<JobRow>, BackendError>> {
        Box::pin(Inspect::list(self, view, page))
    }

    fn job<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<Option<JobRow>, BackendError>> {
        Box::pin(Inspect::job(self, id))
    }

    fn dead(&self, page: Page) -> BoxFuture<'_, Result<Vec<DeadRow>, BackendError>> {
        Box::pin(Inspect::dead(self, page))
    }

    fn dead_one<'a>(
        &'a self,
        id: &'a DeadId,
    ) -> BoxFuture<'a, Result<Option<DeadRow>, BackendError>> {
        Box::pin(Inspect::dead_one(self, id))
    }

    fn run_now<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<bool, BackendError>> {
        Box::pin(Inspect::run_now(self, id))
    }

    fn discard<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<bool, BackendError>> {
        Box::pin(Inspect::discard(self, id))
    }

    fn requeue<'a>(&'a self, id: &'a DeadId) -> BoxFuture<'a, Result<Option<JobId>, BackendError>> {
        Box::pin(Inspect::requeue(self, id))
    }

    fn forget<'a>(&'a self, id: &'a DeadId) -> BoxFuture<'a, Result<bool, BackendError>> {
        Box::pin(Inspect::forget(self, id))
    }

    fn finished(
        &self,
        since: SystemTime,
        bucket: Bucket,
    ) -> BoxFuture<'_, Result<Vec<Finished>, BackendError>> {
        Box::pin(Inspect::finished(self, since, bucket))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_cannot_ask_for_more_than_the_ceiling() {
        assert_eq!(Page::first(10_000).limit, Page::MAX_LIMIT);
        // The offset is computed from the clamped limit, or page 2 of a
        // clamped list would skip rows nobody ever saw.
        let p = Page::at(2, 10_000);
        assert_eq!(p.limit, Page::MAX_LIMIT);
        assert_eq!(p.offset, 2 * Page::MAX_LIMIT as u64);
    }

    #[test]
    fn every_view_has_its_own_name() {
        let mut names: Vec<&str> = View::ALL.iter().map(|v| v.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), View::ALL.len());
    }
}
