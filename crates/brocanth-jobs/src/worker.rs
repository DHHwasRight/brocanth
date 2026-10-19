//! The worker: a pool, a heartbeat, and a shutdown with a deadline.
//!
//! # The runtime question, answered
//!
//! **Workers run on Tokio, and by default on the same runtime that serves
//! requests.**
//!
//! The research flagged this as the decision that has to come before a job
//! system rather than after, and the core deliberately contains no async
//! runtime — `brocanth`'s own test fails the build if one appears in its
//! dependency tree — so bringing one in is this crate's decision to own.
//!
//! *Why Tokio.* The workspace already resolves `sqlx` 0.9 through SeaORM, which
//! is pinned to `runtime-tokio-rustls`. A queue on `smol` or `async-std` in the
//! same process as a Tokio-flavoured driver means two reactors, two thread
//! pools and a connection pool whose timers belong to the wrong one. There is
//! one defensible runtime here and this is it.
//!
//! *Why the same runtime.* [`Worker::start`] spawns onto
//! `tokio::runtime::Handle::current()`, which in a server process is the
//! runtime already running the handlers. The alternative — a second runtime
//! inside the same process — gives you two thread pools sized independently,
//! competing for the same cores, with no shared backpressure and no shared
//! view of how busy the machine is. One runtime, with concurrency limited where
//! it belongs (`WorkerBuilder::concurrency`), is the simpler and more
//! controllable arrangement.
//!
//! *What it costs, stated plainly.*
//!
//! 1. **A job that blocks the thread stalls request handlers.** They share a
//!    worker pool. Blocking work goes in `tokio::task::spawn_blocking`, and
//!    this is now a rule the application has to keep rather than a problem the
//!    framework solved.
//! 2. **A job that burns CPU competes with request latency.** Tokio is fair,
//!    not prioritised; a pegged core is a pegged core.
//! 3. **Tokio arrives in the dependency tree the moment this crate is named in
//!    `main`.** That is the entire reason jobs are not in the core.
//!
//! *The way out of 1 and 2 is the same one Rails uses and it is not a runtime:*
//! run workers in their own process. [`Worker::run`] is a plain `async fn`, so
//! a `jobs` binary is a `main`, a [`Queue`], a [`JobRegistry`] and a call. The
//! defaults are shaped for the in-process case because that is what a developer
//! runs on a laptop and it must be the same code that runs in production — the
//! other half of §4 step 9 — and the separate process is a deployment choice
//! rather than a different queue.
//!
//! # Shutdown
//!
//! [`WorkerHandle::shutdown`] stops claiming immediately, waits up to a
//! deadline for the jobs already running, and then gives the rest back:
//! `state = 'pending'`, runnable now, **and the attempt they had taken is given
//! back with them**. The worker chose to stop; the job did not fail, and
//! charging it an attempt every time somebody deploys is how a four-minute job
//! ends up in the dead letter after a busy afternoon of releases.
//!
//! # Heartbeat
//!
//! A running job's row carries `heartbeat_at`, refreshed on a timer by the
//! worker holding it. A worker that is killed stops refreshing, and any other
//! worker's maintenance tick puts its jobs back — see
//! [`Queue::reclaim_orphans`]. Without this, a job held by a machine that is
//! never coming back is held forever, which is the quiet way a queue loses
//! work.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;
use tokio::task::JoinSet;

use crate::clock::{to_millis, Clock};
use crate::error::{Error, Result};
use crate::events::{JobEvent, JobEvents};
use crate::job::{FailureKind, JobContext, JobFailure, JobId};
use crate::queue::Queue;
use crate::registry::JobRegistry;
use crate::store::Store;

/// How a worker is set up.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// Which queues to claim from. Defaults to `["default"]`.
    pub queues: Vec<String>,
    /// How many jobs may run at once on this worker.
    pub concurrency: usize,
    /// How long to wait after finding nothing before looking again, on a
    /// store that cannot announce new work.
    ///
    /// SQLite, in other words. On Postgres a worker is woken by the enqueue
    /// itself and this is not used; see [`safety_poll`](Self::safety_poll)
    /// and the [`listen`](crate::listen) module.
    pub poll_interval: Duration,
    /// The longest a listening worker sleeps before looking anyway.
    ///
    /// A backstop and not the mechanism. A worker on Postgres sleeps until
    /// the soonest pending job is due, and a notification only shortens
    /// that; this caps the sleep so a notification nobody heard -- sent
    /// while the listener was reconnecting -- costs seconds rather than
    /// waiting for the next enqueue.
    pub safety_poll: Duration,
    /// Whether to listen for new work instead of asking for it.
    ///
    /// On by default, and ignored on a store with nothing to listen to.
    /// Turning it off is for a deployment whose connection limit cannot
    /// spare one connection per worker process: that is what a listener
    /// costs, because `LISTEN` holds a session.
    pub listen: bool,
    /// How often to refresh `heartbeat_at` on the jobs this worker holds.
    pub heartbeat_interval: Duration,
    /// How quiet a worker must go before another worker takes its jobs back.
    ///
    /// Must be comfortably more than `heartbeat_interval`, or a worker that is
    /// merely busy gets its work stolen. The builder refuses a value below
    /// three times the heartbeat.
    pub stale_after: Duration,
    /// How often to reclaim orphans and tick the recurring schedules.
    pub maintenance_interval: Duration,
    /// How long one job may run before it is abandoned as timed out.
    ///
    /// `None` by default: a timeout that is too short turns slow work into
    /// failed work, and this crate will not guess a number. Setting one is how
    /// you stop a job that hangs on a socket from holding a slot forever —
    /// heartbeats keep flowing while it hangs, so the orphan sweep will not
    /// save you.
    pub job_timeout: Option<Duration>,
    /// A name for this worker in the `locked_by` column and in events.
    /// Defaults to `"<hostname-ish>-<pid>-<n>"`.
    pub id: Option<String>,
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            queues: vec!["default".to_string()],
            concurrency: 5,
            poll_interval: Duration::from_millis(500),
            safety_poll: Duration::from_secs(5),
            listen: true,
            heartbeat_interval: Duration::from_secs(5),
            stale_after: Duration::from_secs(60),
            maintenance_interval: Duration::from_secs(15),
            job_timeout: None,
            id: None,
        }
    }
}

/// Builds a [`Worker`].
#[must_use = "a worker builder does nothing until `build` or `start` is called"]
pub struct WorkerBuilder {
    queue: Queue,
    registry: JobRegistry,
    config: WorkerConfig,
}

impl WorkerBuilder {
    /// Which queues to claim from. Replaces the default of `["default"]`.
    pub fn queues<I, S>(mut self, queues: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.queues = queues.into_iter().map(Into::into).collect();
        self
    }

    /// How many jobs may run at once.
    pub fn concurrency(mut self, concurrency: usize) -> Self {
        self.config.concurrency = concurrency.max(1);
        self
    }

    /// How long to wait after an empty poll, where polling is the mechanism.
    pub fn poll_interval(mut self, interval: Duration) -> Self {
        self.config.poll_interval = interval;
        self
    }

    /// The longest a listening worker sleeps before looking anyway.
    pub fn safety_poll(mut self, interval: Duration) -> Self {
        self.config.safety_poll = interval;
        self
    }

    /// Whether to be woken by new work rather than ask for it. On by
    /// default; see [`WorkerConfig::listen`].
    pub fn listen(mut self, listen: bool) -> Self {
        self.config.listen = listen;
        self
    }

    /// How often to refresh the heartbeat.
    pub fn heartbeat_interval(mut self, interval: Duration) -> Self {
        self.config.heartbeat_interval = interval;
        self
    }

    /// How long a silent worker's jobs stay held before another worker takes
    /// them.
    pub fn stale_after(mut self, stale_after: Duration) -> Self {
        self.config.stale_after = stale_after;
        self
    }

    /// How often to reclaim orphans and tick the schedules.
    pub fn maintenance_interval(mut self, interval: Duration) -> Self {
        self.config.maintenance_interval = interval;
        self
    }

    /// Abandons a job that has run for longer than this. Off by default.
    pub fn job_timeout(mut self, timeout: Duration) -> Self {
        self.config.job_timeout = Some(timeout);
        self
    }

    /// Names this worker, which is what lands in `locked_by`.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.config.id = Some(id.into());
        self
    }

    /// Teaches the worker one job type.
    pub fn register<J: crate::Job>(mut self) -> Self {
        self.registry.register::<J>();
        self
    }

    /// Replaces the registry wholesale, for an application that builds one
    /// centrally and hands the same one to every worker.
    pub fn registry(mut self, registry: JobRegistry) -> Self {
        self.registry = registry;
        self
    }

    /// Finishes the worker without starting it.
    ///
    /// Refuses an empty registry — a worker that knows no job types claims
    /// rows, fails every one of them with "no job named …", and burns their
    /// attempts — and refuses a `stale_after` that a merely busy worker could
    /// trip over.
    pub fn build(self) -> Result<Worker> {
        if self.registry.is_empty() {
            return Err(Error::UnknownJob {
                name: "<none>".to_string(),
                known: "nothing: call `register::<J>()` before `build`".to_string(),
            });
        }
        if self.config.stale_after < self.config.heartbeat_interval * 3 {
            return Err(Error::Backend(format!(
                "stale_after ({:?}) must be at least three times heartbeat_interval ({:?}), \
                 or a worker that is merely busy will have its jobs reclaimed from under it",
                self.config.stale_after, self.config.heartbeat_interval
            )));
        }
        let id = self
            .config
            .id
            .clone()
            .unwrap_or_else(|| default_worker_id(&self.config.queues));
        Ok(Worker {
            queue: self.queue,
            registry: self.registry,
            config: self.config,
            id,
        })
    }

    /// Builds the worker and spawns it on the current Tokio runtime.
    ///
    /// See the module documentation for why that is the current runtime rather
    /// than one of this crate's own.
    pub fn start(self) -> Result<WorkerHandle> {
        self.build()?.start()
    }
}

/// A worker.
pub struct Worker {
    queue: Queue,
    registry: JobRegistry,
    config: WorkerConfig,
    id: String,
}

impl Worker {
    /// Starts building a worker against `queue`.
    pub fn builder(queue: &Queue) -> WorkerBuilder {
        WorkerBuilder {
            queue: queue.clone(),
            registry: JobRegistry::new(),
            config: WorkerConfig::default(),
        }
    }

    /// This worker's id, which is the value in `locked_by`.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Spawns the worker on the current Tokio runtime.
    pub fn start(self) -> Result<WorkerHandle> {
        let handle = tokio::runtime::Handle::try_current().map_err(|e| {
            Error::NoRuntime(format!(
                "{e}; call this from inside `#[tokio::main]`, or use `Worker::run`, \
                 which is a plain future you can drive yourself"
            ))
        })?;
        let (shutdown_tx, shutdown_rx) = watch::channel(ShutdownSignal::Running);
        let id = self.id.clone();
        let join = handle.spawn(async move { self.run_with(shutdown_rx).await });
        Ok(WorkerHandle {
            id,
            shutdown_tx,
            join,
        })
    }

    /// Runs the worker until `shutdown` resolves, then finishes in-flight jobs
    /// within `grace` and returns the rest to the queue.
    ///
    /// The form for a dedicated worker process: pass
    /// `tokio::signal::ctrl_c()`, or whatever the deployment uses.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send,
        grace: Duration,
    ) -> Result<ShutdownReport> {
        let (tx, rx) = watch::channel(ShutdownSignal::Running);
        let worker = tokio::spawn(async move { self.run_with(rx).await });
        shutdown.await;
        let _ = tx.send(ShutdownSignal::Draining(grace));
        worker
            .await
            .map_err(|e| Error::NoRuntime(format!("the worker task did not finish: {e}")))?
    }

    async fn run_with(
        self,
        mut shutdown: watch::Receiver<ShutdownSignal>,
    ) -> Result<ShutdownReport> {
        let Worker {
            queue,
            registry,
            config,
            id,
        } = self;

        let store = queue.store_handle();
        let clock = queue.clock_handle();
        let events = queue.event_sink();
        let draining = Arc::new(AtomicBool::new(false));

        events.handle(JobEvent::WorkerStarted {
            worker: &id,
            queues: &config.queues,
        });

        // Two background tasks, both stopped by the same watch channel: the
        // heartbeat that keeps this worker's claims alive, and the maintenance
        // tick that reclaims everybody else's orphans and fires the schedules.
        let heartbeat = tokio::spawn(heartbeat_loop(
            Arc::clone(&store),
            Arc::clone(&clock),
            queue.event_sink(),
            id.clone(),
            config.heartbeat_interval,
            shutdown.clone(),
        ));
        let maintenance = tokio::spawn(maintenance_loop(
            queue.clone(),
            config.stale_after,
            config.maintenance_interval,
            shutdown.clone(),
        ));

        // And a third, where the store can announce its own work. It is an
        // optimisation with no say in correctness: the bell only ever cuts a
        // sleep short, so a listener that never connects costs idle latency
        // and nothing else.
        let bell = crate::listen::Bell::default();
        let listener = crate::listen::spawn(
            &store,
            &bell,
            queue.event_sink(),
            config.safety_poll,
            config.listen,
        );
        // The store decides this, not the config: `listen = true` on SQLite
        // is a wish, and the sleep has to follow what actually happened.
        let listening = listener.is_some();

        let mut running: JoinSet<()> = JoinSet::new();
        let mut finished = 0usize;
        let grace = loop {
            if let ShutdownSignal::Draining(grace) = *shutdown.borrow_and_update() {
                break grace;
            }

            // Claim only as much as there is room for. A worker that claims a
            // batch it cannot start is a worker holding rows nobody is working
            // on, which looks exactly like a stuck queue.
            let room = config.concurrency.saturating_sub(running.len());
            let claimed = if room == 0 {
                Vec::new()
            } else {
                match store
                    .claim(&id, &config.queues, room as i64, to_millis(clock.now()))
                    .await
                {
                    Ok(rows) => rows,
                    Err(e) => {
                        events.handle(JobEvent::InternalError {
                            during: "claiming jobs",
                            error: &e,
                        });
                        Vec::new()
                    }
                }
            };

            let claimed_count = claimed.len();
            for row in claimed {
                let attempt = row.attempts.max(1) as u32;
                let cx = JobContext {
                    id: JobId(row.id),
                    queue: row.queue.clone(),
                    name: row.job_name.clone(),
                    attempt,
                    max_attempts: row.max_attempts.max(1) as u32,
                    enqueued_at: crate::clock::from_millis(row.created_at),
                    worker: id.clone(),
                    shutdown: Arc::clone(&draining),
                };
                let task = RunOne {
                    store: Arc::clone(&store),
                    clock: Arc::clone(&clock),
                    events: queue.event_sink(),
                    registry: registry.clone(),
                    worker: id.clone(),
                    timeout: config.job_timeout,
                    row,
                    cx,
                };
                running.spawn(task.run());
            }

            // Drain whatever has finished, without waiting for anything.
            while let Some(done) = running.try_join_next() {
                if done.is_ok() {
                    finished += 1;
                }
            }

            if claimed_count == 0 {
                // Nothing to do. How long to wait is the whole question, and
                // the answer is the soonest of: the next job's `run_at`, the
                // cap, a notification, a free slot, a shutdown.
                let nap = idle_nap(&store, &clock, &config, listening).await;
                tokio::select! {
                    _ = tokio::time::sleep(nap) => {}
                    _ = bell.rung(), if listening => {}
                    _ = shutdown.changed() => {}
                    Some(done) = running.join_next(), if !running.is_empty() => {
                        if done.is_ok() {
                            finished += 1;
                        }
                    }
                }
            } else if running.len() >= config.concurrency {
                // Full. Wait for a slot rather than spinning on the claim.
                tokio::select! {
                    _ = shutdown.changed() => {}
                    Some(done) = running.join_next() => {
                        if done.is_ok() {
                            finished += 1;
                        }
                    }
                }
            }
        };

        // Draining. Stop claiming, tell the running jobs, and wait out the
        // grace period.
        draining.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + grace;
        while !running.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            match tokio::time::timeout(left, running.join_next()).await {
                Ok(Some(done)) => {
                    if done.is_ok() {
                        finished += 1;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        // Whatever is still running at the deadline is abandoned here and given
        // back to the queue. Aborting first means the task stops at its next
        // await point rather than racing the release.
        running.abort_all();
        while running.join_next().await.is_some() {}

        let returned = match store.release(&id, to_millis(clock.now())).await {
            Ok(n) => n,
            Err(e) => {
                events.handle(JobEvent::InternalError {
                    during: "returning in-flight jobs at shutdown",
                    error: &e,
                });
                0
            }
        };

        heartbeat.abort();
        maintenance.abort();
        if let Some(listener) = listener {
            listener.abort();
        }

        events.handle(JobEvent::WorkerStopped {
            worker: &id,
            finished,
            returned,
        });

        Ok(ShutdownReport { finished, returned })
    }
}

/// What a shutdown did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Jobs that finished — successfully or not — before the deadline.
    pub finished: usize,
    /// Jobs abandoned at the deadline and put back on the queue.
    pub returned: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShutdownSignal {
    Running,
    Draining(Duration),
}

/// A running worker.
pub struct WorkerHandle {
    id: String,
    shutdown_tx: watch::Sender<ShutdownSignal>,
    join: tokio::task::JoinHandle<Result<ShutdownReport>>,
}

impl WorkerHandle {
    /// The worker's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Stops claiming, waits up to `grace` for the jobs already running, and
    /// returns the rest to the queue.
    pub async fn shutdown(self, grace: Duration) -> Result<ShutdownReport> {
        let _ = self.shutdown_tx.send(ShutdownSignal::Draining(grace));
        self.join
            .await
            .map_err(|e| Error::NoRuntime(format!("the worker task did not finish: {e}")))?
    }

    /// Whether the worker task has ended.
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    /// Stops the worker **without** draining: no grace period, nothing
    /// returned to the queue.
    ///
    /// The jobs this worker holds stay locked and `running` until their
    /// heartbeat lapses and some other worker's maintenance tick reclaims
    /// them. That is on purpose — it is what `kill -9`, a lost machine and an
    /// out-of-memory kill all look like from the database's side, and it is
    /// here so that the reclaim path can be tested with a worker that really
    /// does stop mid-job rather than with a hand-written row.
    ///
    /// Use [`WorkerHandle::shutdown`] for an orderly stop.
    pub fn abort(self) {
        self.join.abort();
    }
}

// ---------------------------------------------------------------------------
// One attempt
// ---------------------------------------------------------------------------

struct RunOne {
    store: Arc<Store>,
    clock: Arc<dyn Clock>,
    events: Arc<dyn JobEvents>,
    registry: JobRegistry,
    worker: String,
    timeout: Option<Duration>,
    row: crate::store::JobRow,
    cx: JobContext,
}

impl RunOne {
    async fn run(self) {
        let RunOne {
            store,
            clock,
            events,
            registry,
            worker,
            timeout,
            row,
            cx,
        } = self;

        let id = JobId(row.id);
        let attempt = cx.attempt;
        events.handle(JobEvent::Started {
            id,
            name: &row.job_name,
            attempt,
            worker: &worker,
        });

        let started = Instant::now();
        let outcome = match registry.handler(&row.job_name) {
            Some(handler) => {
                let fut = handler(row.payload.clone(), cx);
                match timeout {
                    Some(limit) => match tokio::time::timeout(limit, fut).await {
                        Ok(result) => result,
                        Err(_) => Err(JobFailure::of(
                            FailureKind::TimedOut,
                            format!("still running after {limit:?}"),
                        )),
                    },
                    None => fut.await,
                }
            }
            None => Err(JobFailure::of(
                FailureKind::Undecodable,
                format!(
                    "no job named `{}` is registered on worker `{worker}`; registered: {}",
                    row.job_name,
                    registry.names().join(", ")
                ),
            )),
        };

        let took = started.elapsed();
        match outcome {
            Ok(()) => {
                if let Err(e) = store.complete(id, &worker).await {
                    events.handle(JobEvent::InternalError {
                        during: "deleting a finished job",
                        error: &e,
                    });
                    return;
                }
                events.handle(JobEvent::Completed {
                    id,
                    name: &row.job_name,
                    took,
                    attempt,
                });
            }
            Err(failure) => {
                let attempts_left = row.attempts < row.max_attempts;
                if failure.kind.is_retryable() && attempts_left {
                    let backoff = registry.backoff_for(&row.job_name);
                    let mut rng = crate::backoff::Prng::from_entropy();
                    let delay = backoff.delay(attempt, &mut rng);
                    let run_at = to_millis(clock.now()) + delay.as_millis() as i64;
                    if let Err(e) = store
                        .retry_at(id, &worker, run_at, &failure.to_string())
                        .await
                    {
                        events.handle(JobEvent::InternalError {
                            during: "rescheduling a failed job",
                            error: &e,
                        });
                        return;
                    }
                    events.handle(JobEvent::Retrying {
                        id,
                        name: &row.job_name,
                        attempt,
                        delay,
                        failure: &failure,
                    });
                } else {
                    match store
                        .dead_letter(id, &failure, to_millis(clock.now()))
                        .await
                    {
                        Ok(_) => events.handle(JobEvent::DeadLettered {
                            id,
                            name: &row.job_name,
                            attempts: attempt,
                            failure: &failure,
                        }),
                        Err(e) => events.handle(JobEvent::InternalError {
                            during: "moving a job to the dead letter",
                            error: &e,
                        }),
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The two background loops
// ---------------------------------------------------------------------------

/// How long a worker with nothing to do should sleep.
///
/// Three inputs, and the smallest wins:
///
/// 1. **The next job's `run_at`.** A job scheduled for 09:00 should start at
///    09:00, not at whichever tick follows it. This is why the cap can be
///    seconds without making every retry wait seconds.
/// 2. **The cap** -- [`safety_poll`](WorkerConfig::safety_poll) while
///    listening, [`poll_interval`](WorkerConfig::poll_interval) otherwise.
/// 3. **Zero**, when the next job is already due. That means the claim that
///    just came back empty lost a race to another worker, and there may be
///    more where that came from, so the loop goes straight round again.
///
/// A failed lookup falls back to the cap. The question "when is the next job
/// due" is an optimisation, and a database that cannot answer it is already
/// being reported by the claim that runs next.
async fn idle_nap(
    store: &Store,
    clock: &Arc<dyn Clock>,
    config: &WorkerConfig,
    listening: bool,
) -> Duration {
    let cap = if listening {
        config.safety_poll
    } else {
        config.poll_interval
    };
    match store.next_run_at(&config.queues).await {
        Ok(Some(due)) => {
            let now = to_millis(clock.now());
            let wait = due.saturating_sub(now).max(0) as u64;
            cap.min(Duration::from_millis(wait))
        }
        // Nothing pending at all. Nothing to wake up for except a
        // notification, which is exactly what the cap is a backstop for.
        Ok(None) => cap,
        Err(_) => cap,
    }
}

async fn heartbeat_loop(
    store: Arc<Store>,
    clock: Arc<dyn Clock>,
    events: Arc<dyn JobEvents>,
    worker: String,
    interval: Duration,
    mut shutdown: watch::Receiver<ShutdownSignal>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => return,
        }
        if let Err(e) = store.heartbeat(&worker, to_millis(clock.now())).await {
            events.handle(JobEvent::InternalError {
                during: "sending a heartbeat",
                error: &e,
            });
        }
    }
}

async fn maintenance_loop(
    queue: Queue,
    stale_after: Duration,
    interval: Duration,
    mut shutdown: watch::Receiver<ShutdownSignal>,
) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(interval) => {}
            _ = shutdown.changed() => return,
        }
        if let Err(e) = queue.reclaim_orphans(stale_after).await {
            queue.events_for_error("reclaiming orphaned jobs", &e);
        }
        if let Err(e) = queue.tick_schedules().await {
            queue.events_for_error("ticking the recurring schedules", &e);
        }
    }
}

/// A worker id that is distinct per process and per worker, without a
/// dependency on a hostname crate.
///
/// `HOSTNAME` is read if the environment sets it — it usually does not on
/// Linux, where it is a shell variable rather than an exported one — and
/// otherwise this falls back to the process id plus a counter, which is enough
/// to tell two workers in one process apart. It only has to be unique, and
/// `locked_by` is a diagnostic rather than a key.
fn default_worker_id(queues: &[String]) -> String {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "host".to_string());
    let queue = queues.first().map(String::as_str).unwrap_or("default");
    format!("{host}-{}-{queue}-{n}", std::process::id())
}
