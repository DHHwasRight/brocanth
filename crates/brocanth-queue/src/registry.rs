//! How a worker turns a kind string and some bytes back into a job.
//!
//! Registration is explicit. `inventory` and `linkme` put a constructor in a
//! link section before `main`, which reads well and fails badly: a job type
//! that lives in a crate nothing else references is dropped by the linker,
//! and the symptom is a job kind that is silently never handled. A framework
//! whose modules are separate crates meets that case on the first module. A
//! generated list of `register::<T>()` calls is greppable, testable, and a
//! generator can write it.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use crate::backend::{BoxFuture, Capabilities};
use crate::job::{Backoff, Job, JobContext, JobFailure, Lane, Uniqueness};

type Runner =
    Arc<dyn Fn(Vec<u8>, JobContext) -> BoxFuture<'static, Result<(), JobFailure>> + Send + Sync>;

/// Everything the worker knows about one registered kind.
#[derive(Clone)]
pub struct Entry {
    /// The kind string.
    pub kind: &'static str,
    /// The lane the job is enqueued onto.
    pub lane: Lane,
    /// The default queue.
    pub queue: &'static str,
    /// The default attempt limit.
    pub max_attempts: u32,
    /// The retry schedule.
    pub backoff: Backoff,
    /// The deduplication the job asks for.
    pub uniqueness: Uniqueness,
    run: Runner,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("kind", &self.kind)
            .field("lane", &self.lane)
            .field("queue", &self.queue)
            .field("max_attempts", &self.max_attempts)
            .finish_non_exhaustive()
    }
}

impl Entry {
    /// Decode the payload and run the job.
    pub fn run(
        &self,
        payload: Vec<u8>,
        cx: JobContext,
    ) -> BoxFuture<'static, Result<(), JobFailure>> {
        (self.run)(payload, cx)
    }
}

/// Two jobs claimed the same [`Job::KIND`].
#[derive(Debug, thiserror::Error)]
#[error("two job types are both registered as {kind:?}; a kind must be unique")]
pub struct DuplicateKind {
    /// The kind both types used.
    pub kind: &'static str,
}

/// The set of job types one process can run.
#[derive(Clone, Default)]
pub struct Registry {
    by_kind: BTreeMap<&'static str, Entry>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.by_kind.keys()).finish()
    }
}

impl Registry {
    /// An empty registry.
    pub fn new() -> Self {
        Registry::default()
    }

    /// Add a job type. Panics on a duplicate kind, because a duplicate is a
    /// programming error that a running process cannot recover from and that
    /// silently routes half the work to the wrong code.
    pub fn register<J: Job>(&mut self) -> &mut Self {
        self.try_register::<J>().expect("duplicate job kind")
    }

    /// Add a job type, reporting a duplicate instead of panicking.
    pub fn try_register<J: Job>(&mut self) -> Result<&mut Self, DuplicateKind> {
        if self.by_kind.contains_key(J::KIND) {
            return Err(DuplicateKind { kind: J::KIND });
        }
        let run: Runner = Arc::new(|payload: Vec<u8>, cx: JobContext| {
            Box::pin(async move {
                let job: J = match serde_json::from_slice(&payload) {
                    Ok(j) => j,
                    Err(e) => return Err(JobFailure::undecodable(e.to_string())),
                };
                job.run(&cx)
                    .await
                    .map_err(|e| JobFailure::returned(e.to_string()))
            })
        });
        self.by_kind.insert(
            J::KIND,
            Entry {
                kind: J::KIND,
                lane: J::LANE,
                queue: J::QUEUE,
                max_attempts: J::MAX_ATTEMPTS,
                backoff: J::BACKOFF,
                uniqueness: J::UNIQUENESS,
                run,
            },
        );
        Ok(self)
    }

    /// Look a kind up. `None` is the rolling-deploy case and is not an error.
    pub fn get(&self, kind: &str) -> Option<&Entry> {
        self.by_kind.get(kind)
    }

    /// Every registered kind, sorted.
    pub fn kinds(&self) -> Vec<String> {
        self.by_kind.keys().map(|k| k.to_string()).collect()
    }

    /// How many kinds are registered.
    pub fn len(&self) -> usize {
        self.by_kind.len()
    }

    /// True when nothing is registered.
    pub fn is_empty(&self) -> bool {
        self.by_kind.is_empty()
    }

    /// Everything the registered jobs need a backend to be able to do.
    pub fn demands(&self) -> Demands {
        Demands {
            uniqueness: self
                .by_kind
                .values()
                .map(|e| (e.kind, e.uniqueness))
                .collect(),
            backoff: self.by_kind.values().map(|e| (e.kind, e.backoff)).collect(),
        }
    }

    /// Compare what the jobs need with what the backend offers.
    ///
    /// Call it at boot and refuse to start on a non-empty result. A
    /// capability mismatch found at request time is a silent behaviour
    /// change: deduplication that stops deduplicating, or a retry schedule
    /// that quietly truncates.
    pub fn check(&self, caps: &Capabilities) -> Vec<Unmet> {
        let mut out = Vec::new();
        for e in self.by_kind.values() {
            if !caps.satisfies(e.uniqueness) {
                out.push(Unmet {
                    kind: e.kind,
                    want: format!("{:?}", e.uniqueness),
                    have: format!("{:?}", caps.uniqueness),
                    what: "uniqueness",
                });
            }
            if !caps.allows_backoff(&e.backoff) {
                out.push(Unmet {
                    kind: e.kind,
                    want: format!("a retry delay up to {:?}", e.backoff.ceiling()),
                    have: match caps.max_backoff {
                        Some(d) => format!("at most {d:?}"),
                        None => "unbounded".into(),
                    },
                    what: "backoff",
                });
            }
        }
        out
    }
}

/// What the registered jobs ask of a backend.
#[derive(Debug, Clone)]
pub struct Demands {
    /// Per kind, the deduplication it wants.
    pub uniqueness: Vec<(&'static str, Uniqueness)>,
    /// Per kind, the retry schedule it wants.
    pub backoff: Vec<(&'static str, Backoff)>,
}

impl Demands {
    /// The longest retry delay any registered job can ask for.
    pub fn max_backoff(&self) -> Option<Duration> {
        self.backoff.iter().map(|(_, b)| b.ceiling()).max()
    }
}

/// One thing a job needs and the backend cannot do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unmet {
    /// The job kind that asked.
    pub kind: &'static str,
    /// Which capability.
    pub what: &'static str,
    /// What the job wants.
    pub want: String,
    /// What the backend offers.
    pub have: String,
}

impl std::fmt::Display for Unmet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "job {:?} needs {} {}, the backend offers {}",
            self.kind, self.what, self.want, self.have
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::UniquenessSupport;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize)]
    struct Strict;

    impl Job for Strict {
        const KIND: &'static str = "test.strict";
        const UNIQUENESS: Uniqueness = Uniqueness::UntilSuccess;
        type Error = std::io::Error;
        async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Serialize, Deserialize)]
    struct Plain;

    impl Job for Plain {
        const KIND: &'static str = "test.plain";
        type Error = std::io::Error;
        async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[test]
    fn boot_refuses_a_backend_that_cannot_hold_the_unique_key() {
        let mut r = Registry::new();
        r.register::<Strict>().register::<Plain>();
        let sqs_ish = Capabilities {
            uniqueness: UniquenessSupport::Window(Duration::from_secs(300)),
            ..Capabilities::minimal()
        };
        let unmet = r.check(&sqs_ish);
        assert_eq!(unmet.len(), 1, "{unmet:?}");
        assert_eq!(unmet[0].kind, "test.strict");
        assert_eq!(unmet[0].what, "uniqueness");
        assert!(r.check(&Capabilities::unbounded()).is_empty());
    }

    #[test]
    fn a_duplicate_kind_is_reported_not_swallowed() {
        let mut r = Registry::new();
        r.register::<Plain>();
        assert!(r.try_register::<Plain>().is_err());
        assert_eq!(r.len(), 1);
    }
}
