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
use std::sync::atomic::{AtomicBool, Ordering};
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

    /// Create the cache's table if it is absent, in the readiness check, the
    /// first time it runs. The server awaits every check once before it
    /// listens ([`Readiness::check_all`]), so the table is there before the
    /// first request.
    ///
    /// [`Readiness::check_all`]: brocanth::plugin_health::Readiness::check_all
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
        // Ready when one read of the table succeeds. A miss is a success: the
        // question is whether the store answers, not what it holds. With
        // `migrating`, the check creates the table first, until that worked
        // once: the server awaits every check before it listens, so the
        // table is there before the first request. No stage waits for it,
        // because a stage that waits hangs a current-thread runtime.
        let cache = self.cache.clone();
        let migrated = Arc::new(AtomicBool::new(!self.migrate));
        lc.ready_when(move || {
            let cache = cache.clone();
            let migrated = Arc::clone(&migrated);
            async move {
                if !migrated.load(Ordering::Acquire) {
                    cache
                        .migrate()
                        .await
                        .map_err(|e| format!("creating the cache table: {e}"))?;
                    migrated.store(true, Ordering::Release);
                }
                let key = crate::key::Namespace::new("brocanth.readyz")
                    .and_then(|ns| crate::key::Key::new(ns, "probe"))
                    .map_err(|e| e.to_string())?;
                cache.get(&key).await.map(drop).map_err(|e| e.to_string())
            }
        });
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(CacheHandle::new(self.cache.clone()));
    }
}
