//! The map from a job name on a row to the code that runs it.
//!
//! It is a value, not a `static`. The Crystal original kept its registries in
//! class variables, so two applications in one process collided and its own
//! test suite needed a reset between cases; PLAN.md §3 lists that as one of the
//! three workarounds that become compile errors here. Two queues in one process
//! — a test harness and the application under test, say — each hold their own
//! [`JobRegistry`] and cannot see each other's.
//!
//! # The engine's registry, built at start
//!
//! The worker under this crate is `brocanth-queue`'s, and so is its registry.
//! A [`JobRegistry`] holds, per job type, a function that builds the engine's
//! entry when a worker starts. The entry's runner is this crate's: it decodes
//! the payload into the job type, runs [`Job::perform`] under the worker's
//! timeout, and turns a panic into a failure with a backtrace. The engine
//! does the rest: the lease, the heartbeat, the retry and the dead letter.
//!
//! A job name that no worker registers is not claimed: the engine filters
//! its claim by the registered names. During a rolling deploy an old worker
//! that meets a new name releases the row without spending an attempt.

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use brocanth_queue::{BoxFuture, Entry, Lane, Uniqueness};

use crate::job::{FailureKind, Job, JobContext, JobFailure};

/// Where a failure's backtrace starts in the message the engine stores.
///
/// The engine's failure is a kind and a message. A backtrace goes at the end
/// of the message after this line, so the dead letter shows it and
/// [`JobFailure::from_queue`] can split it off again.
pub(crate) const BACKTRACE_MARK: &str = "\n\nbacktrace:\n";

/// What a worker tells every runner it builds.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RunEnv {
    /// The per-job timeout, if the worker has one.
    pub(crate) timeout: Option<Duration>,
}

type Build = Arc<dyn Fn(RunEnv) -> Entry + Send + Sync>;

/// The job types a worker can run.
#[derive(Clone, Default)]
pub struct JobRegistry {
    entries: HashMap<&'static str, Build>,
}

impl JobRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a job type under its [`Job::NAME`].
    ///
    /// Registering a second type under the same name replaces the first, so
    /// the last registration wins, as it did before.
    pub fn register<J: Job>(&mut self) -> &mut Self {
        let build: Build = Arc::new(|env: RunEnv| {
            Entry::custom(
                J::NAME,
                Lane::Bulk,
                J::QUEUE,
                J::MAX_ATTEMPTS,
                J::BACKOFF.to_queue(),
                // A unique key is per job value, not per type, so the type
                // asks for nothing here. The enqueue checks a key against the
                // backend instead.
                Uniqueness::None,
                move |payload: Vec<u8>, cx: brocanth_queue::JobContext| run::<J>(env, payload, cx),
            )
        });
        self.entries.insert(J::NAME, build);
        self
    }

    /// Whether a job of this name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Every registered name, sorted.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.entries.keys().copied().collect();
        names.sort_unstable();
        names
    }

    /// How many job types are registered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The engine's registry, with every runner built for `env`.
    pub(crate) fn build(&self, env: RunEnv) -> brocanth_queue::Registry {
        let mut registry = brocanth_queue::Registry::new();
        for build in self.entries.values() {
            // The map's keys are unique, so a duplicate kind cannot happen.
            let _ = registry.try_add(build(env));
        }
        registry
    }
}

impl std::fmt::Debug for JobRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRegistry")
            .field("jobs", &self.names())
            .finish()
    }
}

/// Runs one job of type `J`: decode, perform, time out, catch a panic.
fn run<J: Job>(
    env: RunEnv,
    payload: Vec<u8>,
    cx: brocanth_queue::JobContext,
) -> BoxFuture<'static, Result<(), brocanth_queue::JobFailure>> {
    Box::pin(async move {
        let job: J = match serde_json::from_slice(&payload) {
            Ok(job) => job,
            Err(e) => {
                return Err(JobFailure::of(
                    FailureKind::Undecodable,
                    format!("payload does not decode into `{}`: {e}", J::NAME),
                )
                .into_queue());
            }
        };
        let cx = JobContext { inner: cx };
        let body = CatchUnwind::new(Box::pin(async move { job.perform(&cx).await }));
        let outcome = match env.timeout {
            Some(limit) => match tokio::time::timeout(limit, body).await {
                Ok(outcome) => outcome,
                Err(_) => {
                    return Err(JobFailure::of(
                        FailureKind::TimedOut,
                        format!("still running after the worker's job timeout of {limit:?}"),
                    )
                    .into_queue());
                }
            },
            None => body.await,
        };
        match outcome {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(JobFailure::returned(e.to_string()).into_queue()),
            Err(panic) => Err(
                JobFailure::of(FailureKind::Panicked, panic_message(panic.as_ref()))
                    .with_backtrace(crate::panic_hook::take_backtrace())
                    .into_queue(),
            ),
        }
    })
}

impl JobFailure {
    /// This failure in the engine's terms. The backtrace goes at the end of
    /// the message, after [`BACKTRACE_MARK`].
    pub(crate) fn into_queue(self) -> brocanth_queue::JobFailure {
        let kind = match self.kind {
            FailureKind::Returned | FailureKind::Orphaned => brocanth_queue::FailureKind::Returned,
            FailureKind::Panicked => brocanth_queue::FailureKind::Panicked,
            FailureKind::TimedOut => brocanth_queue::FailureKind::TimedOut,
            FailureKind::Undecodable => brocanth_queue::FailureKind::Undecodable,
        };
        let message = match self.backtrace {
            Some(trace) => format!("{}{BACKTRACE_MARK}{trace}", self.message),
            None => self.message,
        };
        brocanth_queue::JobFailure { kind, message }
    }

    /// A failure the engine reports, back in this crate's terms.
    pub(crate) fn from_queue(failure: &brocanth_queue::JobFailure) -> Self {
        let kind = match failure.kind {
            brocanth_queue::FailureKind::Returned => FailureKind::Returned,
            brocanth_queue::FailureKind::Panicked => FailureKind::Panicked,
            brocanth_queue::FailureKind::TimedOut => FailureKind::TimedOut,
            brocanth_queue::FailureKind::Undecodable => FailureKind::Undecodable,
        };
        match failure.message.split_once(BACKTRACE_MARK) {
            Some((message, trace)) => {
                JobFailure::of(kind, message).with_backtrace(Some(trace.to_string()))
            }
            None => JobFailure::of(kind, failure.message.clone()),
        }
    }
}

// ---------------------------------------------------------------------------
// Catching a panic inside a future
// ---------------------------------------------------------------------------

/// A future wrapper that turns a panic during `poll` into an `Err`.
///
/// The engine catches a panic too, through the task's `JoinHandle`, but it
/// keeps only the message. Catching it here first, on the thread that
/// panicked, is what lets the panic hook's backtrace travel with it.
struct CatchUnwind<T> {
    inner: Pin<Box<dyn Future<Output = T> + Send>>,
}

impl<T> CatchUnwind<T> {
    fn new(inner: Pin<Box<dyn Future<Output = T> + Send>>) -> Self {
        Self { inner }
    }
}

impl<T> Future for CatchUnwind<T> {
    type Output = Result<T, Box<dyn std::any::Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match std::panic::catch_unwind(AssertUnwindSafe(|| this.inner.as_mut().poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(v)) => Poll::Ready(Ok(v)),
            Err(panic) => Poll::Ready(Err(panic)),
        }
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "panicked with a non-string payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct Noop;

    impl Job for Noop {
        const NAME: &'static str = "noop";
        type Error = std::convert::Infallible;
        async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Serialize, Deserialize)]
    struct Boom;

    impl Job for Boom {
        const NAME: &'static str = "boom";
        type Error = std::convert::Infallible;
        async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
            panic!("the sky is falling");
        }
    }

    #[derive(Serialize, Deserialize)]
    struct Sleepy;

    impl Job for Sleepy {
        const NAME: &'static str = "sleepy";
        type Error = std::convert::Infallible;
        async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
            tokio::time::sleep(Duration::from_secs(3_600)).await;
            Ok(())
        }
    }

    fn context(name: &str) -> brocanth_queue::JobContext {
        brocanth_queue::JobContext::detached(
            brocanth_queue::JobId("1".into()),
            name,
            "default",
            1,
            3,
        )
    }

    async fn failure_of(
        registry: &JobRegistry,
        env: RunEnv,
        name: &str,
        payload: &str,
    ) -> brocanth_queue::JobFailure {
        let engine = registry.build(env);
        let entry = engine.get(name).expect("registered");
        entry
            .run(payload.as_bytes().to_vec(), context(name))
            .await
            .expect_err("the job fails")
    }

    #[test]
    fn a_registry_reports_what_it_knows() {
        let mut r = JobRegistry::new();
        assert!(r.is_empty());
        r.register::<Noop>().register::<Boom>();
        assert_eq!(r.names(), vec!["boom", "noop"]);
        assert!(r.contains("noop"));
        assert!(!r.contains("nope"));
        assert_eq!(r.len(), 2);
        assert_eq!(r.build(RunEnv::default()).kinds(), vec!["boom", "noop"]);
    }

    #[tokio::test]
    async fn a_panicking_job_becomes_a_failure_rather_than_killing_the_worker() {
        let mut r = JobRegistry::new();
        r.register::<Boom>();
        let failure = failure_of(&r, RunEnv::default(), "boom", "null").await;
        assert_eq!(failure.kind, brocanth_queue::FailureKind::Panicked);
        assert!(failure.message.contains("the sky is falling"));
    }

    #[tokio::test]
    async fn an_undecodable_payload_is_terminal() {
        let mut r = JobRegistry::new();
        r.register::<Noop>();
        let failure = failure_of(&r, RunEnv::default(), "noop", "{\"not\": \"a unit\"}").await;
        assert_eq!(failure.kind, brocanth_queue::FailureKind::Undecodable);
        assert!(!failure.kind.is_retryable());
    }

    #[tokio::test]
    async fn a_job_past_the_timeout_fails_as_timed_out() {
        let mut r = JobRegistry::new();
        r.register::<Sleepy>();
        let env = RunEnv {
            timeout: Some(Duration::from_millis(20)),
        };
        let failure = failure_of(&r, env, "sleepy", "null").await;
        assert_eq!(failure.kind, brocanth_queue::FailureKind::TimedOut);
        assert!(failure.kind.is_retryable());
    }

    #[test]
    fn a_backtrace_survives_the_trip_through_the_engine() {
        let failure = JobFailure::of(FailureKind::Panicked, "boom")
            .with_backtrace(Some("frame 1\nframe 2".into()));
        let back = JobFailure::from_queue(&failure.into_queue());
        assert_eq!(back.kind, FailureKind::Panicked);
        assert_eq!(back.message, "boom");
        assert_eq!(back.backtrace.as_deref(), Some("frame 1\nframe 2"));
    }
}
