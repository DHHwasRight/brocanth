//! `cache` -- what `brocanth create` writes when somebody asks for a cache.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: the name `PgCache::connect`, the shape of
//! `CacheHandle`, the fact that the lifecycle hook is synchronous while a
//! pool is not. Those move when this crate moves, and a generator living in
//! another repository would learn about it from a bug report.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{plugin, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

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
        .asked_in("data", "A cache, in the database you already have")
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
        plugin(s, project, "cache", "Cache");
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("config") {
            s.at(Slot::ConfigSections)
                .block("\n[cache]\nurl = \"postgres://localhost/development\"\n".to_string());
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
//! the shorter road, so this plugin takes the URL, opens the pool at startup,
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
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            url: "postgres://localhost/development".to_string(),
        }}
    }}
}}

pub struct Cache {{
    url: String,
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
        // The lifecycle hooks are synchronous and opening a pool is not. This
        // is the manoeuvre `CachePlugin` makes, for the same reason: a server
        // process has a runtime, `block_in_place` is wrong on a current-thread
        // one, so take the handle and block the builder on a scoped thread.
        // This is startup, where blocking is what everybody expects.
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {{
            PluginError::new(format!(
                "[cache] needs a Tokio runtime at startup to open its pool ({{e}})"
            ))
        }})?;
        let url = self.url.clone();
        let cache = std::thread::scope(|scope| {{
            scope
                .spawn(|| runtime.block_on(async move {{ PgCache::connect(&url).await }}))
                .join()
                .map_err(|_| PluginError::new("the cache connection thread panicked"))?
                .map_err(|e| PluginError::new(format!("[cache] connecting: {{e}}")))
        }})?;
        // The table is not created here. `brocanth-cache` can create it, and
        // it is off there for the reason it is absent here: the generator
        // proposes and the developer commits. `PgCache::schema_sql` hands out
        // the statements for a migration you can read.
        let cache: Arc<dyn brocanth_cache::Cache> = Arc::new(cache);
        // Ready when one read of the table answers, as `CachePlugin` does. A
        // miss is a pass: the question is whether the store answers.
        let probe = Arc::clone(&cache);
        lc.ready_when(move || {{
            let probe = Arc::clone(&probe);
            async move {{
                let key = brocanth_cache::Namespace::new("brocanth.readyz")
                    .and_then(|ns| brocanth_cache::Key::new(ns, "probe"))
                    .map_err(|e| e.to_string())?;
                probe.get(&key).await.map(drop).map_err(|e| e.to_string())
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
