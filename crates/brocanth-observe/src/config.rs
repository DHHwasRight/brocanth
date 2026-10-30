//! What an [`Observer`](crate::Observer) is built from.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

/// How a log line is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per line. For a log collector.
    Json,
    /// `ts LEVEL msg key=value ...`. For a person at a terminal.
    Text,
}

impl LogFormat {
    /// The default for an environment: JSON in `production`, text elsewhere.
    ///
    /// A collector reads production. A person reads development, and a JSON
    /// line in a terminal is a wall.
    pub fn for_env(env: &str) -> Self {
        if env.eq_ignore_ascii_case("production") {
            Self::Json
        } else {
            Self::Text
        }
    }

    /// The format a config value names, or the default for `env` when the
    /// value is absent or empty.
    ///
    /// # Errors
    ///
    /// A value that is neither `json` nor `text`, with a `help:` line.
    pub fn resolve(value: Option<&str>, env: &str) -> Result<Self, String> {
        match value.map(str::trim) {
            None | Some("") => Ok(Self::for_env(env)),
            Some(value) => value.parse(),
        }
    }
}

impl FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(Self::Json),
            "text" => Ok(Self::Text),
            other => Err(format!(
                "log format `{other}` is unknown\nhelp: write `format = \"json\"` or `format = \"text\"`"
            )),
        }
    }
}

impl fmt::Display for LogFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Json => "json",
            Self::Text => "text",
        })
    }
}

/// Everything the observer reads. Build it with [`ObserveConfig::new`] and
/// change the fields you need.
///
/// A path set to [`None`] turns that endpoint off.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ObserveConfig {
    /// The service name: in every trace, and the default `service.name`.
    pub service: String,
    /// JSON or text. See [`LogFormat::for_env`] for the default.
    pub log_format: LogFormat,
    /// A level (`info`) or per-target levels (`info,sqlx=warn`).
    /// `RUST_LOG`, when set, wins.
    pub log_level: String,
    /// Log probe and metrics requests too. Off: a probe every few seconds on
    /// every replica buries the requests a person came to read.
    pub log_probes: bool,
    /// Install the subscriber as the process default when the plugin
    /// starts. Off for a test that installs its own per thread.
    pub install_subscriber: bool,
    /// The header the request id travels in, in and out.
    pub request_id_header: String,
    /// Liveness: answers 200 while the process can answer at all.
    pub live_path: Option<String>,
    /// Readiness: runs every registered check.
    pub ready_path: Option<String>,
    /// How long one readiness check (or one metrics refresh) may take.
    pub check_timeout: Duration,
    /// After a stop signal: how long to answer "not ready" while still
    /// serving, before the listener stops accepting. Kubernetes needs a few
    /// seconds to remove the pod from its endpoints.
    pub drain_delay: Duration,
    /// Prometheus metrics.
    #[cfg(feature = "metrics")]
    pub metrics_path: Option<String>,
    /// When set, `/metrics` needs `Authorization: Bearer <token>`.
    #[cfg(feature = "metrics")]
    pub metrics_token: Option<String>,
    /// When set, `/metrics` is served on this address only, and not on the
    /// application's own listener. See
    /// [`Observer::serve_admin`](crate::Observer::serve_admin).
    #[cfg(feature = "metrics")]
    pub metrics_bind: Option<String>,
    /// OpenTelemetry tracing.
    #[cfg(feature = "otel")]
    pub tracing: TracingConfig,
}

impl ObserveConfig {
    /// The defaults, for a service called `service`: text logs at `info`,
    /// `/up`, `/readyz` and `/metrics`, a two-second check timeout, no drain
    /// delay.
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
            log_format: LogFormat::Text,
            log_level: "info".to_string(),
            log_probes: false,
            install_subscriber: true,
            request_id_header: "x-request-id".to_string(),
            live_path: Some("/up".to_string()),
            ready_path: Some("/readyz".to_string()),
            check_timeout: Duration::from_secs(2),
            drain_delay: Duration::ZERO,
            #[cfg(feature = "metrics")]
            metrics_path: Some("/metrics".to_string()),
            #[cfg(feature = "metrics")]
            metrics_token: None,
            #[cfg(feature = "metrics")]
            metrics_bind: None,
            #[cfg(feature = "otel")]
            tracing: TracingConfig::default(),
        }
    }
}

impl Default for ObserveConfig {
    fn default() -> Self {
        Self::new("brocanth")
    }
}

/// OpenTelemetry export.
///
/// The standard `OTEL_*` variables win over these fields:
/// `OTEL_EXPORTER_OTLP_ENDPOINT` (and `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`),
/// `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_TRACES_SAMPLER` and
/// `OTEL_TRACES_SAMPLER_ARG`. An operator changes a deployment without a
/// rebuild; the config file holds the defaults.
#[cfg(feature = "otel")]
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TracingConfig {
    /// Export spans. Off: the feature is compiled in and nothing is sent.
    pub enabled: bool,
    /// The OTLP/HTTP traces URL, such as `http://collector:4318/v1/traces`.
    /// [`None`]: the environment, then `http://localhost:4318/v1/traces`.
    pub endpoint: Option<String>,
    /// The fraction of new traces to keep, `0.0..=1.0`. A request that
    /// arrives with a sampled `traceparent` is always kept, so one trace is
    /// never half-exported.
    pub sample_ratio: f64,
}

#[cfg(feature = "otel")]
impl Default for TracingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: None,
            sample_ratio: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_defaults_to_json_and_everything_else_to_text() {
        assert_eq!(LogFormat::resolve(None, "production"), Ok(LogFormat::Json));
        assert_eq!(
            LogFormat::resolve(Some(""), "production"),
            Ok(LogFormat::Json)
        );
        assert_eq!(LogFormat::resolve(None, "development"), Ok(LogFormat::Text));
        assert_eq!(
            LogFormat::resolve(Some("json"), "development"),
            Ok(LogFormat::Json)
        );
        assert_eq!(
            LogFormat::resolve(Some("TEXT"), "production"),
            Ok(LogFormat::Text)
        );
    }

    #[test]
    fn an_unknown_format_says_what_to_write() {
        let error = LogFormat::resolve(Some("yaml"), "production").unwrap_err();
        assert!(error.contains("`yaml`"), "{error}");
        assert!(error.contains("help: "), "{error}");
    }
}
