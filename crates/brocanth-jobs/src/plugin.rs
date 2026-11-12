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

use std::time::Duration;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError, Services};
use brocanth::plugin_context::{ContextBuilder, ProvidesContext};
use brocanth::plugin_shutdown::ShutdownCx;

use crate::queue::Queue;
use crate::registry::JobRegistry;
use crate::worker::{Finish, WorkerConfig, WorkerHandle};

/// What the worker keeps of the grace period to give back the jobs that did
/// not finish, so they are on the queue before the process exits.
const GIVE_BACK: Duration = Duration::from_secs(2);

/// The running jobs' time when the shutdown has no budget: a day, which is
/// "as long as they take" without an `Instant` overflow.
const UNBOUNDED: Duration = Duration::from_secs(24 * 60 * 60);

/// The shutdown grace [`JobsConfig`] assumes: the engine worker's default.
const DEFAULT_GRACE: Duration = Duration::from_secs(25);

/// What [`JobsPlugin`] is built from.
pub struct JobsConfig {
    queue: Queue,
    worker: Option<(JobRegistry, WorkerConfig)>,
    migrate: bool,
    grace: Duration,
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
            grace: DEFAULT_GRACE,
        }
    }

    /// The server's shutdown grace period, so the worker stops claiming
    /// when the server starts to drain and gives its running jobs this
    /// long, less the time to give back the ones that do not finish.
    /// Twenty-five seconds by default.
    #[must_use]
    pub fn shutdown_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
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
    /// that. This exists for a test harness, where the alternative is a
    /// migration directory holding one file nobody will ever edit.
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
    grace: Duration,
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
            grace: config.grace,
            handle: None,
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        lc.services_mut().insert(self.queue.clone());
        // Ready when the queue's tables answer: a catalogue lookup, which
        // costs the same however long the queue is. With `migrating`, the
        // check creates the tables first.
        lc.ready_when(self.queue.readiness_check(self.migrate));
        Ok(())
    }

    fn start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
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
            let handle = builder
                .start()
                .map_err(|e| PluginError::new(format!("starting the job workers: {e}")))?;
            // When the server starts to drain, stop claiming then, not after
            // the listener drained. The running jobs have the grace period,
            // less the time to give back the ones that do not finish.
            handle.stop_when(
                lc.readiness().draining(),
                self.grace.saturating_sub(GIVE_BACK),
            );
            self.handle = Some(handle);
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
        match handle.finish_blocking(sd.remaining()) {
            Ok(Finish::Stopped(_) | Finish::NotWaited) => Ok(()),
            Ok(Finish::Deadline) => Err(PluginError::new(
                "the job workers were still giving jobs back at the deadline\n\
                 help: give the shutdown a longer grace period, so the workers have time to give them back",
            )),
            Err(e) => Err(PluginError::new(format!("stopping the job workers: {e}"))),
        }
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.queue.clone());
    }
}
