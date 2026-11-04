//! `cache` -- what `brocanth create` writes when somebody asks for a cache.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: the name `PgCache::connect_lazy`, the shape of
//! `CacheHandle`, the fact that the lifecycle hook is synchronous while a
//! pool is not. Those move when this crate moves, and a generator living in
//! another repository would learn about it from a bug report.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{configurable, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// The cache, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct Cache;

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
        .asked_in("data", "A cache, in the database you already have")
        .recommends(
            "observability-health",
            "its readiness check is read only by /readyz; without it a deploy cannot tell that the cache table is unreachable",
        )
        .krate(Dep::brocanth("brocanth-cache"))
        // Only for the order of the `[cache]` section and the builder line:
        // the cache reads its URL from the file the config plugin loaded, so
        // it belongs under it rather than above it.
        .after("config")
        .absent(
            "brocanth-cache",
            "no cache; every read does the work a hit would have skipped",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/cache/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod cache;");
        configurable(
            s,
            project,
            "cache",
            "Cache",
            vec![
                Setting::text(
                    "url",
                    "postgres://localhost/development",
                    "The Postgres URL the cache lives in.",
                )
                .note("A URL can carry a password, so no flag sets it.")
                .secret(),
                Setting::bool(
                    "migrate",
                    true,
                    "Create the unlogged cache table at startup if it is absent.",
                )
                .note(
                    "Development only: see `PgCache::schema_sql` for the statements to put in a\n\
                     migration instead.",
                ),
            ],
        );
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
    }
}

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "cache");
    format!(
        r##"//! The cache, as this application registers it.
//!
//! The cache is a table, and the table is `UNLOGGED`: Postgres writes no
//! write-ahead log for it, ships it to no replica, and truncates it on crash
//! recovery. For any other table that is a catastrophe. For a cache it is the
//! only state you can reason about, and it is what makes a cache in a
//! relational database defensible at all. A deployment that already runs
//! Postgres therefore gets a cache without running Redis.
//!
//! `brocanth-cache` is in `Cargo.toml` because this file is here, and through
//! no other route. Delete the registration in `src/app.rs` and delete this
//! file, and the application caches nothing and links nothing to do it with.
//!
//! The crate also ships `CachePlugin`, and this is not that. `CachePlugin` is
//! handed a cache that is already open, because a pool is made asynchronously
//! and `Plugin::new` is not async. An application whose cache is one URL wants
//! the shorter road, so this plugin takes the URL, builds the pool at startup,
//! and publishes the same `CacheHandle`. Hand-built caches -- a shared pool, a
//! second namespace policy, a different backend -- are what `CachePlugin` is
//! for, and swapping to it is a change in this file alone.

use std::sync::Arc;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ProvidesContext}};
use brocanth_cache::{{CacheHandle, PgCache}};

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// The Postgres URL the cache lives in. Postgres and nothing else, on
    /// purpose: a cache for a SQLite deployment is a separate crate behind the
    /// same `Cache` trait.
    pub url: String,
    /// Create the unlogged cache table at startup if it is absent.
    pub migrate: bool,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            url: "postgres://localhost/development".to_string(),
            migrate: true,
        }}
    }}
}}

pub struct Cache {{
    url: String,
    migrate: bool,
    /// Open at `before_start` and not before, so the field is an `Option` for
    /// exactly the length of the build.
    handle: Option<CacheHandle>,
}}

impl Plugin for Cache {{
    type Config = Settings;
    type Provides = ProvidesContext<CacheHandle>;
    const NAME: &'static str = "cache";

    fn new(config: Settings) -> Self {{
        Self {{
            url: config.url,
            migrate: config.migrate,
            handle: None,
        }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.url.is_empty() {{
            return Err(PluginError::new(
                "[cache] url is required; there is no default cache",
            ));
        }}
        Ok(())
    }}

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // The lifecycle hooks are synchronous and opening a pool is not, so
        // the pool opens lazily and nothing here waits. Waiting would hang a
        // current-thread runtime, where this thread drives the I/O. The pool
        // spawns its upkeep task, so it needs the runtime entered.
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {{
            PluginError::new(format!(
                "[cache] needs a Tokio runtime at startup to open its pool ({{e}})"
            ))
        }})?;
        let cache = {{
            let _entered = runtime.enter();
            PgCache::connect_lazy(&self.url)
                .map_err(|e| PluginError::new(format!("[cache] {{e}}")))?
        }};
        let cache: Arc<dyn brocanth_cache::Cache> = Arc::new(cache);
        // Ready when one read of the table answers, as `CachePlugin` does. A
        // miss is a pass: the question is whether the store answers. With
        // `migrate`, the first check creates the table, as the jobs module
        // does, so a new project starts; a project with reviewers turns it
        // off and keeps `PgCache::schema_sql` in a migration. `serve` awaits
        // this once before it listens, so an unreachable store stops the
        // start there.
        let probe = Arc::clone(&cache);
        let migrate = self.migrate;
        let migrated = Arc::new(tokio::sync::OnceCell::new());
        lc.ready_when(move || {{
            let probe = Arc::clone(&probe);
            let migrated = Arc::clone(&migrated);
            async move {{
                if migrate {{
                    migrated
                        .get_or_try_init(|| probe.migrate())
                        .await
                        .map_err(|e| format!("creating the cache table: {{e}}"))?;
                }}
                let key = brocanth_cache::Namespace::new("brocanth.readyz")
                    .and_then(|ns| brocanth_cache::Key::new(ns, "probe"))
                    .map_err(|e| e.to_string())?;
                probe.get(&key).await.map(drop).map_err(|e| {{
                    format!(
                        "{{e}}\nhelp: check `[cache] url`; with `migrate = false`, the \
                         `brocanth_cache` table must exist: `PgCache::schema_sql()` has the \
                         statements for a migration"
                    )
                }})
            }}
        }});
        self.handle = Some(CacheHandle::new(cache));
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `before_start` runs before the server is handed the application, so
        // the handle is here by the time any request builds a context.
        if let Some(handle) = &self.handle {{
            cx.insert(handle.clone());
        }}
    }}
}}
"##
    )
}
