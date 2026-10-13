//! The cache as a brocanth plugin.
//!
//! Three rules from PLAN.md §8 shape this file and none of them is negotiable.
//!
//! **The arrow points from the plugin to the core.** `brocanth-cache` depends
//! on `brocanth`; `brocanth` does not know this crate exists and never will.
//! An application that does not write [`CachePlugin`] in its `main` links no
//! `sqlx` and no cache.
//!
//! **A plugin arrives already configured.** [`CacheConfig`] holds a live
//! cache, because a cache is a connection pool and a connection pool is made
//! asynchronously, which a sync `Plugin::new` cannot do. So the application
//! opens it in its own `main`, where the runtime is, and hands it over. There
//! is no `plugin_from_config` path for the same reason `brocanth-jobs` has
//! none: nothing in a TOML file can become a pool.
//!
//! **Plugins do not autowire.** This one offers no middleware, mounts nothing,
//! starts nothing and runs no background task. The only thing it can be asked
//! to do beyond publishing the handle is create the table, and that is off by
//! default.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use brocanth_cache::{Cache, CacheConfig, CachePlugin, PgCache};
//! use brocanth::plugin_api::Plugin;
//! use brocanth::plugin_app::AppBuilder;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let cache = PgCache::connect("postgres://localhost/app").await?;
//! cache.migrate().await?;
//!
//! let app = AppBuilder::new()
//!     .plugin(CachePlugin::new(CacheConfig::new(Arc::new(cache))))
//!     .build()?;
//! # let _ = app;
//! # Ok(())
//! # }
//! ```
//!
//! A handler then reaches the cache through the context, and a handler in an
//! application that never registered this plugin does not compile:
//! `cx.get::<CacheHandle, _>()`.

use std::ops::Deref;
use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError, Services};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};

use crate::cache::Cache;

/// The cache, as it appears in the per-request context and in [`Services`].
///
/// A newtype around `Arc<dyn Cache>` rather than the `Arc` itself, so the
/// context lookup names something readable and a second cache — a different
/// namespace policy, a different database — can be a second type later rather
/// than a collision now.
///
/// It dereferences to `dyn Cache`, so every [`Cache`] method and every
/// [`CacheExt`](crate::CacheExt) method is called straight on it.
#[derive(Clone)]
pub struct CacheHandle(Arc<dyn Cache>);

impl CacheHandle {
    /// Wraps a cache.
    pub fn new(cache: Arc<dyn Cache>) -> Self {
        Self(cache)
    }

    /// The cache inside.
    pub fn inner(&self) -> &Arc<dyn Cache> {
        &self.0
    }
}

impl Deref for CacheHandle {
    type Target = dyn Cache;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl std::fmt::Debug for CacheHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheHandle")
            .field("max_value_bytes", &self.0.max_value_bytes())
            .finish_non_exhaustive()
    }
}

impl ContextItem for CacheHandle {}

/// What [`CachePlugin`] is built from.
pub struct CacheConfig {
    cache: Arc<dyn Cache>,
    migrate: bool,
}

impl CacheConfig {
    /// A plugin that publishes this cache and does nothing else.
    pub fn new(cache: Arc<dyn Cache>) -> Self {
        Self {
            cache,
            migrate: false,
        }
    }

    /// Create the cache's table during `before_start` if it is absent.
    ///
    /// Off by default, and it should stay off anywhere a human reviews
    /// migrations: §4.3 of the plan says the generator proposes and the
    /// developer commits, and
    /// [`PgCache::schema_sql`](crate::PgCache::schema_sql) hands out the
    /// statements for that. This exists for a test harness and for a
    /// deployment whose cache table is genuinely disposable — which, being
    /// `UNLOGGED`, it is.
    #[must_use]
    pub fn migrating(mut self) -> Self {
        self.migrate = true;
        self
    }
}

/// The cache, as a plugin.
pub struct CachePlugin {
    cache: Arc<dyn Cache>,
    migrate: bool,
}

impl CachePlugin {
    /// The cache this plugin installed, for the application's own use.
    pub fn cache(&self) -> &Arc<dyn Cache> {
        &self.cache
    }

    /// The cache an earlier plugin published, if any.
    ///
    /// `register` puts a [`CacheHandle`] in [`Services`], so a plugin that
    /// runs later — a rate limiter, a view cache, a session store — finds it
    /// here instead of being handed one.
    pub fn cache_of(services: &Services) -> Option<&CacheHandle> {
        services.get::<CacheHandle>()
    }
}

impl Plugin for CachePlugin {
    type Config = CacheConfig;
    /// The cache goes in the per-request context, so a handler can read it.
    type Provides = ProvidesContext<CacheHandle>;
    const NAME: &'static str = "cache";

    fn new(config: CacheConfig) -> Self {
        Self {
            cache: config.cache,
            migrate: config.migrate,
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        lc.services_mut()
            .insert(CacheHandle::new(self.cache.clone()));
        Ok(())
    }

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if !self.migrate {
            return Ok(());
        }
        // The lifecycle hooks are synchronous and `migrate` is not. This is the
        // same manoeuvre `brocanth-jobs` makes, for the same reason: in a
        // server process there is a runtime, `block_in_place` would be wrong on
        // a current-thread one, so take the handle and block the builder on a
        // scoped thread. That is startup, where blocking is what everybody
        // expects.
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {
            PluginError::new(format!(
                "brocanth-cache needs a Tokio runtime at startup to create its \
                 table ({e}); build the application from inside `#[tokio::main]`, \
                 or call `migrate` yourself and drop `migrating()`"
            ))
        })?;
        let cache = self.cache.clone();
        std::thread::scope(|scope| {
            scope
                .spawn(|| runtime.block_on(async move { cache.migrate().await }))
                .join()
                .map_err(|_| PluginError::new("the cache migration thread panicked"))?
                .map_err(|e| PluginError::new(format!("creating the cache table: {e}")))
        })
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(CacheHandle::new(self.cache.clone()));
    }
}
