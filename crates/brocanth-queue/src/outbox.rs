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
//!
//! An outage spends no attempt. [`BackendError::Unavailable`] is not the
//! row's fault, so the relay gives the attempt back with
//! [`Outbox::release`]. Otherwise an hour of downtime would empty the outbox
//! into its dead letter.
//!
//! # When one row will never be accepted
//!
//! A payload over the backend's limit is rejected every time. That row must
//! not hold back the rows batched with it, so the relay splits a refused
//! batch and pushes each row alone. Then each row has its own outcome:
//!
//! | The backend answers | The relay |
//! | --- | --- |
//! | accepted, or a duplicate | deletes the row |
//! | [`BackendError::Rejected`] | moves the row to the dead letter at once |
//! | any other error | defers the row on its own backoff, and moves it to the dead letter after [`RelayConfig::max_attempts`] |
//! | [`BackendError::Unavailable`] | releases the row and every row after it |
//!
//! The dead letter is [`OutboxInspect`]. A person can read a row there, put
//! it back in the outbox with a fresh budget, or delete it. It is not the
//! job dead letter: the job never reached the backend, so a requeue must go
//! through the relay again and not straight onto a queue.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::backend::{BackendError, BoxFuture, DynBackend, NewJob, Pushed};
use crate::inspect::{DeadId, DeadRow, Page};

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
    /// How many delivery attempts this row has had, counting the fetch that
    /// returned it.
    pub attempts: u32,
}

/// The read side of the outbox. The write side is
/// [`TxEnqueue`](crate::TxEnqueue), which runs in the caller's transaction.
pub trait Outbox: Send + Sync + 'static {
    /// Take up to `limit` undelivered rows and hold them for `lease`.
    ///
    /// Each fetch spends one attempt on each row it returns. A relay that
    /// dies holding a row has still spent the attempt, so a row that crashes
    /// the relay cannot loop for ever.
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

    /// Record a failed delivery and make the rows claimable again after
    /// `not_before`. The attempt the fetch spent stays spent.
    fn defer(
        &self,
        ids: &[String],
        not_before: Duration,
        error: &str,
    ) -> impl Future<Output = Result<(), OutboxError>> + Send;

    /// Give rows back and make them claimable again after `not_before`,
    /// **without spending an attempt**.
    ///
    /// For a delivery that failed for a reason that is not the row's: the
    /// backend was unreachable. The fetch spent one attempt; this gives it
    /// back.
    fn release(
        &self,
        ids: &[String],
        not_before: Duration,
        error: &str,
    ) -> impl Future<Output = Result<(), OutboxError>> + Send;

    /// Move one row to the outbox's dead letter, with the reason.
    ///
    /// The row leaves the outbox in the same write, so it stops counting
    /// toward [`pending`](Outbox::pending).
    fn bury(&self, id: &str, failure: &str)
        -> impl Future<Output = Result<(), OutboxError>> + Send;

    /// How many rows are undelivered. The writer reads it to decide whether
    /// to accept more work. Rows in the dead letter do not count.
    fn pending(&self) -> impl Future<Output = Result<u64, OutboxError>> + Send;
}

/// The outbox's dead letter, for a person.
///
/// The same four calls as the job dead letter in
/// [`Inspect`](crate::Inspect), and the same row type. It is a separate
/// trait because the store is different: a row here never reached the
/// backend, so a requeue puts it back in the outbox and not on a queue.
///
/// Unlike [`Inspect`](crate::Inspect), every outbox store can serve this,
/// because the outbox is always a table the application owns.
pub trait OutboxInspect: Send + Sync + 'static {
    /// List the dead letter, most recently dead first.
    ///
    /// [`DeadRow::job_id`] is `None`: the row never became a job.
    fn dead(&self, page: Page) -> impl Future<Output = Result<Vec<DeadRow>, OutboxError>> + Send;

    /// Read one dead row.
    fn dead_one(
        &self,
        id: &DeadId,
    ) -> impl Future<Output = Result<Option<DeadRow>, OutboxError>> + Send;

    /// Put a dead row back in the outbox with no attempts spent.
    ///
    /// The dead row is removed in the same write. Returns the new outbox
    /// row's id, or `None` when the dead row is not there, so a second
    /// press of the button finds nothing.
    fn requeue(
        &self,
        id: &DeadId,
    ) -> impl Future<Output = Result<Option<String>, OutboxError>> + Send;

    /// Delete a dead row for good.
    fn forget(&self, id: &DeadId) -> impl Future<Output = Result<bool, OutboxError>> + Send;
}

/// The object-safe mirror of [`OutboxInspect`]. Do not implement it by hand.
pub trait DynOutboxInspect: Send + Sync + 'static {
    /// See [`OutboxInspect::dead`].
    fn dead(&self, page: Page) -> BoxFuture<'_, Result<Vec<DeadRow>, OutboxError>>;
    /// See [`OutboxInspect::dead_one`].
    fn dead_one<'a>(
        &'a self,
        id: &'a DeadId,
    ) -> BoxFuture<'a, Result<Option<DeadRow>, OutboxError>>;
    /// See [`OutboxInspect::requeue`].
    fn requeue<'a>(&'a self, id: &'a DeadId) -> BoxFuture<'a, Result<Option<String>, OutboxError>>;
    /// See [`OutboxInspect::forget`].
    fn forget<'a>(&'a self, id: &'a DeadId) -> BoxFuture<'a, Result<bool, OutboxError>>;
}

impl<T: OutboxInspect> DynOutboxInspect for T {
    fn dead(&self, page: Page) -> BoxFuture<'_, Result<Vec<DeadRow>, OutboxError>> {
        Box::pin(OutboxInspect::dead(self, page))
    }

    fn dead_one<'a>(
        &'a self,
        id: &'a DeadId,
    ) -> BoxFuture<'a, Result<Option<DeadRow>, OutboxError>> {
        Box::pin(OutboxInspect::dead_one(self, id))
    }

    fn requeue<'a>(&'a self, id: &'a DeadId) -> BoxFuture<'a, Result<Option<String>, OutboxError>> {
        Box::pin(OutboxInspect::requeue(self, id))
    }

    fn forget<'a>(&'a self, id: &'a DeadId) -> BoxFuture<'a, Result<bool, OutboxError>> {
        Box::pin(OutboxInspect::forget(self, id))
    }
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
    /// The first backoff after a delivery failure. The relay uses it after
    /// an outage, and each row uses it after its own failures.
    pub retry_base: Duration,
    /// The ceiling on that backoff.
    pub retry_max: Duration,
    /// How many attempts one row gets before the relay moves it to the dead
    /// letter.
    ///
    /// An outage does not spend one. A [`BackendError::Rejected`] row does
    /// not wait for the limit, because the backend said that retrying cannot
    /// help.
    pub max_attempts: u32,
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
            // About ten minutes of retries on the default backoff. A row
            // that fails for that long, while the rows beside it go
            // through, is not waiting out a blip.
            max_attempts: 25,
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
    /// Rows moved to the dead letter.
    pub dead: usize,
}

impl Tick {
    /// True when the tick had nothing to do.
    pub fn idle(&self) -> bool {
        self.delivered == 0 && self.duplicates == 0 && self.deferred == 0 && self.dead == 0
    }

    fn add(&mut self, other: Tick) {
        self.delivered += other.delivered;
        self.duplicates += other.duplicates;
        self.deferred += other.deferred;
        self.dead += other.dead;
    }

    fn deferred(n: usize) -> Tick {
        Tick {
            deferred: n,
            ..Tick::default()
        }
    }

    fn dead() -> Tick {
        Tick {
            dead: 1,
            ..Tick::default()
        }
    }
}

/// Moves rows from the outbox to the bulk backend.
pub struct Relay<O: Outbox> {
    outbox: Arc<O>,
    backend: Arc<dyn DynBackend>,
    config: RelayConfig,
    /// Consecutive outages. Only an unreachable backend counts here; a row
    /// that fails backs off on its own count.
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
        let mut tick = Tick::default();

        // A row past its budget before the push is a row the relay fetched
        // and never settled. Each fetch spends an attempt and every outcome
        // but a crash settles it, so only a relay that died or overran its
        // lease leaves one like this. Pushing it again repeats that.
        let (spent, rows): (Vec<OutboxRow>, Vec<OutboxRow>) = rows
            .into_iter()
            .partition(|r| r.attempts > self.config.max_attempts);
        for r in &spent {
            let why = format!(
                "fetched {} times and never settled: the relay stopped or overran its lease",
                r.attempts
            );
            self.outbox.bury(&r.id, &why).await?;
            tick.dead += 1;
        }
        if rows.is_empty() {
            return Ok(tick);
        }

        let jobs: Vec<NewJob> = rows.iter().map(|r| r.job.clone()).collect();
        match self.backend.push(jobs).await {
            Ok(results) => {
                self.failures = 0;
                // `push` answers in order, one entry per input. A backend
                // that returns a short vector is broken; treat the missing
                // tail as a failed delivery rather than deleting it.
                let acked = results.len().min(rows.len());
                for r in &results[..acked] {
                    count(&mut tick, r);
                }
                let ids: Vec<String> = rows[..acked].iter().map(|r| r.id.clone()).collect();
                self.outbox.delete(&ids).await?;
                for r in &rows[acked..] {
                    let t = self
                        .fail(r, "backend answered fewer rows than it was given")
                        .await?;
                    tick.add(t);
                }
            }
            Err(e @ BackendError::Unavailable(_)) => {
                // Nothing is deleted. Any prefix the backend may have taken
                // is redelivered later, which is what at-least-once means.
                tick.add(self.outage(&rows, &e).await?);
            }
            Err(e) if rows.len() == 1 => {
                self.failures = 0;
                tick.add(self.settle_error(&rows[0], &e).await?);
            }
            Err(_) => {
                // The backend is up and refused the batch. One row is
                // usually the cause, and the error does not say which. A
                // push per row finds it and lets every other row through.
                // Rows the batch already wrote are written again, which
                // at-least-once allows.
                self.failures = 0;
                tick.add(self.isolate(&rows).await?);
            }
        }
        Ok(tick)
    }

    /// Push each row alone, so one bad row has only its own outcome.
    async fn isolate(&mut self, rows: &[OutboxRow]) -> Result<Tick, OutboxError> {
        let mut tick = Tick::default();
        let mut done = Vec::new();
        for (i, r) in rows.iter().enumerate() {
            match self.backend.push(vec![r.job.clone()]).await {
                Ok(res) => match res.first() {
                    Some(p) => {
                        count(&mut tick, p);
                        done.push(r.id.clone());
                    }
                    None => tick.add(self.fail(r, "backend answered no row for one job").await?),
                },
                Err(e @ BackendError::Unavailable(_)) => {
                    // The backend went away during the split. What is left
                    // is not the rows' fault.
                    tick.add(self.outage(&rows[i..], &e).await?);
                    break;
                }
                Err(e) => tick.add(self.settle_error(r, &e).await?),
            }
        }
        self.outbox.delete(&done).await?;
        Ok(tick)
    }

    /// One row came back with an error while the backend was reachable.
    async fn settle_error(
        &mut self,
        row: &OutboxRow,
        e: &BackendError,
    ) -> Result<Tick, OutboxError> {
        match e {
            // The backend said that retrying the same call cannot help.
            BackendError::Rejected(_) => {
                self.outbox.bury(&row.id, &e.to_string()).await?;
                Ok(Tick::dead())
            }
            _ => self.fail(row, &e.to_string()).await,
        }
    }

    /// One row failed and the attempt is spent. Defer it on its own
    /// backoff, or bury it when the budget is gone.
    async fn fail(&mut self, row: &OutboxRow, error: &str) -> Result<Tick, OutboxError> {
        if row.attempts >= self.config.max_attempts {
            let why = format!("gave up after {} attempts: {error}", row.attempts);
            self.outbox.bury(&row.id, &why).await?;
            return Ok(Tick::dead());
        }
        let wait = self.step(row.attempts);
        self.outbox
            .defer(std::slice::from_ref(&row.id), wait, error)
            .await?;
        Ok(Tick::deferred(1))
    }

    /// The backend is unreachable. Give every attempt back and back off the
    /// whole relay.
    async fn outage(&mut self, rows: &[OutboxRow], e: &BackendError) -> Result<Tick, OutboxError> {
        self.failures = self.failures.saturating_add(1);
        let ids: Vec<String> = rows.iter().map(|r| r.id.clone()).collect();
        self.outbox
            .release(&ids, self.backoff(), &e.to_string())
            .await?;
        Ok(Tick::deferred(ids.len()))
    }

    /// The relay's own backoff, after consecutive outages.
    fn backoff(&self) -> Duration {
        self.step(self.failures)
    }

    /// `retry_base`, doubled once per failure after the first, up to
    /// `retry_max`.
    fn step(&self, failures: u32) -> Duration {
        let steps = failures.saturating_sub(1).min(16);
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
    /// Stops at the first idle tick, at the first outage or error, or after
    /// [`RelayConfig::drain`], whichever comes first. An outage ends the
    /// drain rather than retrying it: the backend is down, and the rows are
    /// safe where they are. A row that fails on its own does not end it.
    pub async fn drain(&mut self) -> Tick {
        let deadline = tokio::time::Instant::now() + self.config.drain;
        let mut total = Tick::default();
        while tokio::time::Instant::now() < deadline {
            match self.tick().await {
                Ok(t) if t.idle() => break,
                Ok(t) => {
                    total.add(t);
                    if self.failures > 0 {
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
                // A row that failed on its own already waits on its own
                // backoff and does not slow the others. Only an outage
                // slows the relay.
                Ok(_) if self.failures > 0 => self.backoff(),
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

fn count(tick: &mut Tick, p: &Pushed) {
    match p {
        Pushed::Enqueued(_) => tick.delivered += 1,
        Pushed::Duplicate => tick.duplicates += 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{
        Backend, Capabilities, ClaimRequest, Claimed, LeaseRef, LeaseState, Settle, UniquenessKind,
    };
    use crate::job::JobId;
    use std::sync::Mutex;

    /// Hands out every row on every fetch. Enough for one tick at a time.
    #[derive(Default)]
    struct Mem {
        rows: Mutex<Vec<OutboxRow>>,
        released: Mutex<Vec<String>>,
        buried: Mutex<Vec<String>>,
    }

    impl Outbox for Mem {
        async fn fetch(&self, limit: usize, _l: Duration) -> Result<Vec<OutboxRow>, OutboxError> {
            let mut rows = self.rows.lock().unwrap();
            for r in rows.iter_mut() {
                r.attempts += 1;
            }
            Ok(rows.iter().take(limit).cloned().collect())
        }
        async fn delete(&self, ids: &[String]) -> Result<(), OutboxError> {
            self.rows.lock().unwrap().retain(|r| !ids.contains(&r.id));
            Ok(())
        }
        async fn defer(&self, _i: &[String], _n: Duration, _e: &str) -> Result<(), OutboxError> {
            Ok(())
        }
        async fn release(&self, ids: &[String], _n: Duration, _e: &str) -> Result<(), OutboxError> {
            for r in self.rows.lock().unwrap().iter_mut() {
                if ids.contains(&r.id) {
                    r.attempts -= 1;
                }
            }
            self.released.lock().unwrap().extend_from_slice(ids);
            Ok(())
        }
        async fn bury(&self, id: &str, _f: &str) -> Result<(), OutboxError> {
            self.rows.lock().unwrap().retain(|r| r.id != id);
            self.buried.lock().unwrap().push(id.to_string());
            Ok(())
        }
        async fn pending(&self) -> Result<u64, OutboxError> {
            Ok(self.rows.lock().unwrap().len() as u64)
        }
    }

    /// Unreachable, or up and refusing any batch that holds a `bad` payload.
    struct Sink {
        down: bool,
    }

    impl Backend for Sink {
        fn name(&self) -> &'static str {
            "sink"
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::unbounded()
        }
        async fn push(&self, b: Vec<NewJob>) -> Result<Vec<Pushed>, BackendError> {
            if self.down {
                return Err(BackendError::Unavailable("connection refused".into()));
            }
            if b.iter().any(|j| j.payload == b"bad") {
                return Err(BackendError::Rejected("payload over the limit".into()));
            }
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

    fn row(id: &str, payload: &[u8]) -> OutboxRow {
        OutboxRow {
            id: id.into(),
            job: NewJob {
                kind: "k".into(),
                queue: "default".into(),
                payload: payload.to_vec(),
                run_at: None,
                max_attempts: 3,
                unique_key: None,
                uniqueness: UniquenessKind::None,
            },
            attempts: 0,
        }
    }

    #[tokio::test]
    async fn a_down_backend_keeps_every_row_and_spends_no_attempt() {
        let mem = Arc::new(Mem::default());
        mem.rows
            .lock()
            .unwrap()
            .extend([row("a", b"{}"), row("b", b"{}")]);
        let mut relay = Relay::new(
            mem.clone(),
            Arc::new(Sink { down: true }),
            RelayConfig::default(),
        );
        let t = relay.tick().await.unwrap();
        assert_eq!(t.deferred, 2);
        assert_eq!(t.delivered, 0);
        assert_eq!(relay.pending().await.unwrap(), 2, "nothing may be dropped");
        assert_eq!(mem.released.lock().unwrap().len(), 2);
        assert!(mem.rows.lock().unwrap().iter().all(|r| r.attempts == 0));
    }

    #[tokio::test]
    async fn a_delivered_batch_leaves_the_outbox_empty() {
        let mem = Arc::new(Mem::default());
        mem.rows
            .lock()
            .unwrap()
            .extend([row("a", b"{}"), row("b", b"{}")]);
        let mut relay = Relay::new(
            mem.clone(),
            Arc::new(Sink { down: false }),
            RelayConfig::default(),
        );
        let t = relay.tick().await.unwrap();
        assert_eq!(t.delivered, 2);
        assert_eq!(relay.pending().await.unwrap(), 0);
        assert!(relay.tick().await.unwrap().idle());
    }

    #[tokio::test]
    async fn a_rejected_row_is_split_out_of_its_batch() {
        let mem = Arc::new(Mem::default());
        mem.rows
            .lock()
            .unwrap()
            .extend([row("a", b"{}"), row("b", b"bad"), row("c", b"{}")]);
        let mut relay = Relay::new(
            mem.clone(),
            Arc::new(Sink { down: false }),
            RelayConfig::default(),
        );
        let t = relay.tick().await.unwrap();
        assert_eq!(t.delivered, 2, "{t:?}");
        assert_eq!(t.dead, 1, "{t:?}");
        assert_eq!(*mem.buried.lock().unwrap(), vec!["b".to_string()]);
        assert_eq!(relay.pending().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_backoff_grows_and_stops_growing() {
        let mem = Arc::new(Mem::default());
        mem.rows.lock().unwrap().push(row("a", b"{}"));
        let mut relay = Relay::new(mem, Arc::new(Sink { down: true }), RelayConfig::default());
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
