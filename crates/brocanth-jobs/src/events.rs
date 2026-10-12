//! Observability, without choosing anybody's logging framework.
//!
//! PLAN.md §8 bans a logging framework from the core and the same reasoning
//! applies here: a queue that calls `tracing::info!` has decided that your
//! application links `tracing`, and `apalis` 1.0 makes exactly that decision in
//! its default feature set. So this crate emits [`JobEvent`] values and does
//! nothing with them. An application that wants them written down installs
//! three lines:
//!
//! ```
//! use brocanth_jobs::{JobEvent, JobEvents};
//!
//! struct ToStderr;
//! impl JobEvents for ToStderr {
//!     fn handle(&self, event: JobEvent<'_>) {
//!         eprintln!("{event}");
//!     }
//! }
//! ```
//!
//! and an application that wants `tracing`, `log`, `slog`, a metrics counter or
//! nothing at all writes the same three lines against its own choice. The
//! default is [`Silent`], which is not a branch that returns early — it is a
//! `handle` with an empty body, which the optimiser deletes.
//!
//! The numbers that matter are not events, though. They are in
//! [`Stats`](crate::Stats), which answers the only question that tells you a
//! queue is in trouble: how old is the oldest thing still waiting.

use std::fmt;
use std::time::Duration;

use crate::job::{JobFailure, JobId};

/// Something the queue did.
///
/// `#[non_exhaustive]`: a future version will add variants, and a `match` in an
/// application should not break when it does.
#[derive(Debug)]
#[non_exhaustive]
pub enum JobEvent<'a> {
    /// A row was inserted.
    Enqueued {
        /// The new row.
        id: JobId,
        /// Its [`Job::NAME`](crate::Job::NAME).
        name: &'a str,
        /// Its queue.
        queue: &'a str,
        /// Whether it will not run until later.
        scheduled: bool,
    },
    /// An enqueue was folded into an existing row by its unique key.
    Deduplicated {
        /// The row that already existed.
        id: JobId,
        /// The job name.
        name: &'a str,
        /// The key that matched.
        unique_key: &'a str,
    },
    /// A worker claimed a job and is about to run it.
    Started {
        /// The row.
        id: JobId,
        /// The job name.
        name: &'a str,
        /// Which attempt, counting from 1.
        attempt: u32,
        /// The worker's id.
        worker: &'a str,
    },
    /// A job returned `Ok` and its row was deleted.
    Completed {
        /// The row, now gone.
        id: JobId,
        /// The job name.
        name: &'a str,
        /// How long `perform` took.
        took: Duration,
        /// Which attempt succeeded.
        attempt: u32,
    },
    /// A job failed and will be tried again.
    Retrying {
        /// The row.
        id: JobId,
        /// The job name.
        name: &'a str,
        /// The attempt that just failed.
        attempt: u32,
        /// How long until the next attempt.
        delay: Duration,
        /// What went wrong.
        failure: &'a JobFailure,
    },
    /// A job ran out of attempts and was moved to the dead letter.
    DeadLettered {
        /// The row, now in `brocanth_dead_jobs`.
        id: JobId,
        /// The job name.
        name: &'a str,
        /// How many attempts it used.
        attempts: u32,
        /// The last thing that went wrong.
        failure: &'a JobFailure,
    },
    /// Jobs held by a worker that stopped heartbeating were put back.
    Reclaimed {
        /// How many rows went back to `pending`.
        requeued: u64,
        /// How many had no attempts left and went to the dead letter.
        dead_lettered: u64,
    },
    /// A recurring schedule fired.
    ScheduleFired {
        /// The schedule's name.
        schedule: &'a str,
        /// The occurrence it fired for.
        occurrence: chrono::DateTime<chrono::Utc>,
        /// How many occurrences were passed over, because the worker was down
        /// across them. See [`Misfire`](crate::Misfire).
        missed: u32,
    },
    /// A worker began polling.
    WorkerStarted {
        /// Its id.
        worker: &'a str,
        /// The queues it subscribed to.
        queues: &'a [String],
    },
    /// A worker finished shutting down.
    WorkerStopped {
        /// Its id.
        worker: &'a str,
        /// Jobs that finished inside the grace period.
        finished: usize,
        /// Jobs abandoned at the deadline and returned to the queue.
        returned: u64,
    },
    /// Something in the queue's own machinery failed — a poll, a heartbeat, a
    /// reclaim sweep.
    ///
    /// The worker keeps going and tries again on the next tick, because a
    /// database that is briefly unreachable is not a reason to stop processing
    /// forever. This is the event that tells you it happened.
    InternalError {
        /// Which piece.
        during: &'static str,
        /// What it said.
        error: &'a crate::Error,
    },
}

impl fmt::Display for JobEvent<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JobEvent::Enqueued {
                id,
                name,
                queue,
                scheduled,
            } => write!(
                f,
                "enqueued {name} {id} on {queue}{}",
                if *scheduled { " (scheduled)" } else { "" }
            ),
            JobEvent::Deduplicated {
                id,
                name,
                unique_key,
            } => write!(f, "deduplicated {name} onto {id} by key `{unique_key}`"),
            JobEvent::Started {
                id,
                name,
                attempt,
                worker,
            } => write!(f, "started {name} {id} attempt {attempt} on {worker}"),
            JobEvent::Completed {
                id,
                name,
                took,
                attempt,
            } => write!(f, "completed {name} {id} on attempt {attempt} in {took:?}"),
            JobEvent::Retrying {
                id,
                name,
                attempt,
                delay,
                failure,
            } => write!(
                f,
                "retrying {name} {id} after attempt {attempt} in {delay:?}: {failure}"
            ),
            JobEvent::DeadLettered {
                id,
                name,
                attempts,
                failure,
            } => write!(
                f,
                "dead-lettered {name} {id} after {attempts} attempts: {failure}"
            ),
            JobEvent::Reclaimed {
                requeued,
                dead_lettered,
            } => write!(
                f,
                "reclaimed {requeued} orphaned jobs, dead-lettered {dead_lettered}"
            ),
            JobEvent::ScheduleFired {
                schedule,
                occurrence,
                missed,
            } => write!(
                f,
                "schedule `{schedule}` fired for {occurrence}, {missed} occurrence(s) missed"
            ),
            JobEvent::WorkerStarted { worker, queues } => {
                write!(f, "worker {worker} polling {}", queues.join(", "))
            }
            JobEvent::WorkerStopped {
                worker,
                finished,
                returned,
            } => write!(
                f,
                "worker {worker} stopped: {finished} finished, {returned} returned to the queue"
            ),
            JobEvent::InternalError { during, error } => {
                write!(f, "queue error during {during}: {error}")
            }
        }
    }
}

/// Where [`JobEvent`]s go.
pub trait JobEvents: Send + Sync + 'static {
    /// Handle one event.
    ///
    /// Called on the worker's own task, so this must not block and must not
    /// panic. A panic here is a panic in the queue's machinery, not in a job,
    /// and it is not caught.
    fn handle(&self, event: JobEvent<'_>);
}

/// The default: events are produced and dropped.
#[derive(Clone, Copy, Debug, Default)]
pub struct Silent;

impl JobEvents for Silent {
    fn handle(&self, _event: JobEvent<'_>) {}
}

/// A closure as an event sink, for the common case.
///
/// ```
/// # use brocanth_jobs::{EventFn, JobEvents};
/// let sink = EventFn::new(|e| eprintln!("{e}"));
/// # let _ = &sink as &dyn JobEvents;
/// ```
pub struct EventFn<F>(F);

impl<F> EventFn<F>
where
    F: Fn(JobEvent<'_>) + Send + Sync + 'static,
{
    /// Wraps `f`.
    pub fn new(f: F) -> Self {
        Self(f)
    }
}

impl<F> JobEvents for EventFn<F>
where
    F: Fn(JobEvent<'_>) + Send + Sync + 'static,
{
    fn handle(&self, event: JobEvent<'_>) {
        (self.0)(event);
    }
}
