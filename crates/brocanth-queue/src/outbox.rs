//! The outbox: how a transaction hands work to a backend it cannot join.
//!
//! A [`Lane::Transactional`](crate::Lane) job often has to spawn bulk work.
//! The bulk backend has no way to take part in the caller's transaction, so
//! the framework writes a row in that transaction and a relay moves it
//! afterwards.
//!
//! # The guarantee
//!
//! At-least-once, and nothing more. The relay reads a row, writes it to the
//! backend and then deletes the row. A crash between the backend's
//! acknowledgement and the delete redelivers the job. Every job in the
//! `Bulk` lane is idempotent by contract, which is what pays for this.
//!
//! The guarantee the outbox **does** add is that a job is never delivered
//! for a transaction that rolled back, and never lost for one that
//! committed. That is the whole reason it exists.
//!
//! # When the backend is down
//!
//! The relay backs off and the table grows. Undelivered rows are the only
//! honest signal, so the relay reports a count and the writer checks it:
//! past `max_pending`, [`TxEnqueue`](crate::TxEnqueue) fails the enqueue and
//! takes the caller's transaction down with it. Accepting work that cannot
//! be delivered turns a visible outage into a silent backlog, and the
//! backlog is discovered when the disk fills. The behaviour is configurable
//! because a few applications would rather accept the write.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::backend::{DynBackend, NewJob, Pushed};

/// What the outbox store can fail with.
#[derive(Debug, thiserror::Error)]
pub enum OutboxError {
    /// The store could not be reached.
    #[error("outbox unavailable: {0}")]
    Unavailable(String),
    /// Anything else.
    #[error("outbox error: {0}")]
    Other(String),
}

/// One undelivered row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxRow {
    /// The row's identity in the outbox table, not in the backend.
    pub id: String,
    /// The job to deliver.
    pub job: NewJob,
    /// How many delivery attempts this row has had.
    pub attempts: u32,
}

/// The read side of the outbox. The write side is
/// [`TxEnqueue`](crate::TxEnqueue), which runs in the caller's transaction.
pub trait Outbox: Send + Sync + 'static {
    /// Take up to `limit` undelivered rows and hold them for `lease`.
    ///
    /// Several relays may run. An implementation must not hand the same row
    /// to two of them inside the lease, or the duplicate rate stops being
    /// "on crash" and becomes "always".
    fn fetch(
        &self,
        limit: usize,
        lease: Duration,
    ) -> impl Future<Output = Result<Vec<OutboxRow>, OutboxError>> + Send;

    /// Remove rows the backend acknowledged.
    fn delete(&self, ids: &[String]) -> impl Future<Output = Result<(), OutboxError>> + Send;

    /// Record a failed delivery and make the rows claimable again later.
    fn defer(
        &self,
        ids: &[String],
        not_before: Duration,
        error: &str,
    ) -> impl Future<Output = Result<(), OutboxError>> + Send;

    /// How many rows are undelivered. The writer reads it to decide whether
    /// to accept more work.
    fn pending(&self) -> impl Future<Output = Result<u64, OutboxError>> + Send;
}

/// What to do when the outbox is over its limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnOutboxFull {
    /// Fail the enqueue, and with it the caller's transaction. The default.
    /// The application sees the outage at the point that caused it.
    #[default]
    Reject,
    /// Accept the write anyway. For an application that would rather keep
    /// serving and let the backlog grow.
    Accept,
}

/// How the relay runs.
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Rows to move per tick. The whole point of a batch is that one round
    /// trip to the backend amortises the second write, so this should not
    /// be small.
    pub batch: usize,
    /// How long a fetched row is held against other relays.
    pub lease: Duration,
    /// How long to wait after a tick that moved nothing.
    pub idle: Duration,
    /// The first backoff after a delivery failure.
    pub retry_base: Duration,
    /// The ceiling on that backoff.
    pub retry_max: Duration,
    /// Past this many undelivered rows, a bulk enqueue inside a transaction
    /// fails. See [`OnOutboxFull`].
    pub max_pending: u64,
    /// What to do at the limit.
    pub on_full: OnOutboxFull,
    /// How long [`Relay::run`] keeps delivering after it is told to stop.
    ///
    /// A relay that stops the instant it is asked to leaves behind every row
    /// written by the jobs that were finishing at the same time, and those
    /// rows then wait for whichever process starts next. Draining first
    /// costs a bounded amount of shutdown time and empties the common case.
    pub drain: Duration,
}

/// The default [`RelayConfig::max_pending`], and the default limit a
/// transactional enqueue refuses at.
///
/// It is a shared constant because the two halves have to agree: the relay
/// reports the backlog and the writer refuses at the limit, and a writer with
/// no limit at all turns a delivery outage into silent unbounded growth.
pub const DEFAULT_MAX_PENDING: u64 = 100_000;

impl Default for RelayConfig {
    fn default() -> Self {
        RelayConfig {
            batch: 500,
            lease: Duration::from_secs(30),
            idle: Duration::from_millis(200),
            retry_base: Duration::from_millis(250),
            retry_max: Duration::from_secs(30),
            max_pending: DEFAULT_MAX_PENDING,
            on_full: OnOutboxFull::Reject,
            drain: Duration::from_secs(5),
        }
    }
}

/// What one tick did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tick {
    /// Rows the backend accepted.
    pub delivered: usize,
    /// Rows the backend reported as a duplicate. Removed, not an error.
    pub duplicates: usize,
    /// Rows left for a later tick because delivery failed.
    pub deferred: usize,
}

impl Tick {
    /// True when the tick had nothing to do.
    pub fn idle(&self) -> bool {
        self.delivered == 0 && self.duplicates == 0 && self.deferred == 0
    }
}

/// Moves rows from the outbox to the bulk backend.
pub struct Relay<O: Outbox> {
    outbox: Arc<O>,
    backend: Arc<dyn DynBackend>,
    config: RelayConfig,
    failures: u32,
}

impl<O: Outbox> Relay<O> {
    /// Build a relay.
    pub fn new(outbox: Arc<O>, backend: Arc<dyn DynBackend>, config: RelayConfig) -> Self {
        Relay {
            outbox,
            backend,
            config,
            failures: 0,
        }
    }

    /// Move one batch. Returns what it did, so a test can drive the relay
    /// without a timer.
    pub async fn tick(&mut self) -> Result<Tick, OutboxError> {
        let rows = self
            .outbox
            .fetch(self.config.batch, self.config.lease)
            .await?;
        if rows.is_empty() {
            self.failures = 0;
            return Ok(Tick::default());
        }
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        let jobs: Vec<NewJob> = rows.into_iter().map(|r| r.job).collect();

        match self.backend.push(jobs).await {
            Ok(results) => {
                self.failures = 0;
                let mut tick = Tick::default();
                // `push` answers in order, one entry per input. A backend
                // that returns a short vector is broken; treat the missing
                // tail as undelivered rather than deleting it.
                let acked = results.len().min(ids.len());
                for r in &results[..acked] {
                    match r {
                        Pushed::Enqueued(_) => tick.delivered += 1,
                        Pushed::Duplicate => tick.duplicates += 1,
                    }
                }
                self.outbox.delete(&ids[..acked]).await?;
                if acked < ids.len() {
                    tick.deferred = ids.len() - acked;
                    self.outbox
                        .defer(
                            &ids[acked..],
                            self.backoff(),
                            "backend answered fewer rows than it was given",
                        )
                        .await?;
                }
                Ok(tick)
            }
            Err(e) => {
                // Nothing is deleted. Any prefix the backend may have taken
                // is redelivered later, which is what at-least-once means.
                self.failures = self.failures.saturating_add(1);
                let wait = self.backoff();
                self.outbox.defer(&ids, wait, &e.to_string()).await?;
                Ok(Tick {
                    deferred: ids.len(),
                    ..Tick::default()
                })
            }
        }
    }

    fn backoff(&self) -> Duration {
        let steps = self.failures.saturating_sub(1).min(16);
        let mult = 1u64 << steps;
        Duration::from_millis((self.config.retry_base.as_millis() as u64).saturating_mul(mult))
            .min(self.config.retry_max)
    }

    /// How many rows are waiting.
    pub async fn pending(&self) -> Result<u64, OutboxError> {
        self.outbox.pending().await
    }

    /// Deliver what is already in the outbox, then return.
    ///
    /// Stops at the first idle tick, at the first failure, or after
    /// [`RelayConfig::drain`], whichever comes first. A failure ends the
    /// drain rather than retrying it: the backend is down, and the rows are
    /// safe where they are.
    pub async fn drain(&mut self) -> Tick {
        let deadline = tokio::time::Instant::now() + self.config.drain;
        let mut total = Tick::default();
        while tokio::time::Instant::now() < deadline {
            match self.tick().await {
                Ok(t) if t.idle() => break,
                Ok(t) => {
                    total.delivered += t.delivered;
                    total.duplicates += t.duplicates;
                    total.deferred += t.deferred;
                    if t.deferred > 0 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        total
    }

    /// Run until `stop` is set, then drain what is left.
    pub async fn run(&mut self, mut stop: tokio::sync::watch::Receiver<bool>) {
        loop {
            if *stop.borrow() {
                self.drain().await;
                return;
            }
            let wait = match self.tick().await {
                Ok(t) if t.idle() => self.config.idle,
                Ok(t) if t.deferred > 0 => self.backoff(),
                Ok(_) => Duration::ZERO,
                Err(_) => {
                    self.failures = self.failures.saturating_add(1);
                    self.backoff()
                }
            };
            if wait.is_zero() {
                tokio::task::yield_now().await;
                continue;
            }
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = stop.changed() => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{
        Backend, BackendError, Capabilities, ClaimRequest, Claimed, LeaseRef, LeaseState, Settle,
        UniquenessKind,
    };
    use crate::job::JobId;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Mem {
        rows: Mutex<Vec<OutboxRow>>,
        deferred: Mutex<Vec<String>>,
    }

    impl Outbox for Mem {
        async fn fetch(&self, limit: usize, _l: Duration) -> Result<Vec<OutboxRow>, OutboxError> {
            let rows = self.rows.lock().unwrap();
            Ok(rows.iter().take(limit).cloned().collect())
        }
        async fn delete(&self, ids: &[String]) -> Result<(), OutboxError> {
            self.rows.lock().unwrap().retain(|r| !ids.contains(&r.id));
            Ok(())
        }
        async fn defer(&self, ids: &[String], _n: Duration, _e: &str) -> Result<(), OutboxError> {
            self.deferred.lock().unwrap().extend_from_slice(ids);
            Ok(())
        }
        async fn pending(&self) -> Result<u64, OutboxError> {
            Ok(self.rows.lock().unwrap().len() as u64)
        }
    }

    struct Down;

    impl Backend for Down {
        fn name(&self) -> &'static str {
            "down"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::minimal()
        }
        async fn push(&self, _b: Vec<NewJob>) -> Result<Vec<Pushed>, BackendError> {
            Err(BackendError::Unavailable("connection refused".into()))
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

    struct Up;

    impl Backend for Up {
        fn name(&self) -> &'static str {
            "up"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::unbounded()
        }
        async fn push(&self, b: Vec<NewJob>) -> Result<Vec<Pushed>, BackendError> {
            Ok((0..b.len())
                .map(|i| Pushed::Enqueued(JobId(i.to_string())))
                .collect())
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

    fn row(id: &str) -> OutboxRow {
        OutboxRow {
            id: id.into(),
            job: NewJob {
                kind: "k".into(),
                queue: "default".into(),
                payload: b"{}".to_vec(),
                run_at: None,
                max_attempts: 3,
                unique_key: None,
                uniqueness: UniquenessKind::None,
            },
            attempts: 0,
        }
    }

    #[tokio::test]
    async fn a_down_backend_keeps_every_row() {
        let mem = Arc::new(Mem::default());
        mem.rows.lock().unwrap().extend([row("a"), row("b")]);
        let mut relay = Relay::new(mem.clone(), Arc::new(Down), RelayConfig::default());
        let t = relay.tick().await.unwrap();
        assert_eq!(t.deferred, 2);
        assert_eq!(t.delivered, 0);
        assert_eq!(relay.pending().await.unwrap(), 2, "nothing may be dropped");
        assert_eq!(mem.deferred.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn a_delivered_batch_leaves_the_outbox_empty() {
        let mem = Arc::new(Mem::default());
        mem.rows.lock().unwrap().extend([row("a"), row("b")]);
        let mut relay = Relay::new(mem.clone(), Arc::new(Up), RelayConfig::default());
        let t = relay.tick().await.unwrap();
        assert_eq!(t.delivered, 2);
        assert_eq!(relay.pending().await.unwrap(), 0);
        assert!(relay.tick().await.unwrap().idle());
    }

    #[tokio::test]
    async fn the_backoff_grows_and_stops_growing() {
        let mem = Arc::new(Mem::default());
        mem.rows.lock().unwrap().push(row("a"));
        let mut relay = Relay::new(mem, Arc::new(Down), RelayConfig::default());
        let mut last = Duration::ZERO;
        for _ in 0..20 {
            relay.tick().await.unwrap();
            let now = relay.backoff();
            assert!(now >= last);
            last = now;
        }
        assert_eq!(last, RelayConfig::default().retry_max);
    }
}
