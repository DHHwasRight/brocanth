//! `/metrics`: the request series, and a registry plugins add to.
//!
//! Three series for every application:
//!
//! - `http_requests_total{method,route,status}`, a counter;
//! - `http_request_duration_seconds{method,route,status}`, a histogram;
//! - `http_requests_in_flight`, a gauge.
//!
//! `route` is the route **template** (`/users/{id}`), never the raw path:
//! one label value per user id is a cardinality explosion that takes the
//! Prometheus server down. A request no route matched is `unmatched`.
//! `status` is the class (`2xx`, `4xx`), for the same reason.
//!
//! A plugin adds its own series with [`Metrics::register`], and a value that
//! has to be read from somewhere slow, such as a queue depth from a table,
//! with [`Metrics::on_scrape`]: the refresh runs when Prometheus asks, under
//! the check timeout, and not on a timer nobody reads.

use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use brocanth::plugin_routes::BoxFuture;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::{Metric, Registry};

/// The content type of the body [`Metrics::render`] returns.
pub const CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// The label a request gets when no route matched it.
pub const UNMATCHED: &str = "unmatched";

/// Seconds. From 5 ms to 10 s: a page under 5 ms is one bucket, and a
/// request past 10 s is a problem whatever its exact length.
const LATENCY_BUCKETS: [f64; 11] = [
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
];

type Refresh = Arc<dyn Fn() -> BoxFuture<'static, ()> + Send + Sync>;

/// One application's metrics.
///
/// Owned by an [`Observer`](crate::Observer), never global: two
/// applications in one process have two registries and do not count each
/// other's requests.
pub struct Metrics {
    registry: Mutex<Registry>,
    refreshers: Mutex<Vec<Refresh>>,
    pub(crate) http: HttpMetrics,
}

impl Metrics {
    pub(crate) fn new() -> Self {
        let mut registry = Registry::default();
        let http = HttpMetrics::new();
        registry.register(
            "http_requests",
            "Requests answered, by method, route template and status class",
            http.requests.clone(),
        );
        registry.register(
            "http_request_duration_seconds",
            "Time from the request arriving to the response headers being ready",
            http.duration.clone(),
        );
        registry.register(
            "http_requests_in_flight",
            "Requests being answered now",
            http.in_flight.clone(),
        );
        Self {
            registry: Mutex::new(registry),
            refreshers: Mutex::new(Vec::new()),
            http,
        }
    }

    /// Adds a series.
    ///
    /// `metric` is a `prometheus-client` metric: a [`Counter`], a [`Gauge`],
    /// a [`Histogram`], or a [`Family`] of one. Keep a clone and update it;
    /// every clone is the same series. A counter's name gets `_total` when it
    /// is written out.
    pub fn register(&self, name: &str, help: &str, metric: impl Metric) {
        self.registry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .register(name, help, metric);
    }

    /// Runs `refresh` before every scrape, to set gauges whose value lives
    /// somewhere slow.
    ///
    /// It runs as a task under the observer's check timeout. A refresh that
    /// fails or times out leaves the gauges at their last value; it logs and
    /// does not fail the scrape.
    pub fn on_scrape<F, Fut>(&self, refresh: F)
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let refresh: Refresh = Arc::new(move || Box::pin(refresh()));
        self.refreshers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(refresh);
    }

    /// Runs the refreshers, then writes every series as OpenMetrics text.
    pub async fn render(&self, timeout: Duration) -> String {
        let refreshers = self
            .refreshers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let tasks: Vec<_> = refreshers
            .into_iter()
            .map(|refresh| tokio::spawn(tokio::time::timeout(timeout, refresh())))
            .collect();
        for task in tasks {
            #[allow(unused_variables)]
            if let Ok(Err(elapsed)) = task.await {
                #[cfg(feature = "logs")]
                tracing::warn!(target: "brocanth::metrics", "a metrics refresh timed out: {elapsed}");
            }
        }
        self.encode()
    }

    /// Writes every series as they are now, without refreshing.
    pub fn encode(&self) -> String {
        let mut out = String::new();
        let registry = self.registry.lock().unwrap_or_else(PoisonError::into_inner);
        // A `fmt::Write` into a `String` does not fail.
        let _ = prometheus_client::encoding::text::encode(&mut out, &registry);
        out
    }
}

impl std::fmt::Debug for Metrics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Metrics").finish_non_exhaustive()
    }
}

/// The labels on both request series.
#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub(crate) struct HttpLabels {
    method: String,
    route: String,
    status: String,
}

pub(crate) struct HttpMetrics {
    requests: Family<HttpLabels, Counter>,
    duration: Family<HttpLabels, Histogram>,
    in_flight: Gauge<i64, std::sync::atomic::AtomicI64>,
}

impl HttpMetrics {
    fn new() -> Self {
        Self {
            requests: Family::default(),
            duration: Family::new_with_constructor(|| Histogram::new(LATENCY_BUCKETS)),
            in_flight: Gauge::default(),
        }
    }

    /// One more request in flight, until the guard drops. A guard, so a
    /// handler that panics still leaves the gauge right.
    pub(crate) fn enter(&self) -> InFlight {
        self.in_flight.inc();
        InFlight(self.in_flight.clone())
    }

    pub(crate) fn observe(&self, method: &str, route: &str, status: u16, took: Duration) {
        let labels = HttpLabels {
            method: method.to_string(),
            route: route.to_string(),
            status: status_class(status).to_string(),
        };
        self.requests.get_or_create(&labels).inc();
        self.duration
            .get_or_create(&labels)
            .observe(took.as_secs_f64());
    }
}

pub(crate) struct InFlight(Gauge<i64, std::sync::atomic::AtomicI64>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.dec();
    }
}

fn status_class(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        _ => "5xx",
    }
}

/// A gauge holding a float, for seconds and ratios.
pub type FloatGauge = Gauge<f64, AtomicU64>;

/// `true` when `header` is `Bearer <token>`, compared in constant time.
pub(crate) fn authorized(header: Option<&str>, token: &str) -> bool {
    let Some(given) = header.and_then(|h| h.strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (given.trim().as_bytes(), token.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_is_counted_by_its_class() {
        assert_eq!(status_class(204), "2xx");
        assert_eq!(status_class(404), "4xx");
        assert_eq!(status_class(503), "5xx");
    }

    #[test]
    fn only_the_right_bearer_token_is_let_in() {
        assert!(authorized(Some("Bearer s3cret"), "s3cret"));
        assert!(!authorized(Some("Bearer s3cre"), "s3cret"));
        assert!(!authorized(Some("s3cret"), "s3cret"));
        assert!(!authorized(None, "s3cret"));
    }

    #[test]
    fn a_plugin_series_is_written_beside_the_request_series() {
        let metrics = Metrics::new();
        let runs: Counter = Counter::default();
        metrics.register("brocanth_example_runs", "Example runs", runs.clone());
        runs.inc_by(3);
        metrics
            .http
            .observe("GET", "/users/{id}", 200, Duration::from_millis(3));
        let text = metrics.encode();
        assert!(text.contains("brocanth_example_runs_total 3"), "{text}");
        assert!(
            text.contains(
                r#"http_requests_total{method="GET",route="/users/{id}",status="2xx"} 1"#
            ),
            "{text}"
        );
    }
}
