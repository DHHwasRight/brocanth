//! The cache as a brocanth plugin.
//!
//! Three rules from PLAN.md §8 shape this file and none of them is negotiable.
//!
//! **The arrow points from the plugin to the core.** `brocanth-cache` depends
//! on `brocanth`; `brocanth` does not know this crate exists and never will.
//! An application that does not write [`CachePlugin`] in its `main` links no
//! `sqlx` and no cache.
//!
//! **A plugin arrives already configured.** [`CacheConfig`] holds an open
//! cache: the application opens it in its own `main` and hands it over. To
//! open one from a URL in a synchronous place, use
//! [`PgCache::connect_lazy`](crate::PgCache::connect_lazy), which connects
//! on first use. The plugin `brocanth create` writes does that from
//! `[cache] url`, or reuses the `db` pool, at `before_start`. It registers
//! the same readiness check as this plugin, [`CacheReadiness`], and the same
//! [`Sweeper`].
//!
//! **Plugins do not autowire.** This one offers no middleware and mounts
//! nothing. It runs one background task, the sweep ([`Sweeper`]), from
//! `start` to `shutdown`, because without it nothing deletes expired rows:
//! see [`crate::sweep`] for why the plugin owns it.
//! [`CacheConfig::sweep_every`] sets the interval, and `Duration::ZERO` turns
//! it off. Creating the table is off by default.
//!
//! [`Sweeper`]: crate::sweep::Sweeper
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
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError, Services};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};
use brocanth::plugin_health::ReadinessCheck;
use brocanth::plugin_routes::BoxFuture;
use brocanth::plugin_shutdown::ShutdownCx;

use crate::cache::Cache;
use crate::sweep::{SweepErrorFn, Sweeper, DEFAULT_SWEEP_EVERY};

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

    /// Makes this the process's cache for a reader built before any plugin
    /// starts, and answers whether it was the first.
    ///
    /// The admin builds its tabs when the route table is built, and a
    /// plugin opens its cache later, at `before_start`. So the cache tab
    /// ([`CachePanel::installed`]) reads the cache here, when it renders.
    /// The first install wins: a process has one cache.
    ///
    /// [`CachePanel::installed`]: crate::admin::CachePanel::installed
    pub fn install(&self) -> bool {
        INSTALLED.set(self.clone()).is_ok()
    }

    /// The cache [`CacheHandle::install`] made the process's, if any.
    pub fn installed() -> Option<CacheHandle> {
        INSTALLED.get().cloned()
    }
}

/// The first cache installed in this process. See [`CacheHandle::install`].
static INSTALLED: OnceLock<CacheHandle> = OnceLock::new();

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

/// The cache's readiness check: ready when one read answers.
///
/// A miss is a pass: the question is whether the store answers, not what it
/// holds. With [`CacheReadiness::migrating`], the check creates the table
/// first, until that worked once. The server awaits every check before it
/// listens, so the table is there before the first request. No stage waits
/// for it, because a stage that waits hangs a current-thread runtime.
///
/// [`CachePlugin`] registers one. A plugin that opens its own cache
/// registers one too: `lc.ready_when(CacheReadiness::new(cache))`.
pub struct CacheReadiness {
    cache: Arc<dyn Cache>,
    migrated: AtomicBool,
    help: Option<String>,
}

impl CacheReadiness {
    /// A check that reads `cache` and creates nothing.
    pub fn new(cache: Arc<dyn Cache>) -> Self {
        Self {
            cache,
            migrated: AtomicBool::new(true),
            help: None,
        }
    }

    /// Creates the table first when `migrate` is true. See
    /// [`CacheConfig::migrating`] for when to turn it on.
    #[must_use]
    pub fn migrating(self, migrate: bool) -> Self {
        self.migrated.store(!migrate, Ordering::Release);
        self
    }

    /// The `help:` line a failed check ends with: what to check, in the
    /// application's own terms (`check [cache] url`).
    #[must_use]
    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    fn fail(&self, what: String) -> String {
        match &self.help {
            Some(help) => format!("{what}\nhelp: {help}"),
            None => what,
        }
    }
}

impl ReadinessCheck for CacheReadiness {
    fn check(&self) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            if !self.migrated.load(Ordering::Acquire) {
                self.cache
                    .migrate()
                    .await
                    .map_err(|e| self.fail(format!("creating the cache table: {e}")))?;
                self.migrated.store(true, Ordering::Release);
            }
            let key = crate::key::Namespace::new("brocanth.readyz")
                .and_then(|ns| crate::key::Key::new(ns, "probe"))
                .map_err(|e| e.to_string())?;
            self.cache
                .get(&key)
                .await
                .map(drop)
                .map_err(|e| self.fail(e.to_string()))
        })
    }
}

/// What [`CachePlugin`] is built from.
pub struct CacheConfig {
    cache: Arc<dyn Cache>,
    migrate: bool,
    ready_help: Option<String>,
    sweep_every: Duration,
    on_sweep_error: SweepErrorFn,
}

impl CacheConfig {
    /// A plugin that publishes this cache and sweeps it every
    /// [`DEFAULT_SWEEP_EVERY`].
    pub fn new(cache: Arc<dyn Cache>) -> Self {
        Self {
            cache,
            migrate: false,
            ready_help: None,
            sweep_every: DEFAULT_SWEEP_EVERY,
            on_sweep_error: Arc::new(|_| {}),
        }
    }

    /// How often the plugin deletes expired entries, about: each wait moves
    /// by up to a quarter, at random. `Duration::ZERO` starts no sweep, for
    /// an application that schedules [`sweep`](crate::sweep()) itself.
    #[must_use]
    pub fn sweep_every(mut self, every: Duration) -> Self {
        self.sweep_every = every;
        self
    }

    /// What to do with a sweep that failed. By default nothing: a failed
    /// sweep leaves expired rows, which no read returns, and the next sweep
    /// tries again. Give it the application's logger.
    #[must_use]
    pub fn on_sweep_error(
        mut self,
        f: impl Fn(&crate::CacheError) + Send + Sync + 'static,
    ) -> Self {
        self.on_sweep_error = Arc::new(f);
        self
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

    /// The `help:` line a failed readiness check ends with, such as
    /// "check `[cache] url`". See [`CacheReadiness::help`].
    #[must_use]
    pub fn ready_help(mut self, help: impl Into<String>) -> Self {
        self.ready_help = Some(help.into());
        self
    }
}

/// The cache, as a plugin.
pub struct CachePlugin {
    cache: Arc<dyn Cache>,
    migrate: bool,
    ready_help: Option<String>,
    sweep_every: Duration,
    on_sweep_error: SweepErrorFn,
    sweeper: Option<Sweeper>,
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
            ready_help: config.ready_help,
            sweep_every: config.sweep_every,
            on_sweep_error: config.on_sweep_error,
            sweeper: None,
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let handle = CacheHandle::new(self.cache.clone());
        handle.install();
        lc.services_mut().insert(handle);
        let mut ready = CacheReadiness::new(self.cache.clone()).migrating(self.migrate);
        if let Some(help) = &self.ready_help {
            ready = ready.help(help.clone());
        }
        lc.ready_when(ready);
        Ok(())
    }

    fn start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if self.sweep_every.is_zero() {
            return Ok(());
        }
        // No runtime: a test or a tool that starts the application without
        // serving it. Nothing sweeps, which costs disk and never correctness.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Ok(());
        };
        self.sweeper = Some(Sweeper::spawn(
            &runtime,
            Arc::clone(&self.cache),
            self.sweep_every,
            Arc::clone(&self.on_sweep_error),
        ));
        Ok(())
    }

    fn shutdown(&mut self, _sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        if let Some(sweeper) = self.sweeper.take() {
            sweeper.stop();
        }
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(CacheHandle::new(self.cache.clone()));
    }
}
