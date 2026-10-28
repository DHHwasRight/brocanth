//! What an application writes: a job, its lane, and its retry policy.

use std::borrow::Cow;
use std::fmt;
use std::future::Future;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Which store holds the job, and therefore which guarantee it gets.
///
/// A lane is a failure mode, not a subject. It is also not the call site: a
/// `Bulk` job enqueued inside a transaction goes to the outbox, and a
/// `Transactional` job enqueued outside one is an error. The lane says where
/// the row lives; the call site says whether the write is atomic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// Lives in the application's own database, always. Losing one is a
    /// correctness bug, so it can only be enqueued inside a transaction that
    /// also writes the data it refers to. It never moves to another backend,
    /// which is why it is not served by [`Backend`](crate::Backend) at all.
    Transactional,
    /// Fire and forget, high volume, idempotent by contract. No atomicity.
    /// This is the only lane a [`Backend`](crate::Backend) ever serves and
    /// the only one that migrates.
    Bulk,
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Lane::Transactional => "transactional",
            Lane::Bulk => "bulk",
        })
    }
}

/// How long to wait before attempt `n + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backoff {
    /// The same delay after every failure.
    Fixed(Duration),
    /// `base * factor^attempt`, clamped to `max`, then reduced by up to
    /// `jitter_pct` percent.
    ///
    /// The jitter only subtracts. A backend declares a `max_backoff`, and a
    /// delay that is computed up to the ceiling and then pushed above it by
    /// jitter would be a value the backend cannot express.
    Exponential {
        /// The delay before the second attempt.
        base: Duration,
        /// The multiplier per attempt.
        factor: u32,
        /// The ceiling. A computed delay never exceeds it.
        max: Duration,
        /// How much of the delay to remove, at most, in percent of itself.
        jitter_pct: u32,
    },
}

impl Backoff {
    /// The default: 5 seconds, doubling, up to 6 hours, with 20% jitter.
    pub const fn exponential() -> Self {
        Backoff::Exponential {
            base: Duration::from_secs(5),
            factor: 2,
            max: Duration::from_secs(6 * 60 * 60),
            jitter_pct: 20,
        }
    }

    /// The largest delay this policy can produce. A worker compares it to the
    /// backend's `max_backoff` at boot.
    pub fn ceiling(&self) -> Duration {
        match *self {
            Backoff::Fixed(d) => d,
            Backoff::Exponential { max, .. } => max,
        }
    }

    /// The delay after `attempt` failures, where `attempt` is 1 for the first.
    ///
    /// `seed` supplies the jitter. The worker derives it from the job id and
    /// the attempt, so the function stays pure and the result is reproducible
    /// in a test. The crate takes no random number generator.
    pub fn delay(&self, attempt: u32, seed: u64) -> Duration {
        match *self {
            Backoff::Fixed(d) => d,
            Backoff::Exponential {
                base,
                factor,
                max,
                jitter_pct,
            } => {
                let steps = attempt.saturating_sub(1).min(32);
                let mult = (factor as u64).saturating_pow(steps);
                let raw = base
                    .checked_mul(mult.min(u32::MAX as u64) as u32)
                    .unwrap_or(max)
                    .min(max);
                if jitter_pct == 0 {
                    return raw;
                }
                let pct = jitter_pct.min(100) as u64;
                let span = raw.as_millis() as u64 * pct / 100;
                let cut = if span == 0 { 0 } else { seed % (span + 1) };
                Duration::from_millis(raw.as_millis() as u64 - cut)
            }
        }
    }
}

/// How long one job may hold a name against other enqueues of the same name.
///
/// Only [`Uniqueness::Window`] is implementable everywhere. SQS FIFO
/// deduplicates for 5 minutes and JetStream for a configured window; neither
/// can hold a key until a job succeeds. The worker refuses to boot when a
/// registered job asks for more than the backend declares.
///
/// # Why there is no "until start"
///
/// An earlier draft had a third spelling: hold the key while the job waits
/// and release it when a worker picks the job up, so a retry and a fresh
/// enqueue could coexist. It is not here, for three reasons.
///
/// It cannot be released again. A job that is picked up and then fails holds
/// no key for the whole of its backoff, which on the default schedule is
/// hours, so the "one copy" an application asked for becomes one copy per
/// retry at exactly the moment the job is already misbehaving.
///
/// It bought no portability. It needs the same unbounded key store
/// [`Uniqueness::UntilSuccess`] needs, so a job using it was pinned to
/// Postgres just as hard, for a weaker promise.
///
/// The field removed it. River made the pending states non-removable from a
/// unique specification (PR #590) and Oban now warns at compile time on the
/// partial state lists that expressed it, both because duplicates land in
/// the gaps between states. The systems that still offer it document advice
/// not to use it.
///
/// The case it served is a periodic tick that must not pile up. That belongs
/// to whatever schedules the tick, which can ask "is a live copy present"
/// in one query, on any backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uniqueness {
    /// Enqueue the job every time it is asked for.
    None,
    /// Reject a duplicate until an earlier copy succeeds or dies. Needs an
    /// unbounded key store.
    UntilSuccess,
    /// Reject a duplicate for this long after the enqueue, whatever happens
    /// to the earlier copy.
    Window(Duration),
}

/// A job: a payload plus the code that runs it.
///
/// There is no derive macro. The trait asks for one string and one method,
/// and a macro that writes `const KIND` from the type name would hide the
/// single declaration that must survive a rename. The erasure a registry
/// needs is a generic function, not a macro.
pub trait Job: Serialize + DeserializeOwned + Send + Sync + 'static {
    /// The name on the wire. A worker looks the job up by this string, so it
    /// outlives any rename of the Rust type. Choose it once.
    const KIND: &'static str;

    /// The lane. Defaults to [`Lane::Bulk`], because a job that needs
    /// atomicity should have to say so.
    const LANE: Lane = Lane::Bulk;

    /// The queue name. Workers subscribe to queue names. There is no numeric
    /// priority: Postgres gives one away with `ORDER BY`, SQS cannot do it at
    /// all, and an API that offers it cannot move.
    const QUEUE: &'static str = "default";

    /// How many times to run the job before it goes to the dead letter.
    const MAX_ATTEMPTS: u32 = 25;

    /// The retry schedule.
    const BACKOFF: Backoff = Backoff::exponential();

    /// The uniqueness guarantee the job asks the backend for.
    const UNIQUENESS: Uniqueness = Uniqueness::None;

    /// What a failure of this job is.
    type Error: std::error::Error + Send + Sync + 'static;

    /// The key [`Self::UNIQUENESS`] applies to. `None` means the job is never
    /// deduplicated, whatever the constant says.
    fn unique_key(&self) -> Option<Cow<'_, str>> {
        None
    }

    /// Run the job. Returning `Err` schedules a retry until the attempts run
    /// out. Panicking is treated as the same failure, with the panic message.
    ///
    /// The future can be dropped at any await point, during shutdown or when
    /// the lease is about to expire. A drop is not a failure: the worker
    /// releases the lease without spending an attempt, and another worker
    /// picks the job up. Write the body so a drop leaves no half state, or
    /// watch [`JobContext::is_stopping`].
    fn run(&self, cx: &JobContext) -> impl Future<Output = Result<(), Self::Error>> + Send;
}

/// Lets an application write `job.enqueue(&client)` as well as
/// `client.enqueue(job)`.
pub trait JobExt: Job + Sized {
    /// Start an enqueue for this job. See [`Client::enqueue`](crate::Client::enqueue).
    fn enqueue(self, client: &crate::Client) -> crate::Enqueue<'_, Self> {
        client.enqueue(self)
    }
}

impl<J: Job> JobExt for J {}

/// The identity of one enqueued job, as the backend spells it.
///
/// It is an opaque string because the backends disagree: Postgres has a
/// `bigint`, SQS has a receipt handle, JetStream has a stream sequence.
/// Nothing in the contract orders, parses or arithmetics it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JobId(pub String);

impl JobId {
    /// Borrow the identity as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for JobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for JobId {
    fn from(s: String) -> Self {
        JobId(s)
    }
}

/// What a running job knows about itself.
#[derive(Debug, Clone)]
pub struct JobContext {
    pub(crate) id: JobId,
    pub(crate) kind: String,
    pub(crate) queue: String,
    pub(crate) attempt: u32,
    pub(crate) max_attempts: u32,
    pub(crate) worker: String,
    pub(crate) stopping: tokio::sync::watch::Receiver<bool>,
}

impl JobContext {
    /// The backend's identity for this job.
    pub fn id(&self) -> &JobId {
        &self.id
    }

    /// The job's [`Job::KIND`].
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The queue the job was claimed from.
    pub fn queue(&self) -> &str {
        &self.queue
    }

    /// Which attempt this is, counting from 1.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The attempt limit in force for this job.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// True when a failure now sends the job to the dead letter.
    pub fn is_last_attempt(&self) -> bool {
        self.attempt >= self.max_attempts
    }

    /// The name of the worker process.
    pub fn worker(&self) -> &str {
        &self.worker
    }

    /// True once the worker started a graceful stop.
    ///
    /// A long job should check this and return early. A job that ignores it
    /// is dropped at the grace deadline, and its lease is released.
    pub fn is_stopping(&self) -> bool {
        *self.stopping.borrow()
    }
}

/// Why a job did not finish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobFailure {
    /// A short machine-readable class.
    pub kind: FailureKind,
    /// The message to record.
    pub message: String,
}

impl JobFailure {
    /// A failure the job returned.
    pub fn returned(message: impl Into<String>) -> Self {
        JobFailure {
            kind: FailureKind::Returned,
            message: message.into(),
        }
    }

    /// A failure from a panic inside the job.
    pub fn panicked(message: impl Into<String>) -> Self {
        JobFailure {
            kind: FailureKind::Panicked,
            message: message.into(),
        }
    }

    /// The payload did not deserialize into the registered type.
    pub fn undecodable(message: impl Into<String>) -> Self {
        JobFailure {
            kind: FailureKind::Undecodable,
            message: message.into(),
        }
    }
}

impl fmt::Display for JobFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

/// The class of a [`JobFailure`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// The job returned `Err`.
    Returned,
    /// The job panicked.
    Panicked,
    /// The payload did not match the registered type. Retrying cannot help,
    /// so the job goes straight to the dead letter.
    Undecodable,
}

impl FailureKind {
    /// False when another attempt cannot change the result.
    pub fn is_retryable(self) -> bool {
        !matches!(self, FailureKind::Undecodable)
    }
}

impl fmt::Display for FailureKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            FailureKind::Returned => "returned",
            FailureKind::Panicked => "panicked",
            FailureKind::Undecodable => "undecodable",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exponential_backoff_doubles_and_clamps() {
        let b = Backoff::Exponential {
            base: Duration::from_secs(1),
            factor: 2,
            max: Duration::from_secs(10),
            jitter_pct: 0,
        };
        assert_eq!(b.delay(1, 0), Duration::from_secs(1));
        assert_eq!(b.delay(2, 0), Duration::from_secs(2));
        assert_eq!(b.delay(3, 0), Duration::from_secs(4));
        assert_eq!(b.delay(9, 0), Duration::from_secs(10));
        assert_eq!(b.delay(u32::MAX, 0), Duration::from_secs(10));
    }

    #[test]
    fn jitter_only_subtracts_so_the_ceiling_holds() {
        let b = Backoff::Exponential {
            base: Duration::from_secs(10),
            factor: 2,
            max: Duration::from_secs(10),
            jitter_pct: 20,
        };
        for seed in 0..500 {
            let d = b.delay(4, seed);
            assert!(d <= b.ceiling(), "{d:?} went over the ceiling");
            assert!(d >= Duration::from_secs(8), "{d:?} cut more than 20%");
        }
    }
}
