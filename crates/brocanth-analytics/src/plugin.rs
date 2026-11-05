//! [`Analytics`]: the plugin, its hook, and the task that writes.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState, Response,
};
use brocanth::plugin_order::Order;
use brocanth::plugin_shutdown::ShutdownCx;

use crate::classify::{clean_path, clean_query, country, is_bot, referrer_origin, ua_class};
use crate::config::AnalyticsConfig;
use crate::recorder::{Clock, Pending, Recorder};
use crate::store::{MemoryStore, Store};

/// The first recorder built in this process, for a panel the admin mounts
/// before it can be handed one. See [`Analytics::installed`].
static INSTALLED: OnceLock<Recorder> = OnceLock::new();

/// Counts page views of this application, first-party.
///
/// One request hook, after dispatch, so it sees the final status and
/// content type and never delays the answer: it hashes, pushes onto a
/// buffer and returns. A task writes the buffer every
/// [`flush_interval`](AnalyticsConfig::flush_interval) seconds.
///
/// The store is picked in `before_start`: a `brocanth_db::Db` in the
/// application's services (the `db` plugin, registered before this one)
/// gives a `PgStore`; without one the views are kept in memory and lost at
/// restart. [`Analytics::with_store`] picks it in code instead.
pub struct Analytics {
    config: AnalyticsConfig,
    recorder: Recorder,
    flusher: Option<tokio::task::JoinHandle<()>>,
}

impl Analytics {
    /// The plugin, writing to `store`, on the system clock.
    pub fn with_store(config: AnalyticsConfig, store: Arc<dyn Store>) -> Self {
        let plugin = Self::new(config);
        plugin.recorder.attach(store);
        plugin
    }

    /// The plugin on another clock, so a test can move the day.
    pub fn with_clock(
        config: AnalyticsConfig,
        store: Arc<dyn Store>,
        clock: impl Fn() -> SystemTime + Send + Sync + 'static,
    ) -> Self {
        let clock: Clock = Arc::new(clock);
        let recorder = Recorder::new(config.site.clone(), config.retention_days, clock);
        recorder.attach(store);
        let _ = INSTALLED.set(recorder.clone());
        Self {
            config,
            recorder,
            flusher: None,
        }
    }

    /// A handle on the buffer and the store: for a test that flushes by
    /// hand, and for [`AnalyticsPanel`](crate::admin::AnalyticsPanel).
    pub fn recorder(&self) -> Recorder {
        self.recorder.clone()
    }

    /// The recorder of the first `Analytics` built in this process.
    ///
    /// A generated admin mounts its routes before any plugin can hand it a
    /// value, so its panel asks here, when it renders. A process builds one
    /// application, so "the first" is "the one". A test that builds
    /// several hands the panel a recorder instead.
    pub fn installed() -> Option<Recorder> {
        INSTALLED.get().cloned()
    }
}

impl Plugin for Analytics {
    type Config = AnalyticsConfig;
    type Provides = NoContext;
    /// Also the configuration section.
    const NAME: &'static str = "analytics";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: AnalyticsConfig) -> Self {
        let clock: Clock = Arc::new(SystemTime::now);
        let recorder = Recorder::new(config.site.clone(), config.retention_days, clock);
        let _ = INSTALLED.set(recorder.clone());
        Self {
            config,
            recorder,
            flusher: None,
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if self.config.site.trim().is_empty() {
            return Err(PluginError::new(
                "[analytics] site is empty\n\
                 help: set `site` under [analytics] to a name for this application",
            ));
        }
        for header in [&self.config.ip_header, &self.config.country_header]
            .into_iter()
            .flatten()
        {
            if http::HeaderName::from_bytes(header.as_bytes()).is_err() {
                return Err(PluginError::new(format!(
                    "[analytics] `{header}` is not a header name\n\
                     help: use the lower-case name the edge sends, such as `fly-client-ip`"
                )));
            }
        }
        Ok(())
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if self.recorder.store().is_none() {
            self.attach_store(lc);
        }
        if !self.config.enabled {
            return Ok(());
        }
        // No runtime: a test or a tool that builds the application without
        // serving it. The buffer still fills, and `Recorder::flush` writes
        // it when somebody calls it.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Ok(());
        };
        let recorder = self.recorder.clone();
        let every = Duration::from_secs(self.config.flush_interval.max(1));
        self.flusher = Some(runtime.spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                // A failed write keeps its batch and records the error for
                // the panel. Nothing here can reach a request.
                let _ = recorder.flush().await;
            }
        }));
        Ok(())
    }

    fn shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        if let Some(flusher) = self.flusher.take() {
            flusher.abort();
        }
        if self.recorder.pending() == 0 {
            return Ok(());
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Ok(());
        };
        let recorder = self.recorder.clone();
        let remaining = sd.remaining();
        let last = async move {
            let flush = recorder.flush();
            match remaining {
                Some(left) => {
                    let _ = tokio::time::timeout(left, flush).await;
                }
                None => {
                    let _ = flush.await;
                }
            }
        };
        // The same two shapes as the `db` plugin's close: a task on a
        // current-thread runtime, whose only thread drives the I/O, and a
        // scoped thread on a multi-thread one.
        if runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {
            runtime.spawn(last);
            return Ok(());
        }
        std::thread::scope(|scope| {
            scope
                .spawn(|| runtime.block_on(last))
                .join()
                .map_err(|_| PluginError::new("[analytics] the last flush panicked"))
        })
    }
}

impl Analytics {
    #[cfg(feature = "postgres")]
    fn attach_store(&mut self, lc: &mut Lifecycle<'_>) {
        let Some(db) = lc.services().get::<brocanth_db::Db>().cloned() else {
            self.recorder.attach(Arc::new(MemoryStore::new()));
            return;
        };
        let store = Arc::new(crate::postgres::PgStore::new(db));
        self.recorder.attach(store.clone());
        if self.config.migrate {
            // The first readiness check creates the tables, once per
            // process, as the cache and jobs modules do.
            let migrated = Arc::new(tokio::sync::OnceCell::new());
            lc.ready_when(move || {
                let store = store.clone();
                let migrated = Arc::clone(&migrated);
                async move {
                    migrated
                        .get_or_try_init(|| store.migrate())
                        .await
                        .map(|_| ())
                        .map_err(|e| {
                            format!(
                                "[analytics] creating the tables: {e}\n\
                                 help: check `[db] url`; with `migrate = false`, run \
                                 `brocanth_analytics::PgStore::SCHEMA` in a migration"
                            )
                        })
                }
            });
        }
    }

    #[cfg(not(feature = "postgres"))]
    fn attach_store(&mut self, _lc: &mut Lifecycle<'_>) {
        self.recorder.attach(Arc::new(MemoryStore::new()));
    }
}

impl HookSource for Analytics {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        if !self.config.enabled {
            return Ok(());
        }
        registrar.request(
            "count",
            Order::new(),
            Count {
                config: self.config.clone(),
                recorder: self.recorder.clone(),
            },
        );
        if !self.config.collector_origins.is_empty() {
            // Before the CSRF check: a beacon is a cross-site POST with no
            // token, and it writes nothing a forged form could abuse. After
            // the proxy reader and the rate limiter, when present, so the
            // edge's address is read and a flood is refused first.
            registrar.request(
                "collect",
                Order::new()
                    .before_if_present("csrf.check")
                    .after_if_present("proxy-headers.read")
                    .after_if_present("rate_limit.count"),
                crate::collector::Collect::new(self.config.clone(), self.recorder.clone()),
            );
        }
        Ok(())
    }
}

/// The hook: decides whether a finished response was a page view, and if
/// so queues it.
struct Count {
    config: AnalyticsConfig,
    recorder: Recorder,
}

impl RequestHook for Count {
    fn after_dispatch(&self, request: &Request, response: &Response, _state: &mut RequestState) {
        if let Some(pending) = self.page_view(request, response) {
            self.recorder.record(pending);
        }
    }
}

impl Count {
    /// The view this exchange was, or `None` when it was not one.
    fn page_view(&self, request: &Request, response: &Response) -> Option<Pending> {
        // A GET, answered with a whole HTML page or a dp-swap partial. A
        // partial is a navigation the visitor made in place of a full load,
        // so it counts once, like the load would have.
        if request.method() != http::Method::GET {
            return None;
        }
        let status = response.status().as_u16();
        if !(200..300).contains(&status) || status == 204 || status == 206 {
            return None;
        }
        let html = response
            .headers()
            .get(http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/html"));
        if !html {
            return None;
        }
        // A prefetch is a guess the browser made, not a visit.
        let purpose = format!(
            "{} {} {}",
            header(request, "sec-purpose"),
            header(request, "purpose"),
            header(request, "x-moz")
        );
        if purpose.to_ascii_lowercase().contains("prefetch") {
            return None;
        }
        let host = request
            .headers()
            .get(http::header::HOST)
            .and_then(|v| v.to_str().ok())
            .or_else(|| request.uri().authority().map(|a| a.as_str()));
        visit(
            &self.config,
            &self.recorder,
            request,
            request.uri().path(),
            request.uri().query().unwrap_or(""),
            header(request, "referer"),
            host,
        )
    }
}

/// One header as text, or `""`.
pub(crate) fn header<'r>(request: &'r Request, name: &str) -> &'r str {
    request
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// A page view from the parts of a request that are kept, after the rules
/// the hook and the collector share: no bot, no signal when the site
/// respects it, no excluded path, and every field reduced.
///
/// `path` and `query` are the visited page's, `referer` is where the
/// visitor came from, and `host` is the visited site, so a referrer from
/// the site itself is dropped.
pub(crate) fn visit(
    config: &AnalyticsConfig,
    recorder: &Recorder,
    request: &Request,
    path: &str,
    query: &str,
    referer: &str,
    host: Option<&str>,
) -> Option<Pending> {
    let ua = header(request, "user-agent");
    if is_bot(ua) {
        return None;
    }
    if config.respect_gpc
        && (header(request, "sec-gpc").trim() == "1" || header(request, "dnt").trim() == "1")
    {
        return None;
    }
    let path = clean_path(path);
    if config.excludes(&path) {
        return None;
    }
    let class = ua_class(ua);
    Some(Pending {
        day: recorder.today(),
        prehash: recorder.prehash(&address(config, request), ua),
        path,
        query: clean_query(query, &config.query_allow),
        referrer: referrer_origin(referer, host).unwrap_or_default(),
        browser: class.browser,
        os: class.os,
        device: class.device,
        country: config
            .country_header
            .as_deref()
            .and_then(|name| country(header(request, name)))
            .unwrap_or_default(),
    })
}

/// The client address as text: the edge's header when one is named, else
/// the socket's. Only ever hashed.
fn address(config: &AnalyticsConfig, request: &Request) -> String {
    if let Some(name) = &config.ip_header {
        let first = header(request, name).split(',').next().unwrap_or("").trim();
        if !first.is_empty() {
            return first.to_string();
        }
    }
    request
        .extensions()
        .get::<std::net::SocketAddr>()
        .map(|peer| peer.ip().to_string())
        .unwrap_or_default()
}
