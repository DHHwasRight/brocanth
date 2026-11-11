//! `jobs` -- what `brocanth create` writes when somebody asks for a queue.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: that a pool opens asynchronously while the
//! lifecycle stages are not, so [`Queue::connect_lazy`](crate::Queue) builds
//! it and the readiness check migrates and checks it, that a worker needs a registry of job
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
//! generated plugin takes the URL, builds the pool at `before_start`, starts
//! the worker at `start` and publishes the same `Queue`. Swapping to `JobsPlugin` is a
//! change in one generated file.
//!
//! # Where the jobs live
//!
//! Three cases, one plugin file:
//!
//! * **`db` chosen**: the queue runs on the pool the `db` plugin opened
//!   (`Queue::from_pool`). One set of connections to size, and `[jobs]`
//!   has no `url`.
//! * **No `db`**: the queue opens its own pool from `[jobs] url`. A
//!   project can have a queue and no ORM.
//! * **`jobs-memory` chosen**: the queue lives in the process
//!   (`Queue::memory()`), for development and tests. `brocanth check
//!   --deploy` reports it (`deploy/development-only`).
//!
//! With `admin`, the queue is a tab at `/admin/jobs`: an integration, on by
//! default, that `src/modules/jobs.rs` turns off.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{configurable, integration, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// Where the generated queue keeps its jobs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Store {
    /// Its own pool, from `[jobs] url`.
    Url,
    /// The pool the `db` plugin opened.
    Db,
    /// The process's memory: `jobs-memory`.
    Memory,
}

impl Store {
    fn of(project: &Project) -> Self {
        if project.has("jobs-memory") {
            Store::Memory
        } else if project.has("db") {
            Store::Db
        } else {
            Store::Url
        }
    }
}

/// Background jobs, as a choice on the `brocanth create` menu.
pub struct Jobs;

impl Module for Jobs {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "jobs",
            "a queue in Postgres with retries and a dead letter; a worker in this process",
        )
        .category(Category::Enhancer)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "db",
            "the queue runs on the pool the db plugin opens, and [jobs] has no url",
        )
        .adapts_to(
            "jobs-memory",
            "the queue lives in the process, and [jobs] has no url and no migrate",
        )
        .integration(
            "admin",
            "a tab at /admin/jobs: the queues, what runs, the retries and the dead letter",
            true,
        )
        .asked_in(
            "behaviour",
            "Background jobs, with retries and a dead letter",
        )
        .recommends(
            "observability-health",
            "its readiness check, one read of the queue, is read only by /readyz; without it a deploy cannot tell that the queue is unreachable",
        )
        .krate(Dep::brocanth("brocanth-jobs").feature_with("admin", "admin"))
        // The generated job derives `Serialize` and `Deserialize`: a payload
        // is stored as JSON. The config plugin brings serde too, but only
        // when `config` is chosen, so the queue asks for its own.
        .krate(
            Dep::version("serde", "1")
                .feature("derive")
                .note("a job's payload is stored as JSON"),
        )
        // Only for the order of the `[jobs]` section and the builder line:
        // the queue reads its URL from the file the config plugin loaded.
        .after("config")
        // The plugin reads the pool from the services the `db` plugin
        // fills, so it registers after it.
        .after("db")
        .absent(
            "brocanth-jobs",
            "no queue; slow work happens inside the request that asked for it",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let store = Store::of(project);
        s.file("src/plugins/jobs/mod.rs", plugin_rs(project, store));
        s.at(Slot::PluginsMod).line("pub mod jobs;");
        configurable(s, project, "jobs", "Jobs", settings(store));
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("admin") {
            integration(s, "jobs", "admin", Slot::AdminPanels, ADMIN_TAB);
        }
    }
}

/// The queue's tab, with the `admin` module. The panel finds the queue the
/// jobs plugin installed when it renders, because the admin mounts its
/// routes before a plugin opens a queue.
const ADMIN_TAB: &str = "// The queue the jobs plugin runs, as a tab.
config = config.panel(brocanth_jobs::admin::JobsPanel::installed());";

/// `jobs-memory`: the queue in the process, for development and tests.
pub struct JobsMemory;

impl Module for JobsMemory {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "jobs-memory",
            "the queue in this process's memory; for development and tests, gone at restart",
        )
        .category(Category::Enhancer)
        .asked_in("behaviour", "")
        .part_of_but_off("jobs")
        .because("it is where the jobs module keeps its jobs")
        .krate(
            Dep::brocanth("brocanth-jobs")
                .feature("memory")
                .note("the queue in memory, for development"),
        )
        .development_only(
            "the jobs live in the process: a restart loses every waiting and every dead job, \
             and two processes do not share one queue",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // The jobs plugin reads `project.has("jobs-memory")` and opens the
        // queue in memory. This module adds the feature and the note.
        s.at(Slot::AgentsSections).block(MEMORY_AGENTS.to_string());
    }
}

/// What an agent reading AGENTS.md needs to know about `jobs-memory`.
const MEMORY_AGENTS: &str = "## Jobs in memory

The queue lives in this process (`jobs-memory`). Jobs survive no restart,
and two processes do not share them. It is for development and tests:
`brocanth check --deploy` refuses it. Run `brocanth remove jobs-memory`
before production; the queue then uses Postgres.
";

/// The `[jobs]` section of `config/development.toml`.
///
/// `migrate` is on here and would not be in production. The generator
/// proposes and the developer commits: a new project should run, and a
/// project with reviewers should turn this off and keep the statements
/// `Queue::schema_sql` hands out in a migration somebody read.
fn settings(store: Store) -> Vec<Setting> {
    let mut out = Vec::new();
    if store == Store::Url {
        out.push(
            Setting::text(
                "url",
                "postgres://localhost/development",
                "The database the queue lives in.",
            )
            .note("A URL can carry a password, so no flag sets it.")
            .secret(),
        );
    }
    if store != Store::Memory {
        out.push(
            Setting::bool(
                "migrate",
                true,
                "Create the queue's tables at startup if they are absent.",
            )
            .note(
                "Development only: see `Queue::schema_sql` for the statements to put in a\n\
                 migration instead.",
            ),
        );
    }
    out.extend([
        Setting::bool("worker", true, "Run a worker in this process.").note(
            "A busy application moves this to a separate deployment, so that a job\n\
             pegging a core is not pegging the core answering requests.",
        ),
        Setting::list(
            "queues",
            &["default"],
            "Which queues the worker claims from.",
        ),
        Setting::integer("concurrency", 5, "How many jobs the worker runs at once."),
    ]);
    out
}

fn plugin_rs(project: &Project, store: Store) -> String {
    let (derive, note) = settings_derive(project, "jobs");
    let (fields, defaults, check_url, open, ready, store_doc) = store_parts(store);
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
//! handler reaches it through `Ctx`:
//!
//! ```ignore
//! let queue = ctx.provided::<Queue, _>().expect("a dispatched request");
//! queue.enqueue(Welcome {{ user_id: 7 }}).await?;
//! ```
//!
//! `provided` compiles only because this plugin is registered. Drop it and
//! the line stops building and names `Queue`, which is the whole reason
//! the provider list lives in the application's type.
//!
//! Anything else holding the `Queue` can enqueue too -- `src/app.rs`, a
//! command under `src/commands/`:
//!
//! ```ignore
//! queue.enqueue(Welcome {{ user_id: 7 }}).await?;
//! ```
//!
//! The note here used to say a handler could not reach it, because `Ctx`
//! carried no context. It does now.
//!
//! # Where job types go
//!
//! `Welcome` is here so that this file compiles into something that runs.
//! Move it, and whatever follows it, into `src/jobs/` once there is more
//! than one -- the only thing that must stay here is the `register` call.
//!
//! **Register every job type before anything enqueues it.** A worker claims
//! only the names it has registered, so a job whose name no worker knows
//! waits on its queue until a worker that knows it starts. During a rolling
//! deploy that means the old workers leave it alone, so ship the
//! registration before you ship the first enqueue.
//!
//! # Where the jobs live
//!
{store_doc}
//!
//! The crate also ships `JobsPlugin`, and this is not that. `JobsPlugin` is
//! handed a queue that is already open, which is what an application that
//! builds its queue in `main` wants. This one builds it from the settings.

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ProvidesContext}};
use brocanth::plugin_shutdown::ShutdownCx;
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
{fields}    /// Run a worker in this process, as well as enqueuing from it.
    pub worker: bool,
    /// Which queues that worker claims from.
    pub queues: Vec<String>,
    /// How many jobs it runs at once.
    pub concurrency: usize,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
{defaults}            worker: true,
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
    /// Kept so `shutdown` can wait for the worker to give back what it
    /// holds.
    worker: Option<WorkerHandle>,
}}

/// What the worker keeps of the grace period to give back the jobs that did
/// not finish, so they are on the queue before the process exits.
const GIVE_BACK: std::time::Duration = std::time::Duration::from_secs(2);

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
{check_url}        if self.settings.worker && self.settings.queues.is_empty() {{
            return Err(PluginError::new(
                "[jobs] worker is true and queues is empty, so the worker would claim \
                 nothing; name a queue or set worker = false",
            ));
        }}
        Ok(())
    }}

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
{open}
{ready}
        // So the admin's jobs tab, which mounts before this runs, finds it.
        queue.install();

        // In the services too, for a plugin that runs a worker of its own on
        // this queue: the mail plugin sends `deliver_later` mail that way.
        // Every `before_start` runs before any `start`, so it finds it there.
        lc.services_mut().insert(queue.clone());
        self.queue = Some(queue);
        Ok(())
    }}

    fn start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // Here and not in `before_start`: `serve` runs this after its
        // startup gate, so the tables exist, and inside the runtime, so the
        // worker has somewhere to spawn. A build that never serves -- a test,
        // a command -- starts no worker. On Postgres an enqueue wakes the
        // worker (`LISTEN/NOTIFY`) rather than a poll.
        let (Some(queue), true) = (&self.queue, self.settings.worker) else {{
            return Ok(());
        }};
        let worker = Worker::builder(queue)
            .registry(registry())
            .queues(self.settings.queues.clone())
            .concurrency(self.settings.concurrency)
            .start()
            .map_err(|e| PluginError::new(format!("[jobs] starting the worker: {{e}}")))?;
        // At SIGTERM `serve` marks the application not ready at once, and
        // the worker stops claiming then, not after the listener drained.
        // The jobs already running have the grace period, less the time it
        // takes to give back the ones that do not finish: those go back to
        // the queue with their attempt, and another pod runs them.
        let grace = crate::plugins::shutdown_grace().map_err(PluginError::new)?;
        worker.stop_when(lc.readiness().draining(), grace.saturating_sub(GIVE_BACK));
        self.worker = Some(worker);
        Ok(())
    }}

    fn begin_shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {{
        // Nothing new from here, if the drain above has not stopped it yet.
        if let Some(worker) = &self.worker {{
            let grace = sd.remaining().unwrap_or(crate::app::SHUTDOWN_GRACE);
            worker.stop(grace.saturating_sub(GIVE_BACK));
        }}
        Ok(())
    }}

    fn shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {{
        let Some(worker) = self.worker.take() else {{
            return Ok(());
        }};
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {{
            return Ok(());
        }};
        // On a current-thread runtime this thread drives the I/O, so a wait
        // here never ends. The worker stops by itself, by its deadline.
        if runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {{
            return Ok(());
        }}
        // On a multi-thread runtime the workers drive it, and a scoped
        // thread waits without blocking one of them, inside what is left.
        let remaining = sd.remaining();
        let finished = async move {{
            match remaining {{
                Some(left) => tokio::time::timeout(left, worker.finished()).await.ok(),
                None => Some(worker.finished().await),
            }}
        }};
        let ended = std::thread::scope(|scope| {{
            scope
                .spawn(|| runtime.block_on(finished))
                .join()
                .map_err(|_| PluginError::new("[jobs] the stopping thread panicked"))
        }})?;
        match ended {{
            Some(Ok(report)) => {{
                let (finished, returned) = (report.finished, report.returned);
                crate::plugins::say!(
                    info,
                    "[jobs] worker stopped: {{finished}} finished, {{returned}} back on the queue";
                    finished,
                    returned
                );
                Ok(())
            }}
            Some(Err(e)) => Err(PluginError::new(format!("[jobs] stopping the worker: {{e}}"))),
            None => Err(PluginError::new(
                "[jobs] the worker was still giving jobs back at the deadline",
            )),
        }}
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

/// The parts of the plugin file that depend on where the jobs live: the
/// settings' fields, their defaults, the URL check, the opening of the
/// queue, the readiness check and the module doc.
fn store_parts(store: Store) -> (String, String, String, String, String, String) {
    let url_field =
        "    /// The database the queue lives in. Its own URL, not the models': a\n    \
                     /// queue and an ORM can share one database and need not.\n    \
                     pub url: String,\n";
    let migrate_field = "    /// Create the queue's tables at startup if they are absent.\n    \
                         pub migrate: bool,\n";
    let url_default = "            url: \"postgres://localhost/development\".to_string(),\n";
    let migrate_default = "            migrate: true,\n";
    let check_url = "        if self.settings.url.is_empty() {\n            \
                     return Err(PluginError::new(\n                \
                     \"[jobs] url is required; the queue is a table and needs a database\\n\\\n                 \
                     help: set `url` under [jobs] in the config file, or the variable for `jobs.url`\",\n            \
                     ));\n        }\n";
    let migrating_ready = |what: &str| {
        format!(
            "        // Ready when {what} answers: one cheap read. With `migrate`, the\n        \
             // first check creates the tables, and the first check is the one\n        \
             // `serve` awaits before it listens: an unreachable database stops\n        \
             // the start there.\n        \
             let probe = queue.clone();\n        \
             let migrate = self.settings.migrate;\n        \
             let migrated = std::sync::Arc::new(tokio::sync::OnceCell::new());\n        \
             lc.ready_when(move || {{\n            \
             let probe = probe.clone();\n            \
             let migrated = std::sync::Arc::clone(&migrated);\n            \
             async move {{\n                \
             if migrate {{\n                    \
             migrated\n                        \
             .get_or_try_init(|| probe.migrate())\n                        \
             .await\n                        \
             .map_err(|e| format!(\"creating the jobs tables: {{e}}\"))?;\n                \
             }}\n                \
             probe.ping().await.map_err(|e| e.to_string())\n            \
             }}\n        \
             }});\n"
        )
    };
    match store {
        Store::Url => (
            format!("{url_field}{migrate_field}"),
            format!("{url_default}{migrate_default}"),
            check_url.to_string(),
            "        // The lifecycle hooks are synchronous and opening a pool is not, so\n        \
             // the pool opens lazily and nothing here waits. Waiting would hang a\n        \
             // current-thread runtime, where this thread drives the I/O. The pool\n        \
             // spawns its upkeep task, so it needs the runtime entered.\n        \
             let runtime = tokio::runtime::Handle::try_current().map_err(|e| {\n            \
             PluginError::new(format!(\n                \
             \"[jobs] needs a Tokio runtime at startup to open its pool ({e})\"\n            \
             ))\n        \
             })?;\n        \
             let queue = {\n            \
             let _entered = runtime.enter();\n            \
             Queue::connect_lazy(&self.settings.url)\n                \
             .map_err(|e| PluginError::new(format!(\"[jobs] {e}\")))?\n        \
             };\n"
                .to_string(),
            migrating_ready("the queue"),
            "//! In Postgres, on a pool of its own opened from `[jobs] url`. Add the `db`\n\
             //! module and the queue shares its pool instead."
                .to_string(),
        ),
        Store::Db => (
            migrate_field.to_string(),
            migrate_default.to_string(),
            String::new(),
            "        // The pool the `db` plugin opened, which registers before this one:\n        \
             // one set of connections for the pages and the queue.\n        \
             let db = lc.require_service::<brocanth_db::Db>()?;\n        \
             let queue = Queue::from_pool(db.pool().clone());\n"
                .to_string(),
            migrating_ready("the queue"),
            "//! In Postgres, on the pool the `db` plugin opened: one set of connections\n\
             //! for the pages and the queue, so `[db] max_connections` sizes both."
                .to_string(),
        ),
        Store::Memory => (
            String::new(),
            String::new(),
            String::new(),
            "        // In this process's memory (`jobs-memory`): for development and\n        \
             // tests only. A restart loses every job, and `brocanth check\n        \
             // --deploy` refuses it.\n        \
             let queue = Queue::memory();\n"
                .to_string(),
            "        // Ready while the queue answers, which in memory is always.\n        \
             let probe = queue.clone();\n        \
             lc.ready_when(move || {\n            \
             let probe = probe.clone();\n            \
             async move { probe.ping().await.map_err(|e| e.to_string()) }\n        \
             });\n"
                .to_string(),
            "//! In this process's memory (`jobs-memory`), for development and tests.\n\
             //! A restart loses every job. `brocanth remove jobs-memory` moves the queue\n\
             //! to Postgres."
                .to_string(),
        ),
    }
}
