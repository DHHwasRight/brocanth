//! `cache` and `cache-memory` -- what `brocanth create` writes when somebody
//! asks for a cache.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: the name `PgCache::connect_lazy`, the shape of
//! `CacheHandle`, the fact that the lifecycle hook is synchronous while a
//! pool is not. Those move when this crate moves, and a generator living in
//! another repository would learn about it from a bug report.
//!
//! # One plugin file, three backends
//!
//! `cache` writes `src/plugins/cache/mod.rs` over Postgres. A child module
//! picks another backend: `cache-memory` (here) or `cache-redis`
//! (`brocanth-cache-redis`). The child writes the plugin file itself, and
//! `cache` then writes none, so each backend's text lives in the crate of
//! that backend. Every variant goes through [`write_plugin`], so they share
//! the handle, the sweep and the readiness shape. A child is a module and
//! not a setting because a backend changes `Cargo.toml`: `brocanth-cache`
//! takes its `postgres` feature only when no child replaces Postgres.
//!
//! With `db`, the Postgres cache reuses the `db` plugin's pool and has no
//! `url` of its own: one set of connections to size, not two against one
//! database.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{configurable, integration, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// The cache, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct Cache;

/// The children that replace the Postgres cache. With one of them, `cache`
/// writes no plugin file and takes no `postgres` feature.
pub const BACKENDS: [&str; 2] = ["cache-memory", "cache-redis"];

impl Module for Cache {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "cache",
            "a cache on an unlogged Postgres table, not Redis; without it no cache at all",
        )
        .category(Category::Enhancer)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "db",
            "the cache reuses the `db` pool and has no `url` of its own",
        )
        .adapts_to(
            "cache-memory",
            "`cache-memory` writes the plugin file, over this process's memory",
        )
        .adapts_to(
            "cache-redis",
            "`cache-redis` writes the plugin file, over a Redis or Valkey server",
        )
        .integration(
            "admin",
            "a `cache` tab in the admin: purge expired entries, clear a namespace you name",
            true,
        )
        .asked_in("data", "A cache, in the database you already have")
        .recommends(
            "observability-health",
            "its readiness check is read only by /readyz; without it a deploy cannot tell that the cache table is unreachable",
        )
        .krate(
            Dep::brocanth("brocanth-cache")
                .no_default_features()
                .feature("runtime")
                .feature_unless(BACKENDS[0], "postgres")
                .feature_unless(BACKENDS[1], "postgres")
                .feature_with("admin", "admin"),
        )
        // Only for the order of the `[cache]` section and the builder line:
        // the cache reads its URL from the file the config plugin loaded, so
        // it belongs under it rather than above it.
        .after("config")
        // With `db`, the plugin reads the pool the `db` plugin publishes in
        // its `before_start`, so it registers after it.
        .after("db")
        .absent(
            "brocanth-cache",
            "no cache; every read does the work a hit would have skipped",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        // The tab reads the cache the plugin installs, whatever the backend,
        // so the parent writes it and no child does.
        if project.has("admin") {
            integration(s, "cache", "admin", Slot::AdminPanels, ADMIN_TAB);
        }
        // The child that replaces Postgres writes the plugin file instead.
        // `has` on each, and not `any`, so the manifest test sees both asked.
        let memory = project.has(BACKENDS[0]);
        let redis = project.has(BACKENDS[1]);
        if memory || redis {
            return;
        }
        if project.has("db") {
            write_plugin(s, project, postgres_on_db());
        } else {
            write_plugin(s, project, postgres());
        }
    }
}

/// The cache tab, with the `admin` module. The panel finds the plugin's
/// cache when it renders, because the admin's `mount` runs before the
/// plugin opens the cache at `before_start`.
const ADMIN_TAB: &str = "// The cache, as a tab: Purge expired, and Clear for each namespace you
// name with `.namespace(..)`.
config = config.panel(brocanth_cache::admin::CachePanel::installed());";

/// `cache-memory`: the cache in the process's memory.
pub struct CacheMemory;

impl Module for CacheMemory {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "cache-memory",
            "the cache in this process's memory, for tests and development; never in production",
        )
        .category(Category::Enhancer)
        .part_of_but_off("cache")
        .because("it is a backend for the cache, so there is nothing to store in it without one")
        .asked_in("data", "")
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .conflicts(
            "cache-redis",
            "a project has one cache backend, and both write `src/plugins/cache/mod.rs`",
        )
        .development_only(
            "the cache is one process's memory: each server has its own, an invalidation reaches one of them, and a restart empties it",
        )
        .after("cache")
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        write_plugin(s, project, memory());
    }
}

/// One way to open the cache, as the text of the plugin file
/// `src/plugins/cache/mod.rs`. [`write_plugin`] fills the shared rest.
///
/// Each text is already indented for its place, and every line fits in
/// 100 columns, so the file passes `cargo fmt` unchanged.
pub struct Backend {
    /// The paragraphs after the file's first line, each line starting
    /// with `//!`.
    pub doc: &'static str,
    /// Items to import from `brocanth_cache` besides `CacheHandle` (and
    /// `Sweeper`, which [`write_plugin`] adds with `sweep`).
    pub cache_items: &'static [&'static str],
    /// `use` lines for other crates, each ending in a newline. They sort
    /// after `brocanth_cache`.
    pub uses: &'static str,
    /// The settings, as `snippet::configurable` declares them.
    pub settings: Vec<Setting>,
    /// The fields of `Settings`, with their doc lines, 4 spaces in.
    pub fields: &'static str,
    /// The fields of `Settings::default()`, 12 spaces in.
    pub defaults: &'static str,
    /// A `configure` method, 4 spaces in, or empty.
    pub configure: &'static str,
    /// Statements in `before_start`, 8 spaces in, that end with
    /// `let cache: Arc<dyn brocanth_cache::Cache> = ...;`.
    pub open: &'static str,
    /// The readiness check, 8 spaces in, over `cache`, or empty.
    pub ready: &'static str,
    /// Whether the plugin runs the sweep, from a `sweep_every` setting.
    pub sweep: bool,
}

/// Writes the cache plugin over `backend`: the file, its `pub mod`, the
/// settings and the registration.
pub fn write_plugin(s: &mut Scaffold, project: &Project, backend: Backend) {
    let (derive, note) = settings_derive(project, "cache");
    let mut items = vec!["CacheHandle"];
    items.extend(backend.cache_items);
    if backend.sweep {
        items.push("Sweeper");
    }
    items.sort_unstable();
    let pick = |text: &'static str| if backend.sweep { text } else { "" };
    // A backend that neither reads a service nor registers a check leaves
    // the lifecycle alone, and an unused name would fail clippy.
    let uses_lc = backend.open.contains("lc.") || backend.ready.contains("lc.");
    let file = PLUGIN_RS
        .replace("@DOC@", backend.doc)
        .replace("@SHUTDOWN_USE@", pick(SHUTDOWN_USE))
        .replace("@CACHE_ITEMS@", &use_list(&items))
        .replace("@USES@", backend.uses)
        .replace("@NOTE@", &note)
        .replace("@DERIVE@", &derive)
        .replace("@FIELDS@", backend.fields)
        .replace("@DEFAULTS@", &defaults(backend.defaults))
        .replace("@SWEEPER_FIELD@", pick(SWEEPER_FIELD))
        .replace("@SWEEPER_NONE@", pick(SWEEPER_NONE))
        .replace("@CONFIGURE@", backend.configure)
        .replace("@LC@", if uses_lc { "lc" } else { "_lc" })
        .replace("@OPEN@", backend.open)
        .replace("@READY@", backend.ready)
        .replace("@SWEEP@", pick(SWEEP));
    s.file("src/plugins/cache/mod.rs", file);
    s.at(Slot::PluginsMod).line("pub mod cache;");
    configurable(s, project, "cache", "Cache", backend.settings);
    if !project.has("config") {
        // The literal form calls `Plugin::new`, which is a trait method, so
        // the trait has to be in scope. Every plugin module needs the same
        // line, and `use` twice does not compile.
        s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
    }
}

/// The names after `use brocanth_cache::`: a single one without braces,
/// as rustfmt writes it.
fn use_list(items: &[&str]) -> String {
    match items {
        [one] => (*one).to_string(),
        _ => format!("{{{}}}", items.join(", ")),
    }
}

/// The body of `Settings::default()`. rustfmt puts a struct literal on one
/// line when its fields fit in `struct_lit_width`, 18 columns by default.
fn defaults(fields: &str) -> String {
    match fields
        .lines()
        .map(|l| l.trim().trim_end_matches(','))
        .collect::<Vec<_>>()[..]
    {
        [one] if one.len() <= 18 => format!("        Self {{ {one} }}\n"),
        _ => format!("        Self {{\n{fields}        }}\n"),
    }
}

/// The `url` setting of a backend that connects to a server. A secret: a
/// URL can carry a password, so no flag sets it.
pub fn url_setting(default: &str, doc: &'static str) -> Setting {
    Setting::text("url", default, doc)
        .note("A URL can carry a password, so no flag sets it.")
        .secret()
}

/// A `configure` that refuses an empty `url`, for a backend with one.
pub const CONFIGURE_URL: &str = r#"
    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if self.settings.url.is_empty() {
            return Err(PluginError::new(
                "[cache] url is required; there is no default cache\n\
                 help: set `url` under [cache] in the config file, or the variable for `cache.url`",
            ));
        }
        Ok(())
    }
"#;

fn migrate_setting() -> Setting {
    Setting::bool(
        "migrate",
        true,
        "Create the unlogged cache table at startup if it is absent.",
    )
    .note(
        "Development only: see `PgCache::schema_sql` for the statements to put in a\n\
         migration instead.",
    )
}

fn sweep_setting() -> Setting {
    Setting::integer(
        "sweep_every",
        300,
        "Seconds between two sweeps of expired entries; 0 turns the sweep off.",
    )
    .note(
        "Each wait moves by up to a quarter, so instances do not sweep together.\n\
         Expired entries are never served; the sweep only frees their space.",
    )
}

/// Postgres, on a pool of its own, from `[cache] url`.
fn postgres() -> Backend {
    Backend {
        doc: PG_DOC,
        cache_items: &["PgCache"],
        uses: "",
        settings: vec![
            url_setting(
                "postgres://localhost/development",
                "The Postgres URL the cache lives in.",
            ),
            migrate_setting(),
            sweep_setting(),
        ],
        fields: concat!(
            "    /// The Postgres URL the cache lives in. Postgres and nothing else, on\n",
            "    /// purpose: a cache for a SQLite deployment is a separate crate behind the\n",
            "    /// same `Cache` trait.\n",
            "    pub url: String,\n",
            "    /// Create the unlogged cache table at startup if it is absent.\n",
            "    pub migrate: bool,\n",
            "    /// Seconds between two sweeps of expired entries; 0 turns the sweep off.\n",
            "    pub sweep_every: u64,\n",
        ),
        defaults: concat!(
            "            url: \"postgres://localhost/development\".to_string(),\n",
            "            migrate: true,\n",
            "            sweep_every: 300,\n",
        ),
        configure: CONFIGURE_URL,
        open: PG_OPEN,
        ready: PG_READY,
        sweep: true,
    }
}

/// Postgres, on the pool the `db` plugin opened.
fn postgres_on_db() -> Backend {
    Backend {
        doc: PG_DB_DOC,
        cache_items: &["PgCache"],
        uses: "",
        settings: vec![migrate_setting(), sweep_setting()],
        fields: concat!(
            "    /// Create the unlogged cache table at startup if it is absent.\n",
            "    pub migrate: bool,\n",
            "    /// Seconds between two sweeps of expired entries; 0 turns the sweep off.\n",
            "    pub sweep_every: u64,\n",
        ),
        defaults: concat!(
            "            migrate: true,\n",
            "            sweep_every: 300,\n",
        ),
        configure: "",
        open: PG_DB_OPEN,
        ready: PG_DB_READY,
        sweep: true,
    }
}

/// The process's memory.
fn memory() -> Backend {
    Backend {
        doc: MEMORY_DOC,
        cache_items: &["MemoryCache"],
        uses: "",
        settings: vec![sweep_setting()],
        fields: concat!(
            "    /// Seconds between two sweeps of expired entries; 0 turns the sweep off.\n",
            "    pub sweep_every: u64,\n",
        ),
        defaults: "            sweep_every: 300,\n",
        configure: "",
        open: MEMORY_OPEN,
        ready: "",
        sweep: true,
    }
}

const PG_DOC: &str = "\
//! The cache is a table, and the table is `UNLOGGED`: Postgres writes no
//! write-ahead log for it, ships it to no replica, and truncates it on crash
//! recovery. For any other table that is a catastrophe. For a cache it is the
//! only state you can reason about, and it is what makes a cache in a
//! relational database defensible at all. A deployment that already runs
//! Postgres therefore gets a cache without running Redis.
";

const PG_DB_DOC: &str = "\
//! The cache is a table, and the table is `UNLOGGED`: Postgres writes no
//! write-ahead log for it, ships it to no replica, and truncates it on crash
//! recovery. For any other table that is a catastrophe. For a cache it is the
//! only state you can reason about, and it is what makes a cache in a
//! relational database defensible at all. A deployment that already runs
//! Postgres therefore gets a cache without running Redis.
//!
//! It lives in the database the `db` plugin opens, on the same pool: one set
//! of connections to size, and a cache read is visible as a connection a
//! request is not using for its real query.
";

const MEMORY_DOC: &str = "\
//! The cache is a map in this process's memory, for tests and development.
//! Each process has its own, an invalidation reaches only the process that
//! made it, and a restart empties it. `brocanth check --deploy` reports
//! `cache-memory` as an error for that reason: before a deploy, run
//! `brocanth remove cache-memory`, and the cache goes back to Postgres.
";

const PG_OPEN: &str = r#"        // The lifecycle hooks are synchronous and opening a pool is not, so
        // the pool opens lazily and nothing here waits. Waiting would hang a
        // current-thread runtime, where this thread drives the I/O. The pool
        // spawns its upkeep task, so it needs the runtime entered.
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
            PluginError::new(format!(
                "[cache] needs a Tokio runtime at startup to open its pool ({e})"
            ))
        })?;
        let cache = {
            let _entered = runtime.enter();
            PgCache::connect_lazy(&self.settings.url)
                .map_err(|e| PluginError::new(format!("[cache] {e}")))?
        };
        let cache: Arc<dyn brocanth_cache::Cache> = Arc::new(cache);
"#;

const PG_DB_OPEN: &str = r#"        // The pool the `db` plugin opened in its own `before_start`, which
        // runs first because that plugin is registered first.
        let db = lc.services().get::<brocanth_db::Db>().ok_or_else(|| {
            PluginError::new(
                "[cache] found no database pool\n\
                 help: register the `db` plugin before the cache in src/plugins/mod.rs",
            )
        })?;
        let cache = PgCache::from_pool(db.pool().clone());
        let cache: Arc<dyn brocanth_cache::Cache> = Arc::new(cache);
"#;

const MEMORY_OPEN: &str = r#"        let cache: Arc<dyn brocanth_cache::Cache> = Arc::new(MemoryCache::new());
"#;

/// The Postgres readiness check, with the help line for `[cache] url`.
const PG_READY: &str = r#"        // Ready when one read of the table answers, as `CachePlugin` does. A
        // miss is a pass: the question is whether the store answers. With
        // `migrate`, the first check creates the table, as the jobs module
        // does, so a new project starts; a project with reviewers turns it
        // off and keeps `PgCache::schema_sql` in a migration. `serve` awaits
        // this once before it listens, so an unreachable store stops the
        // start there.
        let probe = Arc::clone(&cache);
        let migrate = self.settings.migrate;
        let migrated = Arc::new(tokio::sync::OnceCell::new());
        lc.ready_when(move || {
            let probe = Arc::clone(&probe);
            let migrated = Arc::clone(&migrated);
            async move {
                if migrate {
                    migrated
                        .get_or_try_init(|| probe.migrate())
                        .await
                        .map_err(|e| format!("creating the cache table: {e}"))?;
                }
                let key = brocanth_cache::Namespace::new("brocanth.readyz")
                    .and_then(|ns| brocanth_cache::Key::new(ns, "probe"))
                    .map_err(|e| e.to_string())?;
                probe.get(&key).await.map(drop).map_err(|e| {
                    format!(
                        "{e}\nhelp: check `[cache] url`; with `migrate = false`, the \
                         `brocanth_cache` table must exist: `PgCache::schema_sql()` has the \
                         statements for a migration"
                    )
                })
            }
        });
"#;

/// The Postgres readiness check on the `db` pool, with the help line for
/// `[db] url`.
const PG_DB_READY: &str = r#"        // Ready when one read of the table answers, as `CachePlugin` does. A
        // miss is a pass: the question is whether the store answers. With
        // `migrate`, the first check creates the table, as the jobs module
        // does, so a new project starts; a project with reviewers turns it
        // off and keeps `PgCache::schema_sql` in a migration. `serve` awaits
        // this once before it listens, so an unreachable store stops the
        // start there.
        let probe = Arc::clone(&cache);
        let migrate = self.settings.migrate;
        let migrated = Arc::new(tokio::sync::OnceCell::new());
        lc.ready_when(move || {
            let probe = Arc::clone(&probe);
            let migrated = Arc::clone(&migrated);
            async move {
                if migrate {
                    migrated
                        .get_or_try_init(|| probe.migrate())
                        .await
                        .map_err(|e| format!("creating the cache table: {e}"))?;
                }
                let key = brocanth_cache::Namespace::new("brocanth.readyz")
                    .and_then(|ns| brocanth_cache::Key::new(ns, "probe"))
                    .map_err(|e| e.to_string())?;
                probe.get(&key).await.map(drop).map_err(|e| {
                    format!(
                        "{e}\nhelp: check `[db] url`; with `migrate = false`, the \
                         `brocanth_cache` table must exist: `PgCache::schema_sql()` has the \
                         statements for a migration"
                    )
                })
            }
        });
"#;

const SHUTDOWN_USE: &str = "use brocanth::plugin_shutdown::ShutdownCx;\n";

const SWEEPER_FIELD: &str =
    "    /// The sweep, from `start` to `shutdown`.\n    sweeper: Option<Sweeper>,\n";

const SWEEPER_NONE: &str = "            sweeper: None,\n";

const SWEEP: &str = r#"
    fn start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // Nothing else deletes expired entries. Every read checks expiry
        // already, so this only frees their space. Each instance runs it,
        // and the deletes do not mind.
        let every = self.settings.sweep_every;
        let Some(handle) = self.handle.as_ref().filter(|_| every > 0) else {
            return Ok(());
        };
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
            PluginError::new(format!("[cache] needs a Tokio runtime to sweep ({e})"))
        })?;
        let on_error = |e: &brocanth_cache::CacheError| {
            crate::plugins::say!(warn, "[cache] sweep: {e}"; error = %e);
        };
        self.sweeper = Some(Sweeper::spawn(
            &runtime,
            Arc::clone(handle.inner()),
            std::time::Duration::from_secs(every),
            Arc::new(on_error),
        ));
        Ok(())
    }

    fn shutdown(&mut self, _sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        if let Some(sweeper) = self.sweeper.take() {
            sweeper.stop();
        }
        Ok(())
    }
"#;

/// `src/plugins/cache/mod.rs`, with the backend's parts as `@NAME@`.
const PLUGIN_RS: &str = r#"//! The cache, as this application registers it.
//!
@DOC@//!
//! `brocanth-cache` is in `Cargo.toml` because this file is here, and through
//! no other route. Delete the registration in `src/plugins/mod.rs` and delete
//! this file, and the application caches nothing and links nothing to do it
//! with.
//!
//! The crate also ships `CachePlugin`, and this is not that. `CachePlugin` is
//! handed a cache that is already open, because a pool is made asynchronously
//! and `Plugin::new` is not async. An application whose cache is one setting
//! wants the shorter road, so this plugin opens the cache at startup and
//! publishes the same `CacheHandle`. Hand-built caches -- a second namespace
//! policy, a backend of your own -- are what `CachePlugin` is for, and
//! swapping to it is a change in this file alone.

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ProvidesContext};
@SHUTDOWN_USE@use brocanth_cache::@CACHE_ITEMS@;
@USES@
/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
@FIELDS@}

impl Default for Settings {
    fn default() -> Self {
@DEFAULTS@    }
}

pub struct Cache {
    settings: Settings,
    /// Open at `before_start` and not before, so the field is an `Option` for
    /// exactly the length of the build.
    handle: Option<CacheHandle>,
@SWEEPER_FIELD@}

impl Plugin for Cache {
    type Config = Settings;
    type Provides = ProvidesContext<CacheHandle>;
    const NAME: &'static str = "cache";

    fn new(settings: Settings) -> Self {
        Self {
            settings,
            handle: None,
@SWEEPER_NONE@        }
    }
@CONFIGURE@
    fn before_start(&mut self, @LC@: &mut Lifecycle<'_>) -> Result<(), PluginError> {
@OPEN@@READY@        let handle = CacheHandle::new(cache);
        // The first cache this process opens is the one the admin's cache
        // tab reads: the admin is built before any plugin starts.
        handle.install();
        self.handle = Some(handle);
        Ok(())
    }
@SWEEP@
    fn provide(&self, cx: &mut ContextBuilder) {
        // `before_start` runs before the server is handed the application, so
        // the handle is here by the time any request builds a context.
        if let Some(handle) = &self.handle {
            cx.insert(handle.clone());
        }
    }
}
"#;
