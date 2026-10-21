//! `jobs` -- what `brocanth create` writes when somebody asks for a queue.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: that [`Queue::connect`](crate::Queue) is
//! async while `Plugin::new` is not, that a worker needs a registry of job
//! types the project owns, that [`JobsConfig`](crate::JobsConfig) holds a
//! live queue and therefore cannot be read out of a TOML file. Those move
//! when this crate moves.
//!
//! # Why the generated plugin is not `JobsPlugin`
//!
//! Same split as the cache. [`JobsPlugin`](crate::JobsPlugin) is handed a
//! queue that is already open, because a pool is made asynchronously and
//! `Plugin::new` is not. That is right for an application that builds its
//! queue in `main` -- a shared pool, a second database, a worker process
//! with no web server. A new project has one URL in a config file, so the
//! generated plugin takes the URL, opens the pool at `before_start`, starts
//! the worker and publishes the same `Queue`. Swapping to `JobsPlugin` is a
//! change in one generated file.
//!
//! # Why it does not need `model`
//!
//! The queue makes its own pool from its own URL and uses `sqlx` directly,
//! so a project can have a queue and no ORM. Requiring `model` would put
//! SeaORM in the binary of an application that asked for background work,
//! which is the opposite of what the menu is for.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{plugin_line, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// Background jobs, as a choice on the `brocanth create` menu.
pub struct Jobs;

impl Module for Jobs {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "jobs",
            "a queue in Postgres with retries and a dead letter; a worker in this process",
        )
        .asked_in(
            "behaviour",
            "Background jobs, with retries and a dead letter",
        )
        .krate(Dep::brocanth("brocanth-jobs"))
        // Only for the order of the `[jobs]` section and the builder line:
        // the queue reads its URL from the file the config plugin loaded.
        .after("config")
        .absent(
            "brocanth-jobs",
            "no queue; slow work happens inside the request that asked for it",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/jobs/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod jobs;");
        s.at(Slot::AppPlugins)
            .block(plugin_line(project, "jobs", "Jobs"));
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("config") {
            s.at(Slot::ConfigSections).block(SECTION.to_string());
        }
    }
}

/// The `[jobs]` section of `config/development.toml`.
///
/// `migrate` is on here and would not be in production. The generator
/// proposes and the developer commits: a new project should run, and a
/// project with reviewers should turn this off and keep the statements
/// `Queue::schema_sql` hands out in a migration somebody read.
const SECTION: &str = "\n[jobs]\n\
                       url = \"postgres://localhost/development\"\n\
                       # Create the three tables at startup if they are absent. Development\n\
                       # only: see `Queue::schema_sql` for the statements to put in a\n\
                       # migration instead.\n\
                       migrate = true\n\
                       # Run a worker in this process. A busy application moves this to a\n\
                       # separate deployment, so that a job pegging a core is not pegging\n\
                       # the core answering requests.\n\
                       worker = true\n\
                       queues = [\"default\"]\n\
                       concurrency = 5\n";

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "jobs");
    format!(
        r##"//! Background jobs, as this application registers them.
//!
//! A job is a struct that serialises, plus one `perform`. Enqueuing takes the
//! struct itself, so a payload a job cannot accept is a struct-literal error
//! rather than a decode failure on a worker at three in the morning.
//!
//! `brocanth-jobs` is in `Cargo.toml` because this file is here, and through
//! no other route. Delete the registration in `src/app.rs` and delete this
//! file, and the application runs no jobs and links nothing to run them with.
//!
//! # Enqueuing, and what does not work yet
//!
//! This plugin publishes the `Queue` on the per-request context, and a
//! handler in this project **cannot reach it today**. The `Ctx` in
//! `src/pages/mod.rs` carries the verb, the path, the parameters and the
//! partial depth, and has no extractor on it. `cx.extract()` works in
//! `plugins::announce!`, at startup, where the context builder is in
//! scope -- which is the one place this project calls it.
//!
//! So enqueue from `src/app.rs`, from a command under `src/commands/`, or
//! from anything else holding the `Queue`:
//!
//! ```ignore
//! queue.enqueue(Welcome {{ user_id: 7 }}).await?;
//! ```
//!
//! The plugin half is right and the handler half is missing: `Ctx` needs
//! to carry the plugin context and expose an extractor. Until it does,
//! a line claiming otherwise would be a line that does not compile.
//!
//! # Where job types go
//!
//! `Welcome` is here so that this file compiles into something that runs.
//! Move it, and whatever follows it, into `src/jobs/` once there is more
//! than one -- the only thing that must stay here is the `register` call.
//!
//! **Register every job type before anything enqueues it.** A worker claims
//! by queue and not by name, so it will take a job whose name it does not
//! know, and a name it cannot resolve is a terminal failure: the row goes
//! straight to the dead letter rather than waiting for a worker that knows
//! it. The payload is kept and `Queue::requeue_dead` replays it, so nothing
//! is lost -- but the job does not run when you expected it to. During a
//! rolling deploy that means the old workers, so ship the registration
//! before you ship the first enqueue.
//!
//! The crate also ships `JobsPlugin`, and this is not that. `JobsPlugin` is
//! handed a queue that is already open, which is what an application with a
//! shared pool or a worker-only process wants. This one takes a URL, because
//! a new project has a URL.

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ProvidesContext}};
use brocanth_jobs::{{Job, JobContext, JobRegistry, Queue, Worker, WorkerHandle}};
use serde::{{Deserialize, Serialize}};

// ---------------------------------------------------------------------------
// A job
// ---------------------------------------------------------------------------

/// Says hello, slowly, somewhere that is not a request.
#[derive(Serialize, Deserialize)]
pub struct Welcome {{
    /// Who to welcome.
    pub user_id: i64,
}}

impl Job for Welcome {{
    const NAME: &'static str = "welcome";
    /// Which queue it waits on. The worker below claims from the queues the
    /// config names, so a job on a queue nobody claims waits for ever.
    const QUEUE: &'static str = "default";
    type Error = std::io::Error;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {{
        println!("welcome, user {{}}", self.user_id);
        Ok(())
    }}
}}

/// Every job type this application can run.
///
/// The one list that has to be kept: a worker claims a row by name and needs
/// the type to decode it. Adding a job is a type above and a line here.
fn registry() -> JobRegistry {{
    let mut registry = JobRegistry::new();
    registry.register::<Welcome>();
    registry
}}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// The database the queue lives in. Its own URL, not the models': a
    /// queue and an ORM can share one database and need not.
    pub url: String,
    /// Create the three tables at startup if they are absent.
    pub migrate: bool,
    /// Run a worker in this process, as well as enqueuing from it.
    pub worker: bool,
    /// Which queues that worker claims from.
    pub queues: Vec<String>,
    /// How many jobs it runs at once.
    pub concurrency: usize,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            url: "postgres://localhost/development".to_string(),
            migrate: true,
            worker: true,
            queues: vec!["default".to_string()],
            concurrency: 5,
        }}
    }}
}}

pub struct Jobs {{
    settings: Settings,
    /// Opened at `before_start` and not before, so this is an `Option` for
    /// exactly the length of the build.
    queue: Option<Queue>,
    /// Kept so the worker can be stopped. The core's shutdown hook lands
    /// with the server layer; until then, dropping the application stops
    /// the process and the worker with it.
    worker: Option<WorkerHandle>,
}}

impl Plugin for Jobs {{
    type Config = Settings;
    type Provides = ProvidesContext<Queue>;
    const NAME: &'static str = "jobs";

    fn new(config: Settings) -> Self {{
        Self {{
            settings: config,
            queue: None,
            worker: None,
        }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.settings.url.is_empty() {{
            return Err(PluginError::new(
                "[jobs] url is required; the queue is a table and needs a database",
            ));
        }}
        if self.settings.worker && self.settings.queues.is_empty() {{
            return Err(PluginError::new(
                "[jobs] worker is true and queues is empty, so the worker would claim \
                 nothing; name a queue or set worker = false",
            ));
        }}
        Ok(())
    }}

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // The lifecycle hooks are synchronous and opening a pool is not. A
        // server process has a runtime, `block_in_place` is wrong on a
        // current-thread one, so take the handle and block the builder on a
        // scoped thread. This is startup, where blocking is expected.
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {{
            PluginError::new(format!(
                "[jobs] needs a Tokio runtime at startup to open its pool ({{e}})"
            ))
        }})?;

        let url = self.settings.url.clone();
        let migrate = self.settings.migrate;
        let queue = std::thread::scope(|scope| {{
            scope
                .spawn(|| {{
                    runtime.block_on(async move {{
                        let queue = Queue::connect(&url).await?;
                        if migrate {{
                            queue.migrate().await?;
                        }}
                        Ok::<_, brocanth_jobs::Error>(queue)
                    }})
                }})
                .join()
                .map_err(|_| PluginError::new("the jobs connection thread panicked"))?
                .map_err(|e| PluginError::new(format!("[jobs] connecting: {{e}}")))
        }})?;

        if self.settings.worker {{
            // `enter` so that `start` finds the runtime to spawn on. The
            // worker is woken by the enqueue itself on Postgres rather than
            // polling; see `brocanth_jobs::listen`.
            let _guard = runtime.enter();
            self.worker = Some(
                Worker::builder(&queue)
                    .registry(registry())
                    .queues(self.settings.queues.clone())
                    .concurrency(self.settings.concurrency)
                    .start()
                    .map_err(|e| PluginError::new(format!("[jobs] starting the worker: {{e}}")))?,
            );
        }}

        self.queue = Some(queue);
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `before_start` runs before the server is handed the application,
        // so the queue is here by the time any request builds a context.
        if let Some(queue) = &self.queue {{
            cx.insert(queue.clone());
        }}
    }}
}}
"##
    )
}
