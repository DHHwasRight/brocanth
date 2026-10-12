//! A job: a type, its payload, and what it declares about how it should run.
//!
//! # Enqueuing is type-checked
//!
//! A job *is* its payload. `queue.enqueue(SendWelcome { user_id: 7 }).await?`
//! takes the job value, so there is no separate "payload" argument that could
//! be the wrong shape, and handing `SendWelcome` a field it does not have is a
//! field error on a struct literal rather than a run-time decode failure on a
//! worker at three in the morning. The JSON in the database is an encoding
//! detail between [`Job::perform`] and the enqueue site, and both ends are the
//! same Rust type.
//!
//! ```
//! use brocanth_jobs::Job;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct SendWelcome {
//!     user_id: i64,
//! }
//!
//! impl Job for SendWelcome {
//!     const NAME: &'static str = "send_welcome";
//!     const QUEUE: &'static str = "mailers";
//!     const MAX_ATTEMPTS: u32 = 10;
//!     type Error = std::io::Error;
//!
//!     async fn perform(&self, _cx: &brocanth_jobs::JobContext) -> Result<(), Self::Error> {
//!         let _ = self.user_id;
//!         Ok(())
//!     }
//! }
//! ```
//!
//! # Why a trait and not a derive
//!
//! A derive would have to invent an attribute syntax for queue, priority,
//! attempts and backoff, and then map it onto exactly these associated
//! constants. The constants already read as a declaration, they are checked by
//! the compiler without a macro in between, and the error when one is wrong
//! points at the constant rather than at generated code — which §6.5 of the
//! plan lists as budgeted work for every macro in this project. A proc-macro
//! crate is a dependency, a compile-time cost and a span problem; three `const`
//! lines are none of those.

use std::fmt;
use std::future::Future;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::backoff::Backoff;

/// A unit of background work.
///
/// Everything except [`Job::NAME`], [`Job::Error`] and [`Job::perform`] has a
/// default, and the defaults are the ones a Rails application would get: the
/// `default` queue, no priority preference, five attempts, and exponential
/// backoff with jitter.
pub trait Job: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The name this job is stored under, and the key the worker's registry
    /// looks it up by.
    ///
    /// It outlives the Rust type name: renaming the struct is free, changing
    /// this constant orphans every row already in the table. Pick it once.
    const NAME: &'static str;

    /// Which queue the job goes on.
    ///
    /// Queues exist so one slow kind of work cannot starve another. A worker
    /// subscribes to a list of them.
    const QUEUE: &'static str = "default";

    /// Run order within a queue. **Lower runs first**, like `nice`.
    ///
    /// Ties break by `run_at`, then by insertion order, so a priority of `0`
    /// everywhere gives you plain FIFO.
    const PRIORITY: i32 = 0;

    /// How many times the job may be *started* before it goes to the dead
    /// letter.
    ///
    /// Starts, not failures: a job whose worker is killed without a word has
    /// used an attempt. That is what stops a job that crashes its process from
    /// taking down the whole pool one worker at a time, forever.
    const MAX_ATTEMPTS: u32 = 5;

    /// How long to wait between attempts.
    const BACKOFF: Backoff = Backoff::DEFAULT;

    /// What `perform` can fail with. Only [`fmt::Display`] is required,
    /// because the only thing the queue does with it is write it down.
    type Error: fmt::Display + Send + 'static;

    /// An optional key that makes this job unique **while it is queued**.
    ///
    /// Two enqueues with the same key produce one row: the second is reported
    /// as a duplicate and returns the first one's id. The key is released when
    /// the job finishes, so it means "do not queue this twice", not "never run
    /// this twice in the history of the world". For the second guarantee the
    /// application needs its own record of what it has done, written in the
    /// same transaction as the work — nothing a queue does can replace that,
    /// and a queue that claims otherwise is lying about at-least-once delivery.
    ///
    /// Scope the key to something stable and meaningful:
    /// `Some(format!("invoice:{}", self.invoice_id))`.
    fn unique_key(&self) -> Option<String> {
        None
    }

    /// Do the work.
    ///
    /// Returning `Err` schedules a retry, or the dead letter once the attempts
    /// are spent. Panicking does the same and records the panic message, plus a
    /// backtrace if the application called
    /// [`capture_backtraces`](crate::capture_backtraces).
    ///
    /// The future must be `Send`: the worker pool is multi-threaded. Blocking
    /// work belongs in `tokio::task::spawn_blocking`, and a job that blocks the
    /// thread without it will stall the HTTP handlers sharing the runtime.
    fn perform(&self, cx: &JobContext) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// A job's id. A database primary key, and the handle for everything the queue
/// can do to one row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(pub i64);

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// What a running job knows about itself.
#[derive(Clone, Debug)]
pub struct JobContext {
    pub(crate) id: JobId,
    pub(crate) queue: String,
    pub(crate) name: String,
    pub(crate) attempt: u32,
    pub(crate) max_attempts: u32,
    pub(crate) enqueued_at: chrono::DateTime<chrono::Utc>,
    pub(crate) worker: String,
    pub(crate) shutdown: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl JobContext {
    /// The row this job came from.
    pub fn id(&self) -> JobId {
        self.id
    }

    /// The queue it was claimed from.
    pub fn queue(&self) -> &str {
        &self.queue
    }

    /// Its [`Job::NAME`].
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Which attempt this is, counting from 1.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Its [`Job::MAX_ATTEMPTS`].
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// Whether this is the last attempt before the dead letter.
    pub fn is_last_attempt(&self) -> bool {
        self.attempt >= self.max_attempts
    }

    /// When the row was first created.
    pub fn enqueued_at(&self) -> chrono::DateTime<chrono::Utc> {
        self.enqueued_at
    }

    /// The id of the worker running it.
    pub fn worker(&self) -> &str {
        &self.worker
    }

    /// Whether the worker has been asked to stop.
    ///
    /// A long job that checks this can finish early and return `Ok`, which is
    /// strictly better than being abandoned at the shutdown deadline and run
    /// again from the start by somebody else.
    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// How a job failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureKind {
    /// `perform` returned `Err`.
    Returned,
    /// `perform` panicked.
    Panicked,
    /// `perform` was still running when the worker's per-job timeout expired.
    TimedOut,
    /// The stored payload would not deserialise into the job type.
    ///
    /// Always terminal: a payload that cannot be decoded will not decode on the
    /// next attempt either, so this goes straight to the dead letter where you
    /// can read it and decide. This is the case that a queue which simply
    /// retries turns into an infinite loop.
    Undecodable,
    /// The worker holding the job stopped sending heartbeats and the job was
    /// reclaimed, but it had no attempts left.
    Orphaned,
}

impl FailureKind {
    /// Whether this kind of failure is worth another attempt.
    pub fn is_retryable(self) -> bool {
        !matches!(self, FailureKind::Undecodable)
    }

    /// The word written into the dead letter.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureKind::Returned => "returned",
            FailureKind::Panicked => "panicked",
            FailureKind::TimedOut => "timed out",
            FailureKind::Undecodable => "undecodable",
            FailureKind::Orphaned => "orphaned",
        }
    }
}

impl fmt::Display for FailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One failed attempt, as the queue records it.
#[derive(Clone, Debug)]
pub struct JobFailure {
    /// What kind of failure it was.
    pub kind: FailureKind,
    /// The error's `Display`, or the panic's message.
    pub message: String,
    /// A backtrace, when one was captured.
    ///
    /// Present for panics if the application called
    /// [`capture_backtraces`](crate::capture_backtraces). Absent for returned
    /// errors: reading a backtrace out of an arbitrary `Display` error needs
    /// `std::error::request_ref`, which is still unstable, and this crate will
    /// not require `Error: std::error::Error` for a field it can only sometimes
    /// fill.
    pub backtrace: Option<String>,
}

impl JobFailure {
    /// A failure from a returned error.
    pub fn returned(message: impl Into<String>) -> Self {
        Self {
            kind: FailureKind::Returned,
            message: message.into(),
            backtrace: None,
        }
    }

    /// A failure of some other kind.
    pub fn of(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            backtrace: None,
        }
    }

    /// Attaches a backtrace.
    #[must_use]
    pub fn with_backtrace(mut self, backtrace: Option<String>) -> Self {
        self.backtrace = backtrace;
        self
    }
}

impl fmt::Display for JobFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}
