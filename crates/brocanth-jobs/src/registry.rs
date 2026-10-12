//! The map from a job name on a row to the code that runs it.
//!
//! It is a value, not a `static`. The Crystal original kept its registries in
//! class variables, so two applications in one process collided and its own
//! test suite needed a reset between cases; PLAN.md §3 lists that as one of the
//! three workarounds that become compile errors here. Two queues in one process
//! — a test harness and the application under test, say — each hold their own
//! [`JobRegistry`] and cannot see each other's.
//!
//! A job name that is not registered is an [`Error::UnknownJob`], reported with
//! the list of names that *are* registered. Guessing would mean a job that
//! silently does nothing, which is how a queue goes quiet without anybody
//! noticing.
//!
//! [`Error::UnknownJob`]: crate::Error::UnknownJob

use std::collections::HashMap;
use std::future::Future;
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use crate::backoff::Backoff;
use crate::job::{FailureKind, Job, JobContext, JobFailure};

type Handler = Arc<
    dyn Fn(String, JobContext) -> Pin<Box<dyn Future<Output = Result<(), JobFailure>> + Send>>
        + Send
        + Sync,
>;

/// What the registry remembers about one job type.
#[derive(Clone)]
struct Entry {
    handler: Handler,
    backoff: Backoff,
}

/// The job types a worker can run.
#[derive(Clone, Default)]
pub struct JobRegistry {
    entries: HashMap<&'static str, Entry>,
}

impl JobRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Teaches the registry one job type.
    ///
    /// Registering the same [`Job::NAME`] twice replaces the first, which is
    /// what you want when a test overrides a job and is never what you want in
    /// production — so [`JobRegistry::names`] is there to assert against.
    pub fn register<J: Job>(&mut self) -> &mut Self {
        let handler: Handler = Arc::new(|payload: String, cx: JobContext| {
            Box::pin(async move {
                let job: J = match serde_json::from_str(&payload) {
                    Ok(job) => job,
                    Err(e) => {
                        // Terminal on purpose. A payload that does not decode
                        // will not decode on the next attempt either, and a
                        // queue that retries it has built an infinite loop.
                        return Err(JobFailure::of(
                            FailureKind::Undecodable,
                            format!("payload does not decode into `{}`: {e}", J::NAME),
                        ));
                    }
                };
                match CatchUnwind::new(Box::pin(async move { job.perform(&cx).await })).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(JobFailure::returned(e.to_string())),
                    Err(panic) => Err(JobFailure::of(
                        FailureKind::Panicked,
                        panic_message(panic.as_ref()),
                    )
                    .with_backtrace(crate::panic_hook::take_backtrace())),
                }
            })
        });

        self.entries.insert(
            J::NAME,
            Entry {
                handler,
                backoff: J::BACKOFF,
            },
        );
        self
    }

    /// Whether `name` is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    /// Every registered name, sorted. Useful in an error message and in a test
    /// that asserts the worker knows every job the application enqueues.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.entries.keys().copied().collect();
        names.sort_unstable();
        names
    }

    /// How many job types are registered.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is registered. A worker built on an empty registry
    /// claims nothing, so the worker builder refuses one.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The backoff declared by `name`, or the default if this worker does not
    /// know the job — which happens on the reclaim path, where a worker can be
    /// cleaning up after a worker that ran job types it does not have.
    pub(crate) fn backoff_for(&self, name: &str) -> Backoff {
        self.entries
            .get(name)
            .map(|e| e.backoff)
            .unwrap_or(Backoff::DEFAULT)
    }

    pub(crate) fn handler(&self, name: &str) -> Option<Handler> {
        self.entries.get(name).map(|e| e.handler.clone())
    }
}

impl std::fmt::Debug for JobRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRegistry")
            .field("jobs", &self.names())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Catching a panic in a future, without `futures` and without `unsafe`
// ---------------------------------------------------------------------------

/// `futures::FutureExt::catch_unwind` in twenty lines.
///
/// The inner future is `Pin<Box<_>>`, which is `Unpin`, so this needs no
/// projection and therefore no `unsafe` — the crate is `forbid(unsafe_code)`
/// and pulling in `futures` to catch a panic would put an async runtime's worth
/// of crates in the tree for twenty lines of code.
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

    fn context() -> JobContext {
        JobContext {
            id: crate::JobId(1),
            queue: "default".into(),
            name: "test".into(),
            attempt: 1,
            max_attempts: 3,
            enqueued_at: chrono::Utc::now(),
            worker: "w".into(),
            shutdown: Default::default(),
        }
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
    }

    #[tokio::test]
    async fn a_panicking_job_becomes_a_failure_rather_than_killing_the_worker() {
        let mut r = JobRegistry::new();
        r.register::<Boom>();
        let handler = r.handler("boom").expect("registered");
        let failure = handler("null".to_string(), context())
            .await
            .expect_err("Boom panics");
        assert_eq!(failure.kind, FailureKind::Panicked);
        assert!(failure.message.contains("the sky is falling"));
    }

    #[tokio::test]
    async fn an_undecodable_payload_is_terminal() {
        let mut r = JobRegistry::new();
        r.register::<Noop>();
        let handler = r.handler("noop").expect("registered");
        let failure = handler("{\"not\": \"a unit struct\"}".to_string(), context())
            .await
            .expect_err("the payload is wrong");
        assert_eq!(failure.kind, FailureKind::Undecodable);
        assert!(!failure.kind.is_retryable());
    }
}
