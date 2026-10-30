//! Day-2 observability for a brocanth application.
//!
//! Four things a deployed service needs and the core does not assume:
//!
//! | | What | Feature |
//! | --- | --- | --- |
//! | Logs | one line per request and per `tracing` event, JSON or text, on stdout, with a request id | `logs` (default) |
//! | Probes | `/up` (liveness) and `/readyz` (each plugin's readiness check, with latency) | always |
//! | Metrics | `/metrics` in Prometheus text: requests by route template and status class, in-flight, and what plugins add | `metrics` (default) |
//! | Traces | a span per request and per job run, exported over OTLP, W3C `traceparent` in and out | `otel` (off) |
//!
//! # Where it attaches
//!
//! At the one function [`brocanth_server::Server::run`] takes. [`Observer::wrap`]
//! returns a function of the same shape, so the observer sees every request:
//! the 404, the static file and the panic as well as the routed page. It
//! needs nothing from the router except, optionally, the route template for
//! a path ([`Observer::wrap_routed`]).
//!
//! ```no_run
//! use brocanth_observe::{ObserveConfig, Observer};
//! use brocanth_server::{Response, Server};
//!
//! # async fn f() -> Result<(), Box<dyn std::error::Error>> {
//! let observer = Observer::new(ObserveConfig::new("shop"))?;
//! let server = Server::bind("0.0.0.0:3000").await?;
//! tokio::spawn(observer.clone().stop_on_signal(server.stopper()));
//! server
//!     .run(observer.wrap(|request| async move {
//!         Response::text(200, format!("you asked for {}", request.path))
//!     }))
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! # And at the application
//!
//! [`ObservePlugin`] registers the observer with the application: it reads the
//! application's [`Readiness`] for `/readyz`, publishes the [`Observer`] in
//! [`Services`](brocanth::plugin_api::Services), installs the log subscriber
//! in `before_start`, and flushes the trace exporter in `shutdown`. With the
//! `jobs` feature it also finds the job queue and exports its depth.
//!
//! # Shutdown, in the order Kubernetes needs (ADR 2)
//!
//! [`Observer::stop_on_signal`] waits for SIGTERM or Ctrl-C, flips readiness
//! to failing, keeps serving for [`ObserveConfig::drain_delay`] so the
//! endpoints controller removes the pod, and only then stops the listener.
//! [`App::shutdown`](brocanth::plugin_app::App::shutdown) flips the same bit
//! again before `begin_shutdown`, so a stop that arrives another way is
//! still reported before anything drains.

#![warn(missing_docs)]

mod config;
mod probes;
mod text;

#[cfg(feature = "_jobs")]
mod jobs;
#[cfg(feature = "logs")]
pub mod logs;
#[cfg(feature = "metrics")]
pub mod metrics;
#[cfg(feature = "otel")]
mod otel;
#[cfg(feature = "scaffold")]
pub mod scaffold;

use std::future::Future;
use std::sync::{Arc, OnceLock};
#[cfg(feature = "logs")]
use std::time::Duration;
use std::time::Instant;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_health::Readiness;
use brocanth::plugin_routes::BoxFuture;
use brocanth::plugin_shutdown::ShutdownCx;
use brocanth_server::{Request, Response, ServeError, Server, Stopper};

#[cfg(feature = "otel")]
pub use config::TracingConfig;
pub use config::{LogFormat, ObserveConfig};
#[cfg(feature = "_jobs")]
pub use jobs::JobTelemetry;
#[cfg(feature = "metrics")]
pub use metrics::Metrics;
pub use probes::{CheckReport, ReadyReport};

/// The route label for a request no route matched.
pub const UNMATCHED: &str = "unmatched";

/// The request id, logs, probes, metrics and traces of one application.
///
/// Cheap to clone; every clone is the same observer. Build it before the
/// application, so the job queue can take [`Observer::job_events`] when it
/// is built, then register it with [`ObservePlugin`].
#[derive(Clone)]
pub struct Observer {
    inner: Arc<Inner>,
}

struct Inner {
    config: ObserveConfig,
    readiness: OnceLock<Readiness>,
    own_readiness: Readiness,
    #[cfg(feature = "metrics")]
    metrics: Metrics,
    #[cfg(feature = "_jobs")]
    jobs: JobTelemetry,
    #[cfg(feature = "metrics")]
    #[cfg_attr(not(feature = "_jobs"), allow(dead_code))]
    watching_queue: std::sync::atomic::AtomicBool,
    #[cfg(feature = "otel")]
    otel: Option<otel::Otel>,
}

impl std::fmt::Debug for Observer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Observer")
            .field("config", &self.inner.config)
            .finish_non_exhaustive()
    }
}

impl Observer {
    /// An observer for `config`.
    ///
    /// With the `otel` feature and `config.tracing.enabled`, this builds the
    /// OTLP exporter. It sends nothing until a span closes.
    ///
    /// # Errors
    ///
    /// The exporter cannot be built (a malformed endpoint), with a `help:`
    /// line. Without `otel` this does not fail.
    pub fn new(config: ObserveConfig) -> Result<Self, String> {
        #[cfg(feature = "otel")]
        let otel = if config.tracing.enabled {
            Some(otel::Otel::from_config(&config.service, &config.tracing)?)
        } else {
            None
        };
        Ok(Self::build(
            config,
            #[cfg(feature = "otel")]
            otel,
        ))
    }

    /// An observer that exports through a tracer provider built elsewhere:
    /// an in-memory exporter in a test, or an application's own processors.
    /// `config.tracing` is not read.
    #[cfg(feature = "otel")]
    pub fn with_tracer_provider(
        config: ObserveConfig,
        provider: opentelemetry_sdk::trace::SdkTracerProvider,
    ) -> Self {
        Self::build(config, Some(otel::Otel::with_provider(provider)))
    }

    fn build(config: ObserveConfig, #[cfg(feature = "otel")] otel: Option<otel::Otel>) -> Self {
        #[cfg(feature = "metrics")]
        let metrics = Metrics::new();
        #[cfg(feature = "_jobs")]
        let jobs = JobTelemetry::new();
        #[cfg(all(feature = "_jobs", feature = "metrics"))]
        jobs.register(&metrics);
        Self {
            inner: Arc::new(Inner {
                config,
                readiness: OnceLock::new(),
                own_readiness: Readiness::new(),
                #[cfg(feature = "metrics")]
                metrics,
                #[cfg(feature = "_jobs")]
                jobs,
                #[cfg(feature = "metrics")]
                watching_queue: std::sync::atomic::AtomicBool::new(false),
                #[cfg(feature = "otel")]
                otel,
            }),
        }
    }

    /// What it was built from.
    pub fn config(&self) -> &ObserveConfig {
        &self.inner.config
    }

    /// Reads `/readyz` from the application's readiness list.
    ///
    /// [`ObservePlugin`] calls it during `register`. The first call wins;
    /// before any call the observer has an empty list of its own, so `/readyz`
    /// answers ready until it is attached.
    pub fn attach(&self, readiness: Readiness) {
        let _ = self.inner.readiness.set(readiness);
    }

    /// The readiness list `/readyz` reads.
    pub fn readiness(&self) -> &Readiness {
        self.inner
            .readiness
            .get()
            .unwrap_or(&self.inner.own_readiness)
    }

    /// Runs every readiness check now, as `/readyz` does.
    pub async fn ready(&self) -> ReadyReport {
        probes::ready(self.readiness(), self.inner.config.check_timeout).await
    }

    /// The application's metrics registry, for a plugin's own series.
    #[cfg(feature = "metrics")]
    pub fn metrics(&self) -> &Metrics {
        &self.inner.metrics
    }

    /// The sink to hand `Queue::with_events`: a span, a log line and a
    /// counter per job run.
    #[cfg(feature = "_jobs")]
    pub fn job_events(&self) -> JobTelemetry {
        self.inner.jobs.clone()
    }

    /// Exports the queue's depth on `/metrics`: ready, retrying, scheduled
    /// and running per queue, the dead letter, and the age of the oldest
    /// runnable job. Read when Prometheus scrapes. A second call does
    /// nothing.
    ///
    /// [`ObservePlugin`] calls it when a `brocanth-jobs` queue is in the
    /// application's services.
    #[cfg(all(feature = "_jobs", feature = "metrics"))]
    pub fn watch_queue(&self, queue: brocanth_jobs::Queue) {
        use std::sync::atomic::Ordering;
        if !self.inner.watching_queue.swap(true, Ordering::SeqCst) {
            jobs::watch_queue(&self.inner.metrics, queue);
        }
    }

    // -- requests -----------------------------------------------------------

    /// `next`, observed. Pass the result to [`Server::run`].
    ///
    /// Every request is labelled [`UNMATCHED`] in the metrics. Use
    /// [`Observer::wrap_routed`] to label it by route template.
    pub fn wrap<F, Fut>(
        &self,
        next: F,
    ) -> impl Fn(Request) -> BoxFuture<'static, Response> + Send + Sync + 'static
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        self.wrap_routed(next, |_: &Request| None)
    }

    /// `next`, observed, with `route` naming the route template a request
    /// matches: `|r| router.at(&r.path).map(|m| m.template().to_string())`.
    pub fn wrap_routed<F, Fut, R>(
        &self,
        next: F,
        route: R,
    ) -> impl Fn(Request) -> BoxFuture<'static, Response> + Send + Sync + 'static
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
        R: Fn(&Request) -> Option<String> + Send + Sync + 'static,
    {
        let observer = self.clone();
        let next = Arc::new(next);
        let route = Arc::new(route);
        move |request: Request| {
            let observer = observer.clone();
            let next = Arc::clone(&next);
            let label = route(&request);
            Box::pin(async move { observer.handle(request, label, |r| next(r)).await })
        }
    }

    /// One request: the probes and `/metrics`, else the request id, the
    /// span, `next`, the metrics and the log line.
    ///
    /// `next` is called synchronously inside the request span, then its
    /// future is awaited inside it, so a handler that does its work before
    /// returning a future is inside the span too.
    pub async fn handle<F, Fut>(
        &self,
        mut request: Request,
        route: Option<String>,
        next: F,
    ) -> Response
    where
        F: FnOnce(Request) -> Fut,
        Fut: Future<Output = Response>,
    {
        if let Some(answer) = self.answer_own(&request, true).await {
            return answer;
        }

        let config = &self.inner.config;
        let header = config.request_id_header.as_str();
        let id = request
            .header(header)
            .and_then(text::accepted_id)
            .map_or_else(text::new_id, str::to_string);
        request.set_header(header, &id);

        let started = Instant::now();
        let method = request.verb.as_str();
        #[cfg(feature = "logs")]
        let path = request.path.clone();
        let route = route.unwrap_or_else(|| UNMATCHED.to_string());
        #[cfg(feature = "metrics")]
        let in_flight = self.inner.metrics.http.enter();

        #[cfg(feature = "logs")]
        let (response, span) = {
            use tracing::Instrument as _;
            let span = self.request_span(&request, &id, &route);
            let future = {
                let _entered = span.enter();
                next(request)
            };
            (future.instrument(span.clone()).await, span)
        };
        #[cfg(not(feature = "logs"))]
        let response = next(request).await;

        let mut response = response;
        if response.header(header).is_none() {
            response = response.with_header(header, &id);
        }
        #[cfg(feature = "otel")]
        if let Some(otel) = &self.inner.otel {
            response = otel.inject(&span, response);
        }

        let took = started.elapsed();
        #[cfg(feature = "metrics")]
        {
            self.inner
                .metrics
                .http
                .observe(method, &route, response.status, took);
            drop(in_flight);
        }
        #[cfg(feature = "logs")]
        log_request(&span, method, &path, response.status, took);
        #[cfg(not(any(feature = "logs", feature = "metrics")))]
        let _ = (method, took, route);
        response
    }

    /// The span a request runs in.
    #[cfg(feature = "logs")]
    fn request_span(&self, request: &Request, id: &str, route: &str) -> tracing::Span {
        let method = request.verb.as_str();
        let span = tracing::info_span!(
            target: "brocanth::request",
            "request",
            otel.name = %format!("{method} {route}"),
            otel.kind = "server",
            otel.status_code = tracing::field::Empty,
            request_id = %id,
            method,
            path = %request.path,
            route = %route,
            trace_id = tracing::field::Empty,
            span_id = tracing::field::Empty,
        );
        #[cfg(feature = "otel")]
        if let Some(otel) = &self.inner.otel {
            if let Some((trace_id, span_id)) = otel.adopt(&span, request) {
                span.record("trace_id", trace_id.as_str());
                span.record("span_id", span_id.as_str());
            }
        }
        span
    }

    /// The probes, and `/metrics` when `main` and it is not on its own
    /// listener. `None`: not ours, pass it on.
    async fn answer_own(&self, request: &Request, main: bool) -> Option<Response> {
        let config = &self.inner.config;
        let path = request.path.as_str();
        let started = Instant::now();
        let answer = if config.live_path.as_deref() == Some(path) {
            probes::live()
        } else if config.ready_path.as_deref() == Some(path) {
            self.ready().await.into_response()
        } else {
            self.answer_metrics(request, main).await?
        };
        if config.log_probes {
            #[cfg(feature = "logs")]
            {
                let span = tracing::info_span!(target: "brocanth::request", "probe", path);
                log_request(
                    &span,
                    request.verb.as_str(),
                    path,
                    answer.status,
                    started.elapsed(),
                );
            }
        }
        let _ = started;
        Some(answer)
    }

    #[cfg(feature = "metrics")]
    async fn answer_metrics(&self, request: &Request, main: bool) -> Option<Response> {
        let config = &self.inner.config;
        if config.metrics_path.as_deref() != Some(request.path.as_str()) {
            return None;
        }
        if main && config.metrics_bind.is_some() {
            return None;
        }
        if let Some(token) = &config.metrics_token {
            if !metrics::authorized(request.header("authorization"), token) {
                return Some(
                    Response::text(401, "Unauthorized")
                        .with_header("www-authenticate", "Bearer realm=\"metrics\""),
                );
            }
        }
        let body = self.inner.metrics.render(config.check_timeout).await;
        Some(
            Response::new(200, metrics::CONTENT_TYPE, body)
                .with_header("cache-control", "no-store"),
        )
    }

    #[cfg(not(feature = "metrics"))]
    async fn answer_metrics(&self, _request: &Request, _main: bool) -> Option<Response> {
        None
    }

    /// Serves the probes and `/metrics` on `server`, a second listener, until
    /// it stops. Everything else is a 404.
    ///
    /// For [`ObserveConfig::metrics_bind`]: Prometheus scrapes a port the
    /// load balancer does not expose.
    ///
    /// # Errors
    ///
    /// The listener failed.
    pub async fn serve_admin(&self, server: Server) -> Result<(), ServeError> {
        let observer = self.clone();
        server
            .run(move |request| {
                let observer = observer.clone();
                async move {
                    match observer.answer_own(&request, false).await {
                        Some(answer) => answer,
                        None => Response::text(404, "Not Found"),
                    }
                }
            })
            .await
    }

    // -- shutdown -----------------------------------------------------------

    /// Waits for SIGTERM or Ctrl-C, then stops `stopper`'s server in the
    /// order a load balancer needs: readiness fails first, the server keeps
    /// answering for [`ObserveConfig::drain_delay`], then it stops
    /// accepting.
    ///
    /// Spawn it beside [`Server::run`]. After `run` returns, call
    /// [`App::shutdown`](brocanth::plugin_app::App::shutdown).
    pub async fn stop_on_signal(self, stopper: Stopper) {
        terminated().await;
        self.drain_then_stop(stopper).await;
    }

    /// The second half of [`Observer::stop_on_signal`], for a stop that
    /// arrives some other way.
    pub async fn drain_then_stop(&self, stopper: Stopper) {
        self.readiness().drain();
        #[cfg(feature = "logs")]
        tracing::info!(
            target: "brocanth::shutdown",
            drain_delay_ms = self.inner.config.drain_delay.as_millis() as u64,
            "not ready; stopping after the drain delay"
        );
        if !self.inner.config.drain_delay.is_zero() {
            tokio::time::sleep(self.inner.config.drain_delay).await;
        }
        stopper.stop();
    }

    // -- the subscriber -----------------------------------------------------

    /// The `tracing` subscriber: the log layer writing to `sink` under the
    /// configured level, and the OpenTelemetry layer when tracing is on.
    ///
    /// # Errors
    ///
    /// The level (or `RUST_LOG`) does not parse, with a `help:` line.
    #[cfg(feature = "logs")]
    pub fn subscriber(
        &self,
        sink: impl logs::LogSink,
    ) -> Result<Box<dyn tracing::Subscriber + Send + Sync>, String> {
        use tracing_subscriber::layer::SubscriberExt as _;
        use tracing_subscriber::Layer as _;

        let filter = logs::filter(&self.inner.config.log_level)?;
        let registry = tracing_subscriber::registry()
            .with(logs::LogLayer::new(self.inner.config.log_format, sink).with_filter(filter));
        #[cfg(feature = "otel")]
        {
            let otel = self.inner.otel.as_ref().map(|otel| {
                otel.layer()
                    .with_filter(tracing_subscriber::filter::LevelFilter::INFO)
            });
            Ok(Box::new(registry.with(otel)))
        }
        #[cfg(not(feature = "otel"))]
        Ok(Box::new(registry))
    }

    /// Installs [`Observer::subscriber`] on stdout as the process default.
    ///
    /// # Errors
    ///
    /// The level does not parse, or a default subscriber is already set.
    #[cfg(feature = "logs")]
    pub fn install(&self) -> Result<(), String> {
        let subscriber = self.subscriber(logs::Stdout)?;
        tracing::subscriber::set_global_default(subscriber).map_err(|_| {
            "a tracing subscriber is already installed for this process\nhelp: set \
             `install_subscriber = false` and install `Observer::subscriber` yourself"
                .to_string()
        })
    }

    /// Exports what is buffered and stops the trace exporter.
    fn flush(&self) -> Result<(), String> {
        #[cfg(feature = "otel")]
        if let Some(otel) = &self.inner.otel {
            return otel.shutdown();
        }
        Ok(())
    }
}

/// The line a finished request writes: `info` below 500, `error` from it.
#[cfg(feature = "logs")]
fn log_request(span: &tracing::Span, method: &str, path: &str, status: u16, took: Duration) {
    let duration_ms = text::millis(took);
    if status >= 500 {
        span.record("otel.status_code", "ERROR");
        tracing::error!(target: "brocanth::request", parent: span, status, duration_ms, "{method} {path} -> {status}");
    } else {
        tracing::info!(target: "brocanth::request", parent: span, status, duration_ms, "{method} {path} -> {status}");
    }
}

/// SIGTERM, or Ctrl-C. Whichever comes first.
async fn terminated() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The observer, as a plugin.
///
/// Built from an [`Observer`] rather than from an [`ObserveConfig`], because
/// the application needs the observer before the plugins exist: the job
/// queue takes [`Observer::job_events`] when it is built, and the server
/// takes [`Observer::wrap`].
pub struct ObservePlugin {
    observer: Observer,
}

impl ObservePlugin {
    /// The observer this plugin registers.
    pub fn observer(&self) -> &Observer {
        &self.observer
    }
}

impl Plugin for ObservePlugin {
    type Config = Observer;
    type Provides = NoContext;
    const NAME: &'static str = "observe";

    fn new(observer: Observer) -> Self {
        Self { observer }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.observer.attach(lc.readiness().clone());
        lc.services_mut().insert(self.observer.clone());
        Ok(())
    }

    #[allow(unused_variables)]
    fn after_config(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        #[cfg(all(feature = "_jobs", feature = "metrics"))]
        if let Some(queue) = lc.services().get::<brocanth_jobs::Queue>() {
            self.observer.watch_queue(queue.clone());
        }
        Ok(())
    }

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        #[cfg(feature = "logs")]
        if self.observer.config().install_subscriber {
            self.observer.install().map_err(PluginError::new)?;
        }
        Ok(())
    }

    fn shutdown(&mut self, _sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        self.observer
            .flush()
            .map_err(|e| PluginError::new(format!("flushing the trace exporter: {e}")))
    }
}
