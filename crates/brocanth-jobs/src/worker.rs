//! The worker: this crate's settings over the engine's worker, plus the
//! recurring schedules.
//!
//! # The runtime question, answered
//!
//! **Workers run on Tokio, and by default on the same runtime that serves
//! requests.**
//!
//! The core deliberately contains no async runtime — `brocanth`'s own test
//! fails the build if one appears in its dependency tree — so bringing one in
//! is this crate's decision to own.
//!
//! *Why Tokio.* The workspace already resolves `sqlx` 0.9 through SeaORM, which
//! is pinned to `runtime-tokio-rustls`. A queue on another runtime in the same
//! process as a Tokio-flavoured driver means two reactors, two thread pools
//! and a connection pool whose timers belong to the wrong one.
//!
//! *Why the same runtime.* [`Worker::start`] spawns onto
//! `tokio::runtime::Handle::current()`, which in a server process is the
//! runtime already running the handlers. One runtime, with concurrency
//! limited where it belongs (`WorkerBuilder::concurrency`), is the simpler
//! and more controllable arrangement.
//!
//! *What it costs.* A job that blocks the thread stalls request handlers, and
//! a job that burns CPU competes with request latency. The way out is the one
//! Rails uses and it is not a runtime: run workers in their own process.
//! [`Worker::run`] is a plain `async fn`, so a `jobs` binary is a `main`, a
//! [`Queue`], a [`JobRegistry`] and a call.
//!
//! # What the engine does
//!
//! The claim loop, the lease and its heartbeat, the retry and the dead letter
//! are `brocanth_queue::Worker`'s. This worker maps its settings onto the
//! engine's, gives it the registry, and adds three things:
//!
//! * the recurring schedules, ticked every `maintenance_interval`;
//! * on Postgres, a listener, so an enqueue wakes the worker
//!   (`LISTEN/NOTIFY`), and the outbox relay, so a job enqueued with
//!   `in_tx` reaches the queue after its transaction commits;
//! * the [`JobEvent`]s, from the engine's observer.
//!
//! # Shutdown
//!
//! [`WorkerHandle::shutdown`] (or [`WorkerHandle::stop`], which does not
//! wait) stops claiming at once, waits up to a deadline for the jobs already
//! running, and then gives the rest back **without spending an attempt**. The
//! worker chose to stop; the job did not fail, and charging it an attempt
//! every time somebody deploys is how a four-minute job ends up in the dead
//! letter after a busy afternoon of releases.
//!
//! # A worker that dies
//!
//! A running job holds a lease of `stale_after`, renewed every
//! `heartbeat_interval`. A worker that is killed stops renewing, the lease
//! runs out, and the job is claimable again by any worker. Nothing has to
//! notice: the lease is the job's visibility, so a job held by a machine that
//! is never coming back cannot stay held.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use brocanth_queue::{Observer, Settle};
use tokio::sync::watch;

use crate::error::{Error, Result};
use crate::events::{JobEvent, JobEvents};
use crate::job::JobFailure;
use crate::queue::Queue;
use crate::registry::{JobRegistry, RunEnv};

/// How a worker is set up.
#[derive(Clone, Debug)]
pub struct WorkerConfig {
    /// Which queues to claim from. Defaults to `["default"]`.
    pub queues: Vec<String>,
    /// How many jobs may run at once on this worker.
    pub concurrency: usize,
    /// How long to wait after finding nothing before looking again, on a
    /// store that cannot announce new work, or with `listen` off.
    pub poll_interval: Duration,
    /// The longest a listening worker sleeps before looking anyway.
    ///
    /// A backstop and not the mechanism. A notification only shortens the
    /// sleep; this caps it, so a notification nobody heard — sent while the
    /// listener was reconnecting — costs seconds rather than a job.
    pub safety_poll: Duration,
    /// Whether to listen for new work instead of asking for it.
    ///
    /// On by default, and ignored on a store with nothing to listen to.
    /// Turning it off is for a deployment whose connection limit cannot
    /// spare one connection per worker process: that is what a listener
    /// costs, because `LISTEN` holds a session.
    pub listen: bool,
    /// How often to renew the lease of each job this worker holds.
    pub heartbeat_interval: Duration,
    /// How long a lease lasts without a renewal: how quiet a worker must go
    /// before another worker may take its jobs.
    ///
    /// Must be at least three times `heartbeat_interval`, or one late
    /// renewal loses a job a worker is still running. The builder refuses a
    /// smaller value.
    pub stale_after: Duration,
    /// How often to tick the recurring schedules and sweep the store.
    pub maintenance_interval: Duration,
    /// How long one job may run before it is stopped as timed out.
    ///
    /// `None` by default: a timeout that is too short turns slow work into
    /// failed work, and this crate will not guess a number. Setting one is how
    /// you stop a job that hangs on a socket from holding a slot forever —
    /// the lease keeps being renewed while it hangs, so nothing else will.
    pub job_timeout: Option<Duration>,
    /// A name for this worker, on every job it claims and in events.
    /// Defaults to `"<host>-<pid>-<first queue>-<n>"`.
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

    /// How often to renew the lease of a running job.
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

    /// How often to tick the schedules and sweep the store.
    pub fn maintenance_interval(mut self, interval: Duration) -> Self {
        self.config.maintenance_interval = interval;
        self
    }

    /// Stops a job that has run for longer than this. Off by default.
    pub fn job_timeout(mut self, timeout: Duration) -> Self {
        self.config.job_timeout = Some(timeout);
        self
    }

    /// Names this worker, which is what the running screen shows.
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

    /// Checks the settings and builds the worker without starting it.
    pub fn build(self) -> Result<Worker> {
        if self.registry.is_empty() {
            return Err(Error::UnknownJob {
                name: "<none>".to_string(),
                known: "nothing: call `register::<J>()` before `build`".to_string(),
            });
        }
        if self.config.queues.is_empty() {
            return Err(Error::Backend(
                "the worker has no queue to claim from\n\
                 help: name at least one with `queues([\"default\"])`"
                    .to_string(),
            ));
        }
        if self.config.stale_after < self.config.heartbeat_interval * 3 {
            return Err(Error::Backend(format!(
                "stale_after ({:?}) must be at least three times heartbeat_interval ({:?}), \
                 or a worker that is merely busy will have its jobs taken from under it\n\
                 help: raise stale_after or lower heartbeat_interval",
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
    pub fn start(self) -> Result<WorkerHandle> {
        self.build()?.start()
    }
}

/// A worker, built and not yet running.
pub struct Worker {
    queue: Queue,
    registry: JobRegistry,
    config: WorkerConfig,
    id: String,
}

impl Worker {
    /// A builder for a worker on `queue`.
    pub fn builder(queue: &Queue) -> WorkerBuilder {
        WorkerBuilder {
            queue: queue.clone(),
            registry: JobRegistry::new(),
            config: WorkerConfig::default(),
        }
    }

    /// This worker's name.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Spawns the worker on the current Tokio runtime and returns a handle
    /// that stops it.
    pub fn start(self) -> Result<WorkerHandle> {
        let handle = tokio::runtime::Handle::try_current().map_err(|e| {
            Error::NoRuntime(format!(
                "{e}; call this from inside `#[tokio::main]`, or use `Worker::run`, \
                 which is a plain future you can drive yourself"
            ))
        })?;
        let engine = self.engine()?;
        let (shutdown_tx, shutdown_rx) = watch::channel(None);
        let id = self.id.clone();
        let join = handle.spawn(async move { self.run_with(engine, shutdown_rx).await });
        Ok(WorkerHandle {
            id,
            shutdown_tx,
            join,
        })
    }

    /// Runs the worker until `shutdown` completes, then gives running jobs
    /// `grace` to finish.
    ///
    /// For a `jobs` binary: `worker.run(tokio::signal::ctrl_c().map(drop),
    /// Duration::from_secs(30)).await`.
    pub async fn run(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send,
        grace: Duration,
    ) -> Result<ShutdownReport> {
        let engine = self.engine()?;
        let (tx, rx) = watch::channel(None);
        let worker = tokio::spawn(async move { self.run_with(engine, rx).await });
        shutdown.await;
        let _ = tx.send(Some(grace));
        worker
            .await
            .map_err(|e| Error::NoRuntime(format!("the worker task did not finish: {e}")))?
    }

    /// The engine's worker, built and checked, but not started: a wrong
    /// setting fails here, before anything is spawned.
    fn engine(&self) -> Result<brocanth_queue::Worker> {
        let config = &self.config;
        let listening = config.listen && self.queue.backend() == "postgres";
        let engine_config = brocanth_queue::WorkerConfig {
            name: self.id.clone(),
            queue: config.queues[0].clone(),
            extra_queues: config.queues[1..].to_vec(),
            concurrency: config.concurrency.max(1) as u32,
            lease: config.stale_after,
            heartbeat: config.heartbeat_interval,
            poll_idle: if listening {
                config.safety_poll
            } else {
                config.poll_interval
            },
            sweep_every: config.maintenance_interval,
            ..brocanth_queue::WorkerConfig::default()
        };
        let registry = self.registry.build(RunEnv {
            timeout: config.job_timeout,
        });
        let engine = brocanth_queue::Worker::new(
            Arc::clone(self.queue.engine()),
            Arc::new(registry),
            engine_config,
        )
        .map_err(|e| Error::Backend(format!("{e}\nhelp: check the worker's settings")))?;
        Ok(engine.observe(Arc::new(Bridge {
            events: self.queue.event_sink(),
        })))
    }

    async fn run_with(
        self,
        engine: brocanth_queue::Worker,
        shutdown: watch::Receiver<Option<Duration>>,
    ) -> Result<ShutdownReport> {
        let Worker {
            queue, config, id, ..
        } = self;
        let events = queue.event_sink();
        events.handle(JobEvent::WorkerStarted {
            worker: &id,
            queues: &config.queues,
        });

        #[allow(unused_mut)]
        let mut engine = engine;
        #[cfg(feature = "postgres")]
        if let Some(pool) = queue.pg_pool() {
            if config.listen {
                match brocanth_queue_postgres::wakeups(pool).await {
                    Ok(rx) => {
                        events.handle(JobEvent::ListenerConnected);
                        engine = engine.wake_on(rx);
                    }
                    Err(e) => events.handle(JobEvent::ListenerFailed {
                        error: e.to_string(),
                    }),
                }
            }
            // The relay moves what `in_tx` wrote to the outbox into the
            // queue. It runs in every worker: the outbox claim takes rows
            // with `SKIP LOCKED` under a lease, so several relays share the
            // work without a leader.
            let relay = brocanth_queue::Relay::new(
                Arc::new(brocanth_queue_postgres::PgOutbox::new(pool.clone())),
                Arc::clone(queue.engine()),
                brocanth_queue::RelayConfig::default(),
            );
            engine = engine.relay(relay);
        }

        let ticker = tokio::spawn(tick_loop(
            queue.clone(),
            config.maintenance_interval,
            shutdown.clone(),
        ));
        let report = engine.drain(shutdown).await;
        ticker.abort();
        let _ = ticker.await;

        let finished = report.finished as usize;
        let returned = report.returned;
        events.handle(JobEvent::WorkerStopped {
            worker: &id,
            finished,
            returned,
        });
        Ok(ShutdownReport { finished, returned })
    }
}

/// Ticks the schedules every `every`, from the first moment, until a stop.
async fn tick_loop(queue: Queue, every: Duration, mut stop: watch::Receiver<Option<Duration>>) {
    let mut tick = tokio::time::interval(every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    while stop.borrow().is_none() {
        tokio::select! {
            _ = tick.tick() => {
                if let Err(error) = queue.tick_schedules().await {
                    queue.event_sink().handle(JobEvent::InternalError {
                        during: "ticking the recurring schedules",
                        error: &error,
                    });
                }
            }
            changed = stop.changed() => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}

/// The engine's observer, written as [`JobEvent`]s.
struct Bridge {
    events: Arc<dyn JobEvents>,
}

impl Observer for Bridge {
    fn on_start(&self, cx: &brocanth_queue::JobContext) {
        self.events.handle(JobEvent::Started {
            id: cx.id().clone(),
            name: cx.kind(),
            attempt: cx.attempt(),
            worker: cx.worker(),
        });
    }

    fn on_settle(&self, cx: &brocanth_queue::JobContext, outcome: &Settle, took: Duration) {
        match outcome {
            Settle::Done => self.events.handle(JobEvent::Completed {
                id: cx.id().clone(),
                name: cx.kind(),
                took,
                attempt: cx.attempt(),
            }),
            Settle::Retry { at, failure } => {
                let failure = JobFailure::from_queue(failure);
                self.events.handle(JobEvent::Retrying {
                    id: cx.id().clone(),
                    name: cx.kind(),
                    attempt: cx.attempt(),
                    delay: at
                        .duration_since(SystemTime::now())
                        .unwrap_or(Duration::ZERO),
                    failure: &failure,
                });
            }
            Settle::Dead { failure } => {
                let failure = JobFailure::from_queue(failure);
                self.events.handle(JobEvent::DeadLettered {
                    id: cx.id().clone(),
                    name: cx.kind(),
                    attempts: cx.attempt(),
                    failure: &failure,
                });
            }
            Settle::Release { .. } => {}
        }
    }

    fn on_backend_error(&self, op: &'static str, error: &brocanth_queue::BackendError) {
        let error = Error::Backend(error.to_string());
        self.events.handle(JobEvent::InternalError {
            during: op,
            error: &error,
        });
    }
}

/// What a worker did before it stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Jobs that ran to an outcome while the worker ran: done, a retry or
    /// the dead letter.
    pub finished: usize,
    /// Jobs given back at the stop, without an attempt spent.
    pub returned: u64,
}

/// A running worker.
pub struct WorkerHandle {
    id: String,
    shutdown_tx: watch::Sender<Option<Duration>>,
    join: tokio::task::JoinHandle<Result<ShutdownReport>>,
}

impl WorkerHandle {
    /// The worker's name.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Stops the worker and waits for it: no new claims, `grace` for the
    /// running jobs, the rest given back.
    pub async fn shutdown(self, grace: Duration) -> Result<ShutdownReport> {
        self.stop(grace);
        self.finished().await
    }

    /// Asks the worker to stop, and returns at once. A second call does
    /// nothing: the first grace stands.
    pub fn stop(&self, grace: Duration) {
        stop(&self.shutdown_tx, grace);
    }

    /// Stops the worker when `signal` completes, as [`WorkerHandle::stop`]
    /// would.
    pub fn stop_when(
        &self,
        signal: impl std::future::Future<Output = ()> + Send + 'static,
        grace: Duration,
    ) {
        let tx = self.shutdown_tx.clone();
        tokio::spawn(async move {
            tokio::select! {
                () = signal => stop(&tx, grace),
                () = tx.closed() => {}
            }
        });
    }

    /// Waits for the worker to stop on its own, after a [`WorkerHandle::stop`].
    pub async fn finished(self) -> Result<ShutdownReport> {
        self.join
            .await
            .map_err(|e| Error::NoRuntime(format!("the worker task did not finish: {e}")))?
    }

    /// Whether the worker has stopped.
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    /// Stops the worker at once, without a grace. The jobs it held run
    /// again when their leases run out.
    pub fn abort(self) {
        self.join.abort();
    }
}

fn stop(tx: &watch::Sender<Option<Duration>>, grace: Duration) {
    tx.send_if_modified(|signal| {
        if signal.is_some() {
            return false;
        }
        *signal = Some(grace);
        true
    });
}

fn default_worker_id(queues: &[String]) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "host".to_string());
    let queue = queues.first().map(String::as_str).unwrap_or("default");
    format!("{host}-{}-{queue}-{n}", std::process::id())
}
