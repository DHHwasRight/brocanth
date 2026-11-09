//! The queue as a brocanth plugin.
//!
//! Three rules from PLAN.md §8 shape this file and none of them is negotiable.
//!
//! **The arrow points from the plugin to the core.** `brocanth-jobs` depends on
//! `brocanth`; `brocanth` does not know this crate exists and never will. An
//! application that does not write `JobsPlugin` in its `main` links no Tokio,
//! no `sqlx`, no queue.
//!
//! **A plugin arrives already configured.** [`JobsConfig`] holds a live
//! [`Queue`], because a queue is a connection pool and a connection pool is
//! made asynchronously, which a sync `Plugin::new` cannot do. So the
//! application opens the queue in its own `main`, where the runtime is, and
//! hands it over. That also means this plugin has no `plugin_from_config`
//! path — there is nothing in a TOML file that can become a pool — and the
//! database URL belongs to whatever plugin owns the connection anyway.
//!
//! **Plugins do not autowire.** This one offers no middleware and mounts
//! nothing. It starts a worker only when the application put a
//! [`WorkerConfig`] in the configuration, and it hands the
//! [`WorkerHandle`] straight back so the application owns the shutdown.
//!
//! ```no_run
//! # use brocanth_jobs::{Job, JobContext, JobRegistry, JobsConfig, JobsPlugin, Queue};
//! # use serde::{Deserialize, Serialize};
//! # #[derive(Serialize, Deserialize)] struct SendWelcome { user_id: i64 }
//! # impl Job for SendWelcome {
//! #     const NAME: &'static str = "send_welcome";
//! #     type Error = std::io::Error;
//! #     async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
//! # }
//! use brocanth::plugin_api::Plugin;
//! use brocanth::plugin_app::AppBuilder;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let queue = Queue::connect("postgres://localhost/app").await?;
//! queue.migrate().await?;
//!
//! let mut jobs = JobRegistry::new();
//! jobs.register::<SendWelcome>();
//!
//! let app = AppBuilder::new()
//!     .plugin(JobsPlugin::new(JobsConfig::new(queue.clone()).with_workers(jobs)))
//!     .build()?;
//! # let _ = app;
//! # Ok(())
//! # }
//! ```
//!
//! A handler then reaches the queue through the context, and a handler in an
//! application that never registered this plugin does not compile:
//! `cx.get::<Queue, _>()`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError, Services};
use brocanth::plugin_context::{ContextBuilder, ProvidesContext};
use brocanth::plugin_shutdown::ShutdownCx;

use crate::queue::Queue;
use crate::registry::JobRegistry;
use crate::worker::{WorkerConfig, WorkerHandle};

/// What the worker keeps of the grace period to give back the jobs that did
/// not finish, so they are on the queue before the process exits.
const GIVE_BACK: Duration = Duration::from_secs(2);

/// The running jobs' time when the shutdown has no budget: a day, which is
/// "as long as they take" without an `Instant` overflow.
const UNBOUNDED: Duration = Duration::from_secs(24 * 60 * 60);

/// What [`JobsPlugin`] is built from.
pub struct JobsConfig {
    queue: Queue,
    worker: Option<(JobRegistry, WorkerConfig)>,
    migrate: bool,
}

impl JobsConfig {
    /// An application that only enqueues. No worker starts in this process.
    ///
    /// The right shape for a web process whose jobs are run by a separate
    /// worker deployment — and the arrangement a busy application ends up with,
    /// because a job that pegs a core should not be pegging the core that is
    /// also answering requests.
    pub fn new(queue: Queue) -> Self {
        Self {
            queue,
            worker: None,
            migrate: false,
        }
    }

    /// Also run a worker in this process, for the registered job types.
    ///
    /// This is the arrangement `cargo run` wants on a laptop, and it is the
    /// same code path as production — which is the point of §4 step 9. Rails
    /// configures Solid Queue in `production.rb` alone, so a job that works in
    /// development can fail in production; there is no equivalent switch here
    /// to get wrong.
    #[must_use]
    pub fn with_workers(mut self, registry: JobRegistry) -> Self {
        self.worker = Some((registry, WorkerConfig::default()));
        self
    }

    /// Also run a worker, configured.
    #[must_use]
    pub fn with_workers_configured(mut self, registry: JobRegistry, config: WorkerConfig) -> Self {
        self.worker = Some((registry, config));
        self
    }

    /// Create the queue's tables if they are absent, in the readiness check,
    /// the first time it runs. The server awaits every check once before it
    /// listens, so the tables are there before the first job is claimed.
    ///
    /// Off by default, and it should stay off anywhere a human reviews
    /// migrations: §4.3 of the plan says the generator proposes and the
    /// developer commits. [`Queue::schema_sql`] hands out the statements for
    /// that. This exists for a test harness and a single-file SQLite
    /// deployment, where the alternative is a migration directory holding one
    /// file nobody will ever edit.
    #[must_use]
    pub fn migrating(mut self) -> Self {
        self.migrate = true;
        self
    }
}

/// Background jobs, as a plugin.
pub struct JobsPlugin {
    queue: Queue,
    worker: Option<(JobRegistry, WorkerConfig)>,
    migrate: bool,
    handle: Option<WorkerHandle>,
}

impl JobsPlugin {
    /// The queue this plugin installed, for the application's own use.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Takes the running worker's handle, for an application that stops the
    /// worker itself, before the rest of the shutdown.
    ///
    /// Without it, `begin_shutdown` stops claiming and `shutdown` waits for
    /// the running jobs within the grace period.
    pub fn take_handle(&mut self) -> Option<WorkerHandle> {
        self.handle.take()
    }

    /// The queue an earlier plugin published, if any.
    ///
    /// `register` puts the [`Queue`] in [`Services`], so a plugin that runs
    /// later — a mailer, say — finds it here instead of being handed one.
    pub fn queue_of(services: &Services) -> Option<&Queue> {
        services.get::<Queue>()
    }
}

impl Plugin for JobsPlugin {
    type Config = JobsConfig;
    /// The queue goes in the per-request context, so a handler can enqueue.
    type Provides = ProvidesContext<Queue>;
    const NAME: &'static str = "jobs";

    fn new(config: JobsConfig) -> Self {
        // So `JobsPanel::installed()` finds it.
        config.queue.install();
        Self {
            queue: config.queue,
            worker: config.worker,
            migrate: config.migrate,
            handle: None,
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        lc.services_mut().insert(self.queue.clone());
        // Ready when the jobs table answers. A lookup by primary key, so the
        // probe costs one index read however long the queue is; `stats` would
        // count every row on every probe.
        // With `migrating`, the check creates the tables first, until that
        // worked once: the server awaits every check before it listens. No
        // stage waits for it, because a stage that waits hangs a
        // current-thread runtime.
        let queue = self.queue.clone();
        let migrated = Arc::new(AtomicBool::new(!self.migrate));
        lc.ready_when(move || {
            let queue = queue.clone();
            let migrated = Arc::clone(&migrated);
            async move {
                if !migrated.load(Ordering::Acquire) {
                    queue
                        .migrate()
                        .await
                        .map_err(|e| format!("creating the jobs tables: {e}"))?;
                    migrated.store(true, Ordering::Release);
                }
                queue.ping().await.map_err(|e| e.to_string())
            }
        });
        Ok(())
    }

    fn start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // In `start`, after the server's startup gate created the tables,
        // and inside the runtime the server runs on.
        if let Some((registry, config)) = self.worker.take() {
            let mut builder = crate::worker::Worker::builder(&self.queue).registry(registry);
            builder = builder
                .queues(config.queues.clone())
                .concurrency(config.concurrency)
                .poll_interval(config.poll_interval)
                .safety_poll(config.safety_poll)
                .listen(config.listen)
                .heartbeat_interval(config.heartbeat_interval)
                .stale_after(config.stale_after)
                .maintenance_interval(config.maintenance_interval);
            if let Some(timeout) = config.job_timeout {
                builder = builder.job_timeout(timeout);
            }
            if let Some(id) = config.id.clone() {
                builder = builder.id(id);
            }
            self.handle = Some(
                builder
                    .start()
                    .map_err(|e| PluginError::new(format!("starting the job workers: {e}")))?,
            );
        }
        Ok(())
    }

    fn begin_shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        // Stop claiming now. The jobs already running have what is left of
        // the grace period, less the time to give back those that do not
        // finish: they go back to the queue with their attempt.
        if let Some(handle) = &self.handle {
            let grace = sd.remaining().unwrap_or(UNBOUNDED);
            handle.stop(grace.saturating_sub(GIVE_BACK));
        }
        Ok(())
    }

    fn shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        let Some(handle) = self.handle.take() else {
            return Ok(());
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Ok(());
        };
        // On a current-thread runtime this thread drives the I/O, so a wait
        // here never ends. The worker stops by itself, by its deadline.
        if runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {
            return Ok(());
        }
        let remaining = sd.remaining();
        let finished = async move {
            match remaining {
                Some(left) => tokio::time::timeout(left, handle.finished()).await.ok(),
                None => Some(handle.finished().await),
            }
        };
        match std::thread::scope(|scope| {
            scope
                .spawn(|| runtime.block_on(finished))
                .join()
                .map_err(|_| PluginError::new("the jobs stopping thread panicked"))
        })? {
            Some(Ok(_)) => Ok(()),
            Some(Err(e)) => Err(PluginError::new(format!("stopping the job workers: {e}"))),
            None => Err(PluginError::new(
                "the job workers were still giving jobs back at the deadline",
            )),
        }
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.queue.clone());
    }
}
