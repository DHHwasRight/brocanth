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
/// gives a `PgStore`. Without one, an enabled plugin with the `postgres`
/// feature refuses to start; without the feature, the views are kept in
/// memory and lost at restart. [`Analytics::with_store`] picks it in code
/// instead.
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
            self.attach_store(lc)?;
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
        let flusher = self.flusher.take();
        if let Some(flusher) = &flusher {
            flusher.abort();
        }
        if flusher.is_none() && self.recorder.pending() == 0 {
            return Ok(());
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Ok(());
        };
        let recorder = self.recorder.clone();
        let remaining = sd.remaining();
        let last = async move {
            // The aborted timer may have been in a flush. `Recorder::flush`
            // is cancel-safe: once the task has ended, its batch is back on
            // the buffer.
            if let Some(flusher) = flusher {
                let _ = flusher.await;
            }
            if recorder.pending() > 0 {
                let flush = recorder.flush();
                match remaining {
                    Some(left) => {
                        let _ = tokio::time::timeout(left, flush).await;
                    }
                    None => {
                        let _ = flush.await;
                    }
                }
            }
            // What a timeout or a failed write left behind ends with the
            // process: count it.
            recorder.drop_pending();
        };
        // The same two shapes as the `db` plugin's close: a task on a
        // current-thread runtime, whose only thread drives the I/O, and a
        // scoped thread on a multi-thread one. The task cannot be waited
        // for here, because this thread is the one that must run it: it
        // writes what it can while the runtime still runs.
        if runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {
            runtime.spawn(last);
            return Ok(());
        }
        std::thread::scope(|scope| {
            scope.spawn(|| runtime.block_on(last)).join().map_err(|_| {
                PluginError::new(
                    "[analytics] the last flush panicked\n\
                     help: read the panic message above; it names the cause",
                )
            })
        })
    }
}

impl Analytics {
    /// With the `postgres` feature, a `brocanth_db::Db` in the services
    /// gives a `PgStore`. Without one, an enabled plugin refuses to start:
    /// views kept in memory would vanish at the next deploy, and nothing
    /// would say so. A disabled plugin counts nothing, so it takes the
    /// memory store. A test that wants memory says so with
    /// [`Analytics::with_store`].
    #[cfg(feature = "postgres")]
    fn attach_store(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let Some(db) = lc.services().get::<brocanth_db::Db>().cloned() else {
            if self.config.enabled {
                return Err(PluginError::new(
                    "[analytics] no database: the views would be kept in memory and lost at restart\n\
                     help: register the `db` plugin before `analytics`, or build the plugin with \
                     `Analytics::with_store` to choose a store in code",
                ));
            }
            self.recorder.attach(Arc::new(MemoryStore::new()));
            return Ok(());
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
        Ok(())
    }

    #[cfg(not(feature = "postgres"))]
    fn attach_store(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.recorder.attach(Arc::new(MemoryStore::new()));
        Ok(())
    }
}

impl HookSource for Analytics {
    type Families = OnRequest;
    /// `count` exists only while `enabled` is true, and `collect` only
    /// while `collector_origins` names an origin too.
    const HOOKS: &'static [&'static str] = &["count", "collect"];

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
    fn after_dispatch(&self, request: &Request, response: &Response, state: &mut RequestState) {
        if let Some(pending) = self.page_view(request, response, state) {
            self.recorder.record(pending);
        }
    }
}

impl Count {
    /// The view this exchange was, or `None` when it was not one.
    fn page_view(
        &self,
        request: &Request,
        response: &Response,
        state: &RequestState,
    ) -> Option<Pending> {
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
        let html =
            brocanth::plugin_body::is_html(response.headers().get(http::header::CONTENT_TYPE));
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
            state,
            Page {
                site: self.recorder.site(),
                path: request.uri().path(),
                query: request.uri().query().unwrap_or(""),
                referer: header(request, "referer"),
                host,
            },
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

/// The visited page, as the hook or the collector read it.
pub(crate) struct Page<'a> {
    /// The site the row belongs to: the recorder's own, or the host of a
    /// static site that sent a beacon. It is also in the visitor hash.
    pub site: &'a str,
    /// The visited path, before `clean_path`.
    pub path: &'a str,
    /// The visited query, before `clean_query`.
    pub query: &'a str,
    /// Where the visitor came from.
    pub referer: &'a str,
    /// The visited host, so a referrer from the site itself is dropped.
    pub host: Option<&'a str>,
}

/// A page view from the parts of a request that are kept, after the rules
/// the hook and the collector share: no bot, no signal when the site
/// respects it, no excluded path, and every field reduced. The address and
/// the user agent come from `request`, which the browser sent, and from
/// the `proxy-headers` hook's reading of it in `state`.
pub(crate) fn visit(
    config: &AnalyticsConfig,
    recorder: &Recorder,
    request: &Request,
    state: &RequestState,
    page: Page<'_>,
) -> Option<Pending> {
    let Page {
        site,
        path,
        query,
        referer,
        host,
    } = page;
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
        site: site.to_string(),
        day: recorder.today(),
        prehash: recorder.prehash(site, &address(config, request, state), ua),
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

/// The client address as text. Only ever hashed. In order: the address
/// the `proxy-headers` hook read from a trusted proxy, the edge's header
/// when one is named, the socket's.
fn address(config: &AnalyticsConfig, request: &Request, state: &RequestState) -> String {
    if let Some(client) = state
        .get::<brocanth_middleware::ClientInfo>()
        .and_then(|info| info.address)
    {
        return client.to_string();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trusted_proxys_client_address_comes_before_the_header_and_the_socket() {
        let config = AnalyticsConfig {
            ip_header: Some("fly-client-ip".to_string()),
            ..AnalyticsConfig::default()
        };
        let mut request = http::Request::builder()
            .header("fly-client-ip", "203.0.113.9")
            .body(Vec::new())
            .unwrap();
        request
            .extensions_mut()
            .insert(std::net::SocketAddr::from(([10, 0, 0, 1], 443)));
        let mut state = RequestState::new();
        assert_eq!(address(&config, &request, &state), "203.0.113.9");
        state.insert(brocanth_middleware::ClientInfo {
            address: Some([198, 51, 100, 4].into()),
            scheme: None,
            host: None,
            chain: Vec::new(),
        });
        assert_eq!(address(&config, &request, &state), "198.51.100.4");
        assert_eq!(
            address(&AnalyticsConfig::default(), &request, &RequestState::new()),
            "10.0.0.1",
            "behind no proxy, the socket"
        );
    }
}
