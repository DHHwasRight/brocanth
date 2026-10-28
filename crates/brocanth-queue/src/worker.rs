//! The worker: claim, run, heartbeat, settle, and stop without losing work.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;

use crate::backend::{
    BackendError, ClaimRequest, Claimed, DynBackend, LeaseRef, LeaseState, ReleaseReason, Settle,
};
use crate::job::{JobContext, JobFailure, JobId};
use crate::outbox::{Outbox, Relay};
use crate::registry::{Registry, Unmet};

/// Runs an outbox relay to completion inside the worker process.
///
/// [`Relay`] is generic over its store and [`Outbox`] is not object safe, so
/// the worker holds the relay as a closure it can spawn rather than as a
/// value it can name.
type RelayDriver =
    Box<dyn FnOnce(watch::Receiver<bool>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send>;

/// How many database connections the worker needs, and for what.
///
/// Pool size is the concurrency ceiling, and the symptom of getting it wrong
/// does not resemble the cause: every job is slow and the database looks
/// idle, because the tasks are queued on `acquire()` and never reach it.
///
/// The failure that matters is worse than slow. If the jobs and the queue's
/// own bookkeeping share one pool, `concurrency` running jobs can hold every
/// connection, the heartbeat cannot get one, every lease expires, and every
/// job is claimed again by another worker that does the same. A saturated
/// pool turns into a redelivery storm.
///
/// So the queue uses a **separate, small control pool** for claim, heartbeat,
/// settle and sweep. It is never shared with job bodies. [`Budget::check`]
/// is called at boot and refuses to start rather than letting the process
/// find this out under load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Budget {
    /// Connections the application's jobs draw on.
    pub app_pool: u32,
    /// Connections reserved for the queue's own calls.
    pub control_pool: u32,
    /// How many jobs may run at once.
    pub concurrency: u32,
    /// How many outbox relays run in this process.
    pub relays: u32,
    /// Whether a `LISTEN` connection is held. `sqlx`'s `PgListener` takes a
    /// connection for its whole life, and a notification sent while it is
    /// reconnecting is lost, so it is a latency optimisation over polling
    /// and never the only way work is picked up.
    pub listener: bool,
    /// Spare connections for anything else the process does.
    pub headroom: u32,
}

impl Budget {
    /// A budget for a worker that runs `concurrency` jobs, with a control
    /// pool sized to match.
    pub fn for_worker(app_pool: u32, concurrency: u32) -> Self {
        let mut b = Budget {
            app_pool,
            control_pool: 0,
            concurrency,
            relays: 1,
            listener: true,
            headroom: 1,
        };
        b.control_pool = b.control_needed();
        b
    }

    /// The smallest control pool this shape needs.
    ///
    /// One for the claim loop, one for a heartbeat that must not wait behind
    /// it, one for a settle that must not wait behind either, one per relay,
    /// and one for the listener.
    pub fn control_needed(&self) -> u32 {
        3 + self.relays + u32::from(self.listener)
    }

    /// The smallest application pool this shape needs.
    pub fn app_needed(&self) -> u32 {
        self.concurrency + self.headroom
    }

    /// Refuse a shape that cannot work. Call it at boot.
    pub fn check(&self) -> Result<(), PoolTooSmall> {
        if self.control_pool < self.control_needed() {
            return Err(PoolTooSmall {
                which: "control",
                have: self.control_pool,
                need: self.control_needed(),
                why: "claim, heartbeat and settle must never queue behind a job",
            });
        }
        if self.app_pool < self.app_needed() {
            return Err(PoolTooSmall {
                which: "application",
                have: self.app_pool,
                need: self.app_needed(),
                why: "every running job may hold one connection at the same time",
            });
        }
        Ok(())
    }
}

/// The boot refusal that replaces "everything is slow and the database is
/// idle".
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "the {which} pool has {have} connections and needs {need} for concurrency: {why}. \
     Raise the pool or lower the concurrency."
)]
pub struct PoolTooSmall {
    /// Which pool.
    pub which: &'static str,
    /// Its size.
    pub have: u32,
    /// What it must be.
    pub need: u32,
    /// The reason.
    pub why: &'static str,
}

/// How a worker runs.
#[derive(Debug, Clone)]
pub struct WorkerConfig {
    /// A name for this process, recorded on every claim.
    pub name: String,
    /// The queue to work.
    pub queue: String,
    /// How many jobs run at once. Bounded by [`Budget`].
    pub concurrency: u32,
    /// How long a claim holds a job before it expires.
    pub lease: Duration,
    /// How often to push the lease out. Must be well under `lease`, or a
    /// slow heartbeat loses the job it is trying to keep.
    pub heartbeat: Duration,
    /// How long to wait after a claim that returned nothing.
    pub poll_idle: Duration,
    /// How long running jobs have after a stop is asked for.
    pub shutdown_grace: Duration,
    /// How long to hide a job this worker cannot run, so it does not claim
    /// the same row again at once during a rolling deploy.
    pub unknown_kind_backoff: Duration,
    /// How often to call [`Backend::sweep`](crate::Backend::sweep).
    pub sweep_every: Duration,
    /// How many times to retry a settle that failed for a transient reason.
    /// A settle that never lands leaves the job to expire and run twice.
    pub settle_retries: u32,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        WorkerConfig {
            name: default_worker_name(),
            queue: "default".into(),
            concurrency: 8,
            lease: Duration::from_secs(60),
            heartbeat: Duration::from_secs(15),
            poll_idle: Duration::from_millis(500),
            shutdown_grace: Duration::from_secs(25),
            unknown_kind_backoff: Duration::from_secs(10),
            sweep_every: Duration::from_secs(30),
            settle_retries: 5,
        }
    }
}

impl WorkerConfig {
    /// Reject a configuration that cannot keep a lease alive.
    pub fn check(&self) -> Result<(), ConfigError> {
        if self.heartbeat * 3 > self.lease {
            return Err(ConfigError::HeartbeatTooSlow {
                heartbeat: self.heartbeat,
                lease: self.lease,
            });
        }
        if self.concurrency == 0 {
            return Err(ConfigError::NoConcurrency);
        }
        Ok(())
    }
}

/// A worker configuration that cannot work.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigError {
    /// The heartbeat cannot outrun the lease.
    #[error(
        "the heartbeat is {heartbeat:?} and the lease is {lease:?}; \
         one missed beat loses the job. Use a lease at least three beats long."
    )]
    HeartbeatTooSlow {
        /// The configured heartbeat.
        heartbeat: Duration,
        /// The configured lease.
        lease: Duration,
    },
    /// Concurrency of zero.
    #[error("concurrency is 0, so the worker would claim nothing")]
    NoConcurrency,
}

fn default_worker_name() -> String {
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "host".into());
    format!("{host}/{}", std::process::id())
}

/// What a worker tells the outside about its work.
///
/// Every method does nothing by default, so an application wires only what
/// it watches.
pub trait Observer: Send + Sync + 'static {
    /// A batch was claimed.
    fn on_claim(&self, _queue: &str, _count: usize) {}
    /// A job finished.
    fn on_done(&self, _kind: &str, _took: Duration) {}
    /// A job failed and will run again.
    fn on_retry(&self, _kind: &str, _attempt: u32, _failure: &JobFailure) {}
    /// A job failed for the last time.
    fn on_dead(&self, _kind: &str, _failure: &JobFailure) {}
    /// A job was handed back without spending an attempt.
    fn on_release(&self, _kind: &str, _reason: ReleaseReason) {}
    /// A job this worker cannot run was released. During a rolling deploy
    /// this is expected and brief. A number that does not fall means the
    /// new workers never arrived, and the queue is not draining.
    fn on_unknown_kind(&self, _kind: &str, _seen: u64) {}
    /// The fence did not match: another worker owns the job now.
    fn on_lease_lost(&self, _kind: &str) {}
    /// A backend call failed.
    fn on_backend_error(&self, _op: &'static str, _error: &BackendError) {}
}

/// An observer that does nothing.
#[derive(Debug, Default, Clone, Copy)]
pub struct Silent;

impl Observer for Silent {}

/// Pulls jobs from one queue and runs them.
pub struct Worker {
    backend: Arc<dyn DynBackend>,
    registry: Arc<Registry>,
    config: WorkerConfig,
    observer: Arc<dyn Observer>,
    unknown_seen: Arc<AtomicU64>,
    wakeup: Option<watch::Receiver<u64>>,
    relay: Option<RelayDriver>,
}

impl Worker {
    /// Build a worker. Checks the configuration and the registry against the
    /// backend's capabilities, and refuses a combination that cannot work.
    pub fn new(
        backend: Arc<dyn DynBackend>,
        registry: Arc<Registry>,
        config: WorkerConfig,
    ) -> Result<Self, StartError> {
        config.check()?;
        let caps = backend.capabilities();
        let unmet = registry.check(&caps);
        if !unmet.is_empty() {
            return Err(StartError::Unmet(unmet));
        }
        Ok(Worker {
            backend,
            registry,
            config,
            observer: Arc::new(Silent),
            unknown_seen: Arc::new(AtomicU64::new(0)),
            wakeup: None,
            relay: None,
        })
    }

    /// Watch the worker.
    pub fn observe(mut self, o: Arc<dyn Observer>) -> Self {
        self.observer = o;
        self
    }

    /// Run an outbox relay in this process, alongside the claim loop.
    ///
    /// The relay is what moves a bulk job out of the caller's database after
    /// the transaction that wrote it commits. Without one running somewhere,
    /// every such job stays in the outbox table and nothing delivers it, so a
    /// process that enqueues in a transaction must either carry a relay or
    /// know that another process does.
    ///
    /// [`Budget::control_needed`] already reserves a connection per relay, so
    /// the relay never competes with the claim, the heartbeat or a job body.
    ///
    /// It outlives the jobs on purpose. A job that is finishing inside the
    /// shutdown grace period can still enqueue bulk work, so the relay is
    /// stopped last and drains what it finds.
    pub fn relay<O: Outbox>(mut self, mut relay: Relay<O>) -> Self {
        self.relay = Some(Box::new(move |stop| {
            Box::pin(async move { relay.run(stop).await })
        }));
        self
    }

    /// Wake the claim loop early.
    ///
    /// A backend that can push a notification sends one here instead of
    /// waiting for `poll_idle`. It only ever shortens a wait: a lost
    /// notification costs latency and never a job, which is the only safe
    /// way to use `LISTEN/NOTIFY`.
    pub fn wake_on(mut self, rx: watch::Receiver<u64>) -> Self {
        self.wakeup = Some(rx);
        self
    }

    /// Run until `stop` turns true, then finish or release what is running.
    pub async fn run(mut self, mut stop: watch::Receiver<bool>) {
        let permits = Arc::new(Semaphore::new(self.config.concurrency as usize));
        let (job_stop_tx, job_stop_rx) = watch::channel(false);
        let mut running = JoinSet::new();
        let max_batch = self
            .backend
            .capabilities()
            .max_claim_batch
            .max(1)
            .min(self.config.concurrency as usize);
        let mut wakeup = self.wakeup.clone();

        // The relay gets its own stop signal, because it has to outlive the
        // jobs: one of them may enqueue bulk work on its way out.
        let (relay_stop_tx, relay_stop_rx) = watch::channel(false);
        let relay_task = self
            .relay
            .take()
            .map(|drive| tokio::spawn(drive(relay_stop_rx)));

        // The sweep runs on its own task and not as an arm of the claim
        // loop. As an arm it only fired on a claim that came back empty, so
        // a queue with a standing backlog never swept: scheduled work was
        // never promoted, exhausted rows were never dead-lettered, and the
        // deduplication table never shrank. The failure was invisible
        // because it needed load to appear.
        let sweeper = tokio::spawn(sweep_loop(
            Arc::clone(&self.backend),
            Arc::clone(&self.observer),
            self.config.sweep_every,
            stop.clone(),
        ));

        while !*stop.borrow() {
            // Never claim more than there are permits for. A claimed job
            // that waits for a permit is a job holding a lease it is not
            // using, and the lease is what another worker is waiting on.
            let free = permits.available_permits();
            if free == 0 {
                tokio::select! {
                    _ = stop.changed() => continue,
                    _ = tokio::time::sleep(Duration::from_millis(10)) => continue,
                }
            }

            let req = ClaimRequest {
                queue: self.config.queue.clone(),
                max: free.min(max_batch),
                lease: self.config.lease,
                kinds: self
                    .backend
                    .capabilities()
                    .filter_by_kind
                    .then(|| self.registry.kinds()),
                worker: self.config.name.clone(),
            };

            // The claim is awaited to completion and never raced against the
            // stop signal. Dropping it mid-flight does not undo it: the
            // statement commits on the server, and the rows it took come
            // back with an attempt spent and a lease token no process has
            // ever seen. They would be unreleasable, and invisible until the
            // lease expired. Finishing the claim costs one round trip of
            // shutdown latency and loses nothing.
            let claimed = self.backend.claim(&req).await;

            let claimed = match claimed {
                Ok(c) => c,
                Err(e) => {
                    self.observer.on_backend_error("claim", &e);
                    tokio::select! {
                        _ = tokio::time::sleep(self.config.poll_idle) => {}
                        _ = stop.changed() => {}
                    }
                    continue;
                }
            };

            if claimed.is_empty() {
                tokio::select! {
                    _ = tokio::time::sleep(self.config.poll_idle) => {}
                    _ = async {
                        match wakeup.as_mut() {
                            Some(rx) => { let _ = rx.changed().await; }
                            None => std::future::pending::<()>().await,
                        }
                    } => {}
                    _ = stop.changed() => {}
                }
                continue;
            }

            self.observer.on_claim(&self.config.queue, claimed.len());
            for job in claimed {
                let permit = Arc::clone(&permits)
                    .try_acquire_owned()
                    .expect("claimed no more than there were permits");
                running.spawn(run_one(
                    Arc::clone(&self.backend),
                    Arc::clone(&self.registry),
                    Arc::clone(&self.observer),
                    Arc::clone(&self.unknown_seen),
                    self.config.clone(),
                    job,
                    job_stop_rx.clone(),
                    permit,
                ));
            }
            // Reap what finished, without waiting.
            while running.try_join_next().is_some() {}
        }

        // Stopping. Tell the jobs, then wait out the grace period. A job
        // that returns in time settles normally. One that does not is
        // dropped, and its guard releases the lease so another worker takes
        // it at once rather than after the lease expires.
        let _ = job_stop_tx.send(true);
        let grace = tokio::time::sleep(self.config.shutdown_grace);
        tokio::pin!(grace);
        loop {
            tokio::select! {
                next = running.join_next() => {
                    if next.is_none() { break; }
                }
                _ = &mut grace => {
                    running.shutdown().await;
                    break;
                }
            }
        }

        // Only now is it safe to stop the relay: every job that was going to
        // write an outbox row has either finished or been released.
        if let Some(task) = relay_task {
            let _ = relay_stop_tx.send(true);
            let _ = task.await;
        }
        sweeper.abort();
        let _ = sweeper.await;
    }
}

/// Call [`Backend::sweep`](crate::Backend::sweep) on a timer until `stop`.
///
/// A backend that declares the call unsupported is asked once and then left
/// alone: repeating it would report the same error for the life of the
/// process and bury the errors that matter.
async fn sweep_loop(
    backend: Arc<dyn DynBackend>,
    observer: Arc<dyn Observer>,
    every: Duration,
    mut stop: watch::Receiver<bool>,
) {
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // the first tick is immediate
    while !*stop.borrow() {
        tokio::select! {
            _ = tick.tick() => {
                if let Err(e) = backend.sweep().await {
                    let unsupported = matches!(e, BackendError::Unsupported(_));
                    observer.on_backend_error("sweep", &e);
                    if unsupported {
                        return;
                    }
                }
            }
            _ = stop.changed() => return,
        }
    }
}

/// Why a worker could not start.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The configuration is not self-consistent.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The pool cannot serve the configured concurrency.
    #[error(transparent)]
    Pool(#[from] PoolTooSmall),
    /// A registered job needs something the backend cannot do.
    #[error("the backend cannot serve every registered job: {}", fmt_unmet(.0))]
    Unmet(Vec<Unmet>),
}

fn fmt_unmet(u: &[Unmet]) -> String {
    u.iter()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Releases a lease if the task is dropped before it settles.
///
/// The release is spawned, because `Drop` cannot await. A runtime that is
/// already shutting down may never run it, which is exactly why lease expiry
/// exists: the guard shortens the gap, it does not replace the backstop.
struct LeaseGuard {
    backend: Arc<dyn DynBackend>,
    lease: Option<LeaseRef>,
    not_before: Duration,
}

impl LeaseGuard {
    fn disarm(&mut self) {
        self.lease = None;
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let Some(lease) = self.lease.take() else {
            return;
        };
        let backend = Arc::clone(&self.backend);
        let not_before = SystemTime::now() + self.not_before;
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let _ = backend
                    .settle(
                        &lease,
                        Settle::Release {
                            not_before,
                            reason: ReleaseReason::Cancelled,
                        },
                    )
                    .await;
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_one(
    backend: Arc<dyn DynBackend>,
    registry: Arc<Registry>,
    observer: Arc<dyn Observer>,
    unknown_seen: Arc<AtomicU64>,
    config: WorkerConfig,
    claimed: Claimed,
    stopping: watch::Receiver<bool>,
    _permit: tokio::sync::OwnedSemaphorePermit,
) {
    let mut guard = LeaseGuard {
        backend: Arc::clone(&backend),
        lease: Some(claimed.lease.clone()),
        not_before: Duration::ZERO,
    };

    // The rolling-deploy case. An old worker that meets a new job kind hands
    // it back **without spending an attempt**. Failing it instead would burn
    // the retry budget of a job the new workers can run, and dead-letter it
    // before they finish rolling out.
    let Some(entry) = registry.get(&claimed.kind) else {
        let seen = unknown_seen.fetch_add(1, Ordering::Relaxed) + 1;
        observer.on_unknown_kind(&claimed.kind, seen);
        observer.on_release(&claimed.kind, ReleaseReason::UnknownKind);
        guard.disarm();
        settle_with_retries(
            &backend,
            &observer,
            &claimed,
            Settle::Release {
                not_before: SystemTime::now() + config.unknown_kind_backoff,
                reason: ReleaseReason::UnknownKind,
            },
            config.settle_retries,
        )
        .await;
        return;
    };

    let cx = JobContext {
        id: claimed.lease.id.clone(),
        kind: claimed.kind.clone(),
        queue: claimed.queue.clone(),
        attempt: claimed.attempt,
        max_attempts: claimed.max_attempts,
        worker: config.name.clone(),
        stopping: stopping.clone(),
    };

    let started = std::time::Instant::now();
    let mut handle = tokio::spawn(entry.run(claimed.payload.clone(), cx));
    let mut beat = tokio::time::interval(config.heartbeat);
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    beat.tick().await; // the first tick is immediate

    let mut stopping = stopping;
    let mut deadline: Option<std::pin::Pin<Box<tokio::time::Sleep>>> = if *stopping.borrow() {
        Some(Box::pin(tokio::time::sleep(config.shutdown_grace)))
    } else {
        None
    };

    // What the worker believes the lease runs out at. A successful heartbeat
    // pushes it; a failed one does not. The backend reports the first expiry
    // on the claim, so this starts from the backend's clock and moves only by
    // durations.
    let mut expires_at = claimed.lease.expires_at;

    let outcome: Settle = loop {
        tokio::select! {
            joined = &mut handle => {
                break match joined {
                    Ok(Ok(())) => {
                        observer.on_done(&claimed.kind, started.elapsed());
                        Settle::Done
                    }
                    Ok(Err(failure)) => decide(&observer, &claimed, entry, failure),
                    Err(e) if e.is_panic() => {
                        let msg = panic_message(e.into_panic());
                        decide(&observer, &claimed, entry, JobFailure::panicked(msg))
                    }
                    // Aborted from outside. Give it back, do not spend an
                    // attempt.
                    Err(_) => Settle::Release {
                        not_before: SystemTime::now(),
                        reason: ReleaseReason::Cancelled,
                    },
                };
            }
            // The lease ran out without a heartbeat landing. Another worker
            // is free to claim this job now, so this one stops touching it:
            // it aborts the body and settles nothing, exactly as it does for
            // a lease it is told it lost. Logging the failed heartbeat and
            // carrying on would leave two workers running the same job with
            // neither of them aware of it.
            _ = tokio::time::sleep(remaining(expires_at)) => {
                observer.on_lease_lost(&claimed.kind);
                handle.abort();
                let _ = handle.await;
                guard.disarm();
                return;
            }
            _ = beat.tick() => {
                match backend.heartbeat(&claimed.lease, config.lease).await {
                    Ok(LeaseState::Held) => expires_at = SystemTime::now() + config.lease,
                    Ok(LeaseState::Lost) => {
                        // Another worker owns this job. Stop at once and
                        // settle nothing: a settle now would delete their
                        // row. This is what the fence is for.
                        observer.on_lease_lost(&claimed.kind);
                        handle.abort();
                        let _ = handle.await;
                        guard.disarm();
                        return;
                    }
                    Err(e) => observer.on_backend_error("heartbeat", &e),
                }
            }
            _ = stopping.changed(), if deadline.is_none() => {
                deadline = Some(Box::pin(tokio::time::sleep(config.shutdown_grace)));
            }
            _ = async {
                match deadline.as_mut() {
                    Some(d) => d.as_mut().await,
                    None => std::future::pending::<()>().await,
                }
            }, if deadline.is_some() => {
                handle.abort();
                let _ = handle.await;
                observer.on_release(&claimed.kind, ReleaseReason::Stopping);
                break Settle::Release {
                    not_before: SystemTime::now(),
                    reason: ReleaseReason::Stopping,
                };
            }
        }
    };

    guard.disarm();
    settle_with_retries(
        &backend,
        &observer,
        &claimed,
        outcome,
        config.settle_retries,
    )
    .await;
}

fn decide(
    observer: &Arc<dyn Observer>,
    claimed: &Claimed,
    entry: &crate::registry::Entry,
    failure: JobFailure,
) -> Settle {
    let last = claimed.attempt >= claimed.max_attempts;
    if last || !failure.kind.is_retryable() {
        observer.on_dead(&claimed.kind, &failure);
        return Settle::Dead { failure };
    }
    observer.on_retry(&claimed.kind, claimed.attempt, &failure);
    let seed = jitter_seed(&claimed.lease.id, claimed.attempt);
    let at = SystemTime::now() + entry.backoff.delay(claimed.attempt, seed);
    Settle::Retry { at, failure }
}

/// How long until an instant, from this process's clock. Zero if it passed.
fn remaining(t: SystemTime) -> Duration {
    t.duration_since(SystemTime::now())
        .unwrap_or(Duration::ZERO)
}

/// A reproducible jitter source, so the crate takes no random number
/// generator and a test can predict a delay.
fn jitter_seed(id: &JobId, attempt: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.as_str().hash(&mut h);
    attempt.hash(&mut h);
    h.finish()
}

fn panic_message(p: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "panicked".into()
    }
}

/// A settle that does not land leaves the job to expire and run twice, so
/// retry a transient failure before giving up.
async fn settle_with_retries(
    backend: &Arc<dyn DynBackend>,
    observer: &Arc<dyn Observer>,
    claimed: &Claimed,
    outcome: Settle,
    retries: u32,
) {
    let mut wait = Duration::from_millis(50);
    for _ in 0..=retries {
        match backend.settle(&claimed.lease, outcome.clone()).await {
            Ok(LeaseState::Held) => return,
            Ok(LeaseState::Lost) => {
                observer.on_lease_lost(&claimed.kind);
                return;
            }
            Err(e) if e.is_transient() => {
                observer.on_backend_error("settle", &e);
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(2));
            }
            Err(e) => {
                observer.on_backend_error("settle", &e);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_control_pool_that_cannot_heartbeat_is_refused() {
        let b = Budget {
            app_pool: 20,
            control_pool: 2,
            concurrency: 16,
            relays: 1,
            listener: true,
            headroom: 1,
        };
        let e = b.check().unwrap_err();
        assert_eq!(e.which, "control");
        assert_eq!(e.need, 5);
    }

    #[test]
    fn concurrency_above_the_pool_is_refused_at_boot() {
        let b = Budget {
            app_pool: 10,
            control_pool: 8,
            concurrency: 32,
            relays: 1,
            listener: false,
            headroom: 1,
        };
        let e = b.check().unwrap_err();
        assert_eq!(e.which, "application");
        assert_eq!(e.need, 33);
        assert!(Budget::for_worker(33, 32).check().is_ok());
    }

    #[test]
    fn a_heartbeat_that_cannot_outrun_the_lease_is_refused() {
        let c = WorkerConfig {
            lease: Duration::from_secs(30),
            heartbeat: Duration::from_secs(20),
            ..WorkerConfig::default()
        };
        assert!(matches!(
            c.check(),
            Err(ConfigError::HeartbeatTooSlow { .. })
        ));
        assert!(WorkerConfig::default().check().is_ok());
    }

    #[test]
    fn the_jitter_seed_is_stable_for_a_job_and_attempt() {
        let id = JobId("42".into());
        assert_eq!(jitter_seed(&id, 3), jitter_seed(&id, 3));
        assert_ne!(jitter_seed(&id, 3), jitter_seed(&id, 4));
    }
}
