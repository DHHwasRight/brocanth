//! The contract every adapter implements.
//!
//! The contract is written against the weakest backend this crate intends to
//! support, which is SQS. Anything Postgres can do and SQS cannot is either
//! absent or sits behind a [`Capabilities`] flag that the worker checks at
//! boot. A capability that is read at request time instead of boot time is a
//! migration that fails in production.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::job::{Backoff, JobFailure, JobId, Uniqueness};

/// A boxed future, as the object-safe mirror of the trait returns one.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// What an adapter can fail with.
///
/// The split is the one a worker acts on, not the one a driver reports.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The store could not be reached. The worker backs off and retries the
    /// same call; nothing is lost.
    #[error("backend unavailable: {0}")]
    Unavailable(String),
    /// The call is correct but the backend does not implement it. Always the
    /// answer for a method whose capability is off. Never a panic.
    #[error("backend does not support {0}")]
    Unsupported(&'static str),
    /// The request breaks a declared limit, for example a payload over
    /// `max_payload_bytes`. Retrying the same call cannot help.
    #[error("rejected: {0}")]
    Rejected(String),
    /// The backend is reachable and the call is valid, but it failed.
    #[error("backend error: {0}")]
    Other(String),
}

impl BackendError {
    /// True when calling again with the same arguments could succeed.
    pub fn is_transient(&self) -> bool {
        matches!(self, BackendError::Unavailable(_) | BackendError::Other(_))
    }
}

/// How far a backend can be pushed. Read once, at boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    /// The largest payload the backend accepts. SQS is 256 KiB, JetStream is
    /// 1 MiB by default, Postgres has no useful limit. `None` means
    /// unbounded. The client checks before the write, so an oversized job
    /// fails at the enqueue and not at the delivery.
    pub max_payload_bytes: Option<usize>,
    /// The furthest future a job can be scheduled for. SQS caps a delay at
    /// 900 seconds. `None` means unbounded. A backend that cannot delay at
    /// all reports `Some(Duration::ZERO)`.
    pub max_delay: Option<Duration>,
    /// The longest retry delay the backend can express. SQS tops out at a
    /// 12 hour visibility timeout. `None` means unbounded.
    pub max_backoff: Option<Duration>,
    /// Whether `claim` honours [`ClaimRequest::kinds`]. Postgres can filter
    /// in the `WHERE` clause; SQS has no server-side filter and needs one
    /// queue per dimension. A worker on a backend without it must release
    /// jobs it does not know, which is slower but correct.
    pub filter_by_kind: bool,
    /// The strongest uniqueness the backend offers.
    pub uniqueness: UniquenessSupport,
    /// Whether a pending job can be removed by id. SQS cannot delete a
    /// message it is not holding, so this is off almost everywhere.
    pub cancel_pending: bool,
    /// Whether `approx_depth` is exact. It is approximate on SQS and on
    /// JetStream, so a test must never assert an exact count.
    pub exact_depth: bool,
    /// The most messages that may be leased at one time across all workers.
    /// SQS allows 120,000 in flight on a standard queue.
    pub max_in_flight: Option<u64>,
    /// The most jobs one `claim` call may return. SQS returns 10.
    pub max_claim_batch: usize,
    /// Whether delivery order matches enqueue order. False everywhere this
    /// crate supports, and the `Bulk` lane promises nothing about it. The
    /// flag exists so a test can assert that nothing depends on it.
    pub ordered: bool,
}

impl Capabilities {
    /// The floor: everything optional is off. An adapter starts here and
    /// turns on what it can prove.
    pub const fn minimal() -> Self {
        Capabilities {
            max_payload_bytes: Some(64 * 1024),
            max_delay: Some(Duration::from_secs(900)),
            max_backoff: Some(Duration::from_secs(12 * 60 * 60)),
            filter_by_kind: false,
            uniqueness: UniquenessSupport::None,
            cancel_pending: false,
            exact_depth: false,
            max_in_flight: None,
            max_claim_batch: 10,
            ordered: false,
        }
    }

    /// Everything on and nothing bounded. Only a backend that holds the
    /// queue in the application's own database can claim this.
    pub const fn unbounded() -> Self {
        Capabilities {
            max_payload_bytes: None,
            max_delay: None,
            max_backoff: None,
            filter_by_kind: true,
            uniqueness: UniquenessSupport::Unbounded,
            cancel_pending: true,
            exact_depth: true,
            max_in_flight: None,
            max_claim_batch: 1000,
            ordered: false,
        }
    }

    /// Whether this backend can honour the uniqueness a job asks for.
    pub fn satisfies(&self, want: Uniqueness) -> bool {
        match (want, self.uniqueness) {
            (Uniqueness::None, _) => true,
            (_, UniquenessSupport::Unbounded) => true,
            (Uniqueness::Window(w), UniquenessSupport::Window(have)) => w <= have,
            (Uniqueness::UntilSuccess, _) => false,
            (Uniqueness::Window(_), UniquenessSupport::None) => false,
        }
    }

    /// Whether this backend can express the whole of a retry schedule.
    pub fn allows_backoff(&self, b: &Backoff) -> bool {
        match self.max_backoff {
            None => true,
            Some(cap) => b.ceiling() <= cap,
        }
    }
}

/// The strongest deduplication a backend offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniquenessSupport {
    /// No deduplication. Every enqueue produces a job.
    None,
    /// A key is held for at most this long, whatever happens to the job.
    /// SQS FIFO is 5 minutes; a JetStream stream sets its own.
    Window(Duration),
    /// A key is held until the job reaches a terminal state.
    Unbounded,
}

/// A job on its way into a backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewJob {
    /// The registered [`Job::KIND`](crate::Job::KIND).
    pub kind: String,
    /// The queue to enqueue onto.
    pub queue: String,
    /// The serialized job.
    pub payload: Vec<u8>,
    /// When the job becomes claimable. `None` means at once.
    pub run_at: Option<SystemTime>,
    /// The attempt limit for this job.
    pub max_attempts: u32,
    /// The deduplication key, if the job asked for one.
    pub unique_key: Option<String>,
    /// The deduplication the key asks for.
    pub uniqueness: UniquenessKind,
}

/// The wire form of [`Uniqueness`], flattened so a row can hold it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UniquenessKind {
    /// No key.
    None,
    /// Held until the job is done or dead.
    UntilSuccess,
    /// Held for this many milliseconds after the enqueue.
    Window(u64),
}

impl From<Uniqueness> for UniquenessKind {
    fn from(u: Uniqueness) -> Self {
        match u {
            Uniqueness::None => UniquenessKind::None,
            Uniqueness::UntilSuccess => UniquenessKind::UntilSuccess,
            Uniqueness::Window(d) => UniquenessKind::Window(d.as_millis() as u64),
        }
    }
}

/// What a `push` did with one job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pushed {
    /// The job was accepted and has this identity.
    Enqueued(JobId),
    /// A job with the same unique key was already present, so nothing was
    /// written. Not an error: the caller asked for one copy and there is one.
    Duplicate,
}

/// What a worker asks for when it claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRequest {
    /// The queue to claim from.
    pub queue: String,
    /// How many jobs to take. The backend may return fewer, never more, and
    /// never more than `max_claim_batch`.
    pub max: usize,
    /// How long the lease lasts. The worker extends it while the job runs.
    pub lease: Duration,
    /// The kinds this worker can run. The backend honours it only when
    /// [`Capabilities::filter_by_kind`] is on; otherwise it is a hint and
    /// the worker must handle an unknown kind itself.
    pub kinds: Option<Vec<String>>,
    /// A name for the claiming process, for the admin screens.
    pub worker: String,
}

/// A job a worker now holds a lease on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    /// The lease. Every later call about this job carries it.
    pub lease: LeaseRef,
    /// The registered kind.
    pub kind: String,
    /// The queue it came from.
    pub queue: String,
    /// The serialized job.
    pub payload: Vec<u8>,
    /// Which attempt this is, counting from 1. The backend increments it on
    /// the claim, so a worker that dies silently still spends an attempt and
    /// a poison job cannot loop for ever.
    pub attempt: u32,
    /// The attempt limit recorded at enqueue.
    pub max_attempts: u32,
}

/// Proof that a worker holds a job, and the fence that makes it safe.
///
/// `token` changes on every claim. A worker that stalls past its lease,
/// loses the job to another worker and then wakes up must not settle a job
/// it no longer owns, so every call that changes a job's state carries the
/// token and the backend compares it. Without the token, a garbage collection
/// pause silently deletes another worker's in-progress job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseRef {
    /// The job.
    pub id: JobId,
    /// The fence. Opaque; the backend chooses the spelling. On SQS it is the
    /// receipt handle, which already has this property.
    pub token: String,
    /// When the lease runs out if nothing extends it.
    pub expires_at: SystemTime,
}

/// Whether a worker still owns a job after a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseState {
    /// The call applied. The lease is still held, or the job is settled.
    Held,
    /// The token did not match. Another worker owns the job now, or it is
    /// gone. The call changed nothing. The worker drops its copy and does
    /// not report a failure: the job is someone else's problem and the
    /// duplicate run is what at-least-once delivery means.
    Lost,
}

/// How a worker finishes with a job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settle {
    /// The job succeeded. The backend removes it.
    Done,
    /// The job failed and has attempts left. The backend makes it claimable
    /// again at `at` and records the failure.
    Retry {
        /// When the job becomes claimable again.
        at: SystemTime,
        /// What went wrong.
        failure: JobFailure,
    },
    /// The job failed for the last time, or failed in a way retrying cannot
    /// fix. The backend moves it to the dead letter.
    Dead {
        /// What went wrong.
        failure: JobFailure,
    },
    /// The worker gives the job back without running it to a conclusion, and
    /// **without spending an attempt**.
    ///
    /// This is the shutdown path and the rolling-deploy path. An old worker
    /// that meets a job kind it was built before must release it rather than
    /// fail it, or it burns the retry budget of a job the new workers can
    /// run and dead-letters it before they roll out.
    Release {
        /// When the job becomes claimable again. A release for an unknown
        /// kind sets this a few seconds out so the same worker does not spin
        /// on the same row.
        not_before: SystemTime,
        /// Why the worker let go.
        reason: ReleaseReason,
    },
}

/// Why a worker released a job instead of settling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReason {
    /// The worker has no registered job with that kind.
    UnknownKind,
    /// The worker is stopping and did not finish in the grace period.
    Stopping,
    /// The job future was dropped for another reason.
    Cancelled,
}

/// The ergonomic form of the contract. Adapters implement this.
///
/// `async fn` in a trait is not object safe, and backend selection is a
/// runtime choice, so the worker holds [`DynBackend`] instead. A blanket impl
/// bridges the two: an adapter writes plain `async fn` and gets the boxed
/// mirror for free.
pub trait Backend: Send + Sync + 'static {
    /// A short name for logs and for a conformance report.
    fn name(&self) -> &'static str;

    /// What this backend can do. Called at boot and may be called again; it
    /// must not change while the process runs.
    fn capabilities(&self) -> Capabilities;

    /// Write jobs. Required.
    ///
    /// The result has one entry per input, in the same order. A write that
    /// collides with a live unique key reports [`Pushed::Duplicate`] rather
    /// than an error.
    ///
    /// The batch is not atomic. A partial failure returns `Err`, and the
    /// caller must assume any prefix may have been written: every push is
    /// at-least-once. This is why the only atomic enqueue in the crate is
    /// [`TxEnqueue`](crate::TxEnqueue), which does not go through a backend.
    fn push(
        &self,
        batch: Vec<NewJob>,
    ) -> impl Future<Output = Result<Vec<Pushed>, BackendError>> + Send;

    /// Take an exclusive lease on up to `req.max` claimable jobs. Required.
    ///
    /// Returning an empty vector is normal and means the queue is empty; it
    /// is not an error and the worker backs off. The backend increments each
    /// job's attempt counter as part of the claim.
    ///
    /// The call must not block for longer than it takes to answer. Long
    /// polling belongs in the adapter's own configuration, not in a caller
    /// that holds a semaphore permit.
    fn claim(
        &self,
        req: &ClaimRequest,
    ) -> impl Future<Output = Result<Vec<Claimed>, BackendError>> + Send;

    /// Push the lease's expiry out by `extend` from now. Required.
    ///
    /// Returns [`LeaseState::Lost`] when the token no longer matches. The
    /// worker treats that as a signal to stop: it must not settle the job.
    fn heartbeat(
        &self,
        lease: &LeaseRef,
        extend: Duration,
    ) -> impl Future<Output = Result<LeaseState, BackendError>> + Send;

    /// Finish with a job. Required.
    ///
    /// Must be idempotent for the same token: a worker that settles, loses
    /// the answer to a timeout and settles again must not produce two jobs
    /// or two dead letters.
    fn settle(
        &self,
        lease: &LeaseRef,
        outcome: Settle,
    ) -> impl Future<Output = Result<LeaseState, BackendError>> + Send;

    /// Roughly how many jobs are claimable. Optional.
    ///
    /// Exact only when [`Capabilities::exact_depth`] is on. Default:
    /// [`BackendError::Unsupported`].
    fn approx_depth(&self, _queue: &str) -> impl Future<Output = Result<u64, BackendError>> + Send {
        async { Err(BackendError::Unsupported("approx_depth")) }
    }

    /// Remove a job that has not been claimed. Optional, and off nearly
    /// everywhere: see [`Capabilities::cancel_pending`].
    fn cancel(&self, _id: &JobId) -> impl Future<Output = Result<bool, BackendError>> + Send {
        async { Err(BackendError::Unsupported("cancel")) }
    }

    /// Drop every job in a queue. Optional, and for tests. A backend that
    /// implements it in production is handing an application a footgun.
    fn purge(&self, _queue: &str) -> impl Future<Output = Result<u64, BackendError>> + Send {
        async { Err(BackendError::Unsupported("purge")) }
    }

    /// Do whatever periodic work the store needs: expire leases the backend
    /// does not expire on its own, move exhausted jobs to the dead letter,
    /// drop old partitions. Called on a timer by the worker. A backend that
    /// needs none returns `Ok(())`.
    fn sweep(&self) -> impl Future<Output = Result<(), BackendError>> + Send {
        async { Ok(()) }
    }
}

/// The object-safe mirror of [`Backend`]. Do not implement it by hand.
///
/// Every method boxes its future. That costs one allocation per call on a
/// path that already does a network round trip, which is why the choice
/// between this and a compile-time type parameter is not a performance
/// question. See `DESIGN.md`.
pub trait DynBackend: Send + Sync + 'static {
    /// See [`Backend::name`].
    fn name(&self) -> &'static str;
    /// See [`Backend::capabilities`].
    fn capabilities(&self) -> Capabilities;
    /// See [`Backend::push`].
    fn push(&self, batch: Vec<NewJob>) -> BoxFuture<'_, Result<Vec<Pushed>, BackendError>>;
    /// See [`Backend::claim`].
    fn claim<'a>(
        &'a self,
        req: &'a ClaimRequest,
    ) -> BoxFuture<'a, Result<Vec<Claimed>, BackendError>>;
    /// See [`Backend::heartbeat`].
    fn heartbeat<'a>(
        &'a self,
        lease: &'a LeaseRef,
        extend: Duration,
    ) -> BoxFuture<'a, Result<LeaseState, BackendError>>;
    /// See [`Backend::settle`].
    fn settle<'a>(
        &'a self,
        lease: &'a LeaseRef,
        outcome: Settle,
    ) -> BoxFuture<'a, Result<LeaseState, BackendError>>;
    /// See [`Backend::approx_depth`].
    fn approx_depth<'a>(&'a self, queue: &'a str) -> BoxFuture<'a, Result<u64, BackendError>>;
    /// See [`Backend::cancel`].
    fn cancel<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<bool, BackendError>>;
    /// See [`Backend::purge`].
    fn purge<'a>(&'a self, queue: &'a str) -> BoxFuture<'a, Result<u64, BackendError>>;
    /// See [`Backend::sweep`].
    fn sweep(&self) -> BoxFuture<'_, Result<(), BackendError>>;
}

impl<T: Backend> DynBackend for T {
    fn name(&self) -> &'static str {
        Backend::name(self)
    }

    fn capabilities(&self) -> Capabilities {
        Backend::capabilities(self)
    }

    fn push(&self, batch: Vec<NewJob>) -> BoxFuture<'_, Result<Vec<Pushed>, BackendError>> {
        Box::pin(Backend::push(self, batch))
    }

    fn claim<'a>(
        &'a self,
        req: &'a ClaimRequest,
    ) -> BoxFuture<'a, Result<Vec<Claimed>, BackendError>> {
        Box::pin(Backend::claim(self, req))
    }

    fn heartbeat<'a>(
        &'a self,
        lease: &'a LeaseRef,
        extend: Duration,
    ) -> BoxFuture<'a, Result<LeaseState, BackendError>> {
        Box::pin(Backend::heartbeat(self, lease, extend))
    }

    fn settle<'a>(
        &'a self,
        lease: &'a LeaseRef,
        outcome: Settle,
    ) -> BoxFuture<'a, Result<LeaseState, BackendError>> {
        Box::pin(Backend::settle(self, lease, outcome))
    }

    fn approx_depth<'a>(&'a self, queue: &'a str) -> BoxFuture<'a, Result<u64, BackendError>> {
        Box::pin(Backend::approx_depth(self, queue))
    }

    fn cancel<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<bool, BackendError>> {
        Box::pin(Backend::cancel(self, id))
    }

    fn purge<'a>(&'a self, queue: &'a str) -> BoxFuture<'a, Result<u64, BackendError>> {
        Box::pin(Backend::purge(self, queue))
    }

    fn sweep(&self) -> BoxFuture<'_, Result<(), BackendError>> {
        Box::pin(Backend::sweep(self))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_backend_cannot_serve_until_success() {
        let caps = Capabilities {
            uniqueness: UniquenessSupport::Window(Duration::from_secs(300)),
            ..Capabilities::minimal()
        };
        assert!(caps.satisfies(Uniqueness::None));
        assert!(caps.satisfies(Uniqueness::Window(Duration::from_secs(60))));
        assert!(!caps.satisfies(Uniqueness::Window(Duration::from_secs(600))));
        assert!(!caps.satisfies(Uniqueness::UntilSuccess));
    }

    #[test]
    fn the_default_backoff_does_not_fit_sqs() {
        // Six hours of ceiling against a twelve hour visibility timeout is
        // fine; a day is not. The check is the point, not the numbers.
        let caps = Capabilities::minimal();
        assert!(caps.allows_backoff(&Backoff::exponential()));
        assert!(!caps.allows_backoff(&Backoff::Fixed(Duration::from_secs(24 * 60 * 60))));
        assert!(Capabilities::unbounded()
            .allows_backoff(&Backoff::Fixed(Duration::from_secs(7 * 24 * 60 * 60))));
    }
}
