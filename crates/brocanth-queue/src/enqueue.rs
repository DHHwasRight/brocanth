//! The write side: a client, a builder, and the one atomic path.

use std::future::{Future, IntoFuture};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::backend::{BackendError, BoxFuture, Capabilities, DynBackend, NewJob, Pushed};
use crate::job::{Job, JobId, Lane, Uniqueness};

/// What an enqueue can fail with.
#[derive(Debug, thiserror::Error)]
pub enum EnqueueError {
    /// A [`Lane::Transactional`] job was enqueued outside a transaction.
    /// There is no correct thing to do: the job exists to commit with the
    /// data it refers to, and there is no transaction to commit with.
    #[error("job {kind:?} is transactional and must be enqueued with in_tx")]
    NeedsTransaction {
        /// The job's kind.
        kind: &'static str,
    },
    /// A [`Lane::Bulk`] job was enqueued into a transaction with a handle
    /// that has no outbox.
    #[error("job {kind:?} is bulk and the transaction handle has no outbox")]
    NoOutbox {
        /// The job's kind.
        kind: &'static str,
    },
    /// The payload is larger than the backend accepts. Caught here so it
    /// fails at the call site, with a stack, and not at delivery.
    #[error("payload for {kind:?} is {size} bytes, the backend accepts {limit}")]
    PayloadTooLarge {
        /// The job's kind.
        kind: &'static str,
        /// The serialized size.
        size: usize,
        /// The declared limit.
        limit: usize,
    },
    /// The requested delay is further out than the backend can express.
    #[error("delay for {kind:?} is {want:?}, the backend allows at most {limit:?}")]
    DelayTooLong {
        /// The job's kind.
        kind: &'static str,
        /// The requested delay.
        want: Duration,
        /// The declared limit.
        limit: Duration,
    },
    /// The job asks for a uniqueness the backend cannot hold.
    #[error("job {kind:?} asks for {want:?}, the backend offers {have:?}")]
    UniquenessUnsupported {
        /// The job's kind.
        kind: &'static str,
        /// What the job asked for.
        want: Uniqueness,
        /// What the backend offers.
        have: crate::backend::UniquenessSupport,
    },
    /// The job did not serialize.
    #[error("could not serialize {kind:?}: {source}")]
    Serialize {
        /// The job's kind.
        kind: &'static str,
        /// The serde error.
        source: serde_json::Error,
    },
    /// The outbox has more undelivered rows than it is allowed to hold. The
    /// transaction should roll back. See `OnOutboxFull` in [`crate::outbox`].
    #[error("the outbox holds {rows} undelivered rows, the limit is {limit}")]
    OutboxFull {
        /// How many rows are waiting.
        rows: u64,
        /// The configured limit.
        limit: u64,
    },
    /// The backend refused or could not be reached.
    #[error(transparent)]
    Backend(#[from] BackendError),
    /// The transactional store refused or could not be reached.
    #[error("transactional store: {0}")]
    Store(String),
}

/// The handle an application enqueues through.
///
/// It holds the bulk backend and the capabilities read from it at
/// construction. It does not hold a transaction: the atomic path takes the
/// caller's transaction as an argument, because the framework must not own
/// the application's unit of work.
#[derive(Clone)]
pub struct Client {
    backend: Arc<dyn DynBackend>,
    caps: Capabilities,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("backend", &self.backend.name())
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Wrap a bulk backend.
    pub fn new(backend: Arc<dyn DynBackend>) -> Self {
        let caps = backend.capabilities();
        Client { backend, caps }
    }

    /// What the bulk backend can do.
    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    /// The bulk backend, for a worker or a relay.
    pub fn backend(&self) -> &Arc<dyn DynBackend> {
        &self.backend
    }

    /// Start an enqueue.
    ///
    /// ```ignore
    /// client.enqueue(SendWelcome { user: id }).await?;
    /// client.enqueue(Sweep).after(Duration::from_secs(3600)).await?;
    /// client.enqueue(ChargeCard { order }).in_tx(&mut tx).await?;
    /// ```
    pub fn enqueue<J: Job>(&self, job: J) -> Enqueue<'_, J> {
        Enqueue {
            client: self,
            job,
            queue: None,
            run_at: None,
            max_attempts: None,
            uniqueness: None,
        }
    }
}

/// An enqueue under construction. Await it to send, or call
/// [`Enqueue::in_tx`] to make it part of a transaction.
pub struct Enqueue<'a, J: Job> {
    client: &'a Client,
    job: J,
    queue: Option<String>,
    run_at: Option<SystemTime>,
    max_attempts: Option<u32>,
    uniqueness: Option<Uniqueness>,
}

impl<'a, J: Job> Enqueue<'a, J> {
    /// Send to a queue other than [`Job::QUEUE`].
    pub fn queue(mut self, q: impl Into<String>) -> Self {
        self.queue = Some(q.into());
        self
    }

    /// Hold the job until this instant.
    pub fn at(mut self, when: SystemTime) -> Self {
        self.run_at = Some(when);
        self
    }

    /// Hold the job for this long.
    pub fn after(self, delay: Duration) -> Self {
        let when = SystemTime::now() + delay;
        self.at(when)
    }

    /// Override [`Job::MAX_ATTEMPTS`] for this one job.
    pub fn max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = Some(n);
        self
    }

    /// Override [`Job::UNIQUENESS`] for this one job.
    pub fn unique(mut self, u: Uniqueness) -> Self {
        self.uniqueness = Some(u);
        self
    }

    fn uniqueness(&self) -> Uniqueness {
        self.uniqueness.unwrap_or(J::UNIQUENESS)
    }

    fn build(&self) -> Result<NewJob, EnqueueError> {
        let payload = serde_json::to_vec(&self.job).map_err(|source| EnqueueError::Serialize {
            kind: J::KIND,
            source,
        })?;
        let uniqueness = self.uniqueness();
        Ok(NewJob {
            kind: J::KIND.to_string(),
            queue: self.queue.clone().unwrap_or_else(|| J::QUEUE.to_string()),
            payload,
            run_at: self.run_at,
            max_attempts: self.max_attempts.unwrap_or(J::MAX_ATTEMPTS),
            unique_key: match uniqueness {
                Uniqueness::None => None,
                _ => self.job.unique_key().map(|k| k.into_owned()),
            },
            uniqueness: uniqueness.into(),
        })
    }

    /// Check the request against what the bulk backend declared. Runs for
    /// the direct path and for the outbox path, because the relay will have
    /// to deliver what the outbox accepts.
    fn check_bulk(&self, new: &NewJob) -> Result<(), EnqueueError> {
        let caps = &self.client.caps;
        if let Some(limit) = caps.max_payload_bytes {
            if new.payload.len() > limit {
                return Err(EnqueueError::PayloadTooLarge {
                    kind: J::KIND,
                    size: new.payload.len(),
                    limit,
                });
            }
        }
        if let (Some(at), Some(limit)) = (new.run_at, caps.max_delay) {
            let want = at
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO);
            if want > limit {
                return Err(EnqueueError::DelayTooLong {
                    kind: J::KIND,
                    want,
                    limit,
                });
            }
        }
        let want = self.uniqueness();
        if new.unique_key.is_some() && !caps.satisfies(want) {
            return Err(EnqueueError::UniquenessUnsupported {
                kind: J::KIND,
                want,
                have: caps.uniqueness,
            });
        }
        Ok(())
    }

    /// Enqueue inside the caller's transaction.
    ///
    /// A [`Lane::Transactional`] job is written to the application's own job
    /// table and commits with the caller's data. A [`Lane::Bulk`] job is
    /// written to the outbox, and a relay moves it to the bulk backend after
    /// the commit. Both are one write in the caller's transaction, so a
    /// rollback takes the job with it.
    pub async fn in_tx<T: TxEnqueue>(self, tx: &mut T) -> Result<Pushed, EnqueueError> {
        let new = self.build()?;
        if J::LANE == Lane::Bulk {
            self.check_bulk(&new)?;
        }
        tx.enqueue_in_tx(J::LANE, new).await
    }
}

impl<'a, J: Job> IntoFuture for Enqueue<'a, J> {
    type Output = Result<Pushed, EnqueueError>;
    type IntoFuture = BoxFuture<'a, Result<Pushed, EnqueueError>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move {
            if J::LANE == Lane::Transactional {
                return Err(EnqueueError::NeedsTransaction { kind: J::KIND });
            }
            let new = self.build()?;
            self.check_bulk(&new)?;
            let mut out = self.client.backend.push(vec![new]).await?;
            Ok(out.pop().unwrap_or(Pushed::Duplicate))
        })
    }
}

/// A transaction that can hold a job write.
///
/// This is deliberately **not** part of [`Backend`](crate::Backend) and
/// deliberately not object safe. "Enqueue inside the caller's transaction"
/// means sharing the caller's connection, and no `dyn` signature can hand a
/// `sqlx::Transaction` to an SQS adapter. A trait that tried would take an
/// opaque handle and downcast, which turns a compile error into a runtime
/// one. So the transactional store is monomorphic over the application's own
/// database, and the backend trait serves only the lane that can move.
pub trait TxEnqueue: Send {
    /// Write one job inside this transaction.
    ///
    /// `lane` picks the destination: [`Lane::Transactional`] goes to the
    /// job table the application's workers read, [`Lane::Bulk`] goes to the
    /// outbox. The implementation owns that mapping because it owns the
    /// schema.
    fn enqueue_in_tx(
        &mut self,
        lane: Lane,
        job: NewJob,
    ) -> impl Future<Output = Result<Pushed, EnqueueError>> + Send;
}

/// A job id produced by a transactional write, for a caller that wants to
/// record it beside the data it committed with.
pub type TxJobId = JobId;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, ClaimRequest, Claimed, LeaseRef, LeaseState, Settle};
    use serde::{Deserialize, Serialize};
    use std::sync::Mutex;

    struct Spy {
        caps: Capabilities,
        pushed: Mutex<Vec<NewJob>>,
    }

    impl Backend for Spy {
        fn name(&self) -> &'static str {
            "spy"
        }
        fn capabilities(&self) -> Capabilities {
            self.caps.clone()
        }
        async fn push(&self, batch: Vec<NewJob>) -> Result<Vec<Pushed>, BackendError> {
            let mut out = Vec::new();
            for (i, j) in batch.into_iter().enumerate() {
                out.push(Pushed::Enqueued(JobId(i.to_string())));
                self.pushed.lock().unwrap().push(j);
            }
            Ok(out)
        }
        async fn claim(&self, _r: &ClaimRequest) -> Result<Vec<Claimed>, BackendError> {
            Ok(Vec::new())
        }
        async fn heartbeat(&self, _l: &LeaseRef, _e: Duration) -> Result<LeaseState, BackendError> {
            Ok(LeaseState::Held)
        }
        async fn settle(&self, _l: &LeaseRef, _o: Settle) -> Result<LeaseState, BackendError> {
            Ok(LeaseState::Held)
        }
    }

    #[derive(Serialize, Deserialize)]
    struct Bulky {
        blob: String,
    }

    impl Job for Bulky {
        const KIND: &'static str = "test.bulky";
        type Error = std::io::Error;
        async fn run(&self, _cx: &crate::JobContext) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Serialize, Deserialize)]
    struct Money;

    impl Job for Money {
        const KIND: &'static str = "test.money";
        const LANE: Lane = Lane::Transactional;
        type Error = std::io::Error;
        async fn run(&self, _cx: &crate::JobContext) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    fn client(caps: Capabilities) -> Client {
        Client::new(Arc::new(Spy {
            caps,
            pushed: Mutex::new(Vec::new()),
        }))
    }

    #[tokio::test]
    async fn a_transactional_job_cannot_be_enqueued_on_its_own() {
        let c = client(Capabilities::unbounded());
        let err = c.enqueue(Money).await.unwrap_err();
        assert!(
            matches!(err, EnqueueError::NeedsTransaction { .. }),
            "{err}"
        );
    }

    #[tokio::test]
    async fn an_oversized_payload_fails_at_the_call_site() {
        let c = client(Capabilities {
            max_payload_bytes: Some(64),
            ..Capabilities::unbounded()
        });
        let err = c
            .enqueue(Bulky {
                blob: "x".repeat(200),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, EnqueueError::PayloadTooLarge { .. }), "{err}");
    }

    #[tokio::test]
    async fn a_delay_past_the_backend_ceiling_fails_at_the_call_site() {
        let c = client(Capabilities {
            max_delay: Some(Duration::from_secs(900)),
            ..Capabilities::unbounded()
        });
        let err = c
            .enqueue(Bulky {
                blob: String::new(),
            })
            .after(Duration::from_secs(3600))
            .await
            .unwrap_err();
        assert!(matches!(err, EnqueueError::DelayTooLong { .. }), "{err}");
        c.enqueue(Bulky {
            blob: String::new(),
        })
        .after(Duration::from_secs(60))
        .await
        .expect("inside the ceiling");
    }
}
