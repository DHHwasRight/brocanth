//! OpenTelemetry: a span per request and per job run, exported over OTLP.
//!
//! The spans are `tracing` spans. `tracing-opentelemetry` turns them into
//! OpenTelemetry spans, so a span a handler or `sqlx` opens inside a request
//! becomes a child of the request's span with nothing written for it.
//!
//! W3C trace context in and out: an incoming `traceparent` becomes the
//! request span's parent, and the response carries the request span's own
//! `traceparent`, so a caller can find the trace it started.

use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::trace::{TraceContextExt, TracerProvider as _};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

use brocanth_server::{Request, Response};

use crate::config::TracingConfig;

/// The tracer provider and the propagator, held by the observer.
#[derive(Clone, Debug)]
pub(crate) struct Otel {
    provider: SdkTracerProvider,
    propagator: TraceContextPropagator,
}

impl Otel {
    /// An OTLP/HTTP exporter, configured from `config` and the `OTEL_*`
    /// environment, which wins.
    pub(crate) fn from_config(service: &str, config: &TracingConfig) -> Result<Self, String> {
        let mut exporter = opentelemetry_otlp::SpanExporter::builder().with_http();
        if let Some(endpoint) = config.endpoint.as_deref().filter(|e| !e.is_empty()) {
            if !env_set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
                && !env_set("OTEL_EXPORTER_OTLP_ENDPOINT")
            {
                use opentelemetry_otlp::WithExportConfig as _;
                exporter = exporter.with_endpoint(endpoint);
            }
        }
        let exporter = exporter.build().map_err(|e| {
            format!(
                "cannot build the OTLP exporter: {e}\nhelp: set `tracing.endpoint` or \
                 OTEL_EXPORTER_OTLP_ENDPOINT to a collector URL such as http://localhost:4318"
            )
        })?;

        let mut builder = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
            .with_resource(resource(service));
        // The SDK reads OTEL_TRACES_SAMPLER itself. Set a sampler only when
        // the environment did not, or the environment would lose.
        if !env_set("OTEL_TRACES_SAMPLER") {
            let ratio = config.sample_ratio.clamp(0.0, 1.0);
            builder = builder.with_sampler(Sampler::ParentBased(Box::new(
                Sampler::TraceIdRatioBased(ratio),
            )));
        }
        Ok(Self::with_provider(builder.build()))
    }

    /// A provider built elsewhere: an in-memory exporter in a test, or an
    /// application's own processors.
    pub(crate) fn with_provider(provider: SdkTracerProvider) -> Self {
        Self {
            provider,
            propagator: TraceContextPropagator::new(),
        }
    }

    /// The `tracing` layer that turns spans into OpenTelemetry spans.
    pub(crate) fn layer<S>(&self) -> impl Layer<S> + Send + Sync + 'static
    where
        S: Subscriber + for<'a> LookupSpan<'a> + Send + Sync,
    {
        tracing_opentelemetry::layer().with_tracer(self.provider.tracer("brocanth"))
    }

    /// Makes `span` a child of the request's `traceparent`, if it has one,
    /// and returns its trace and span ids as hex.
    pub(crate) fn adopt(
        &self,
        span: &tracing::Span,
        request: &Request,
    ) -> Option<(String, String)> {
        let parent = self.propagator.extract(&Headers(request));
        if parent.span().span_context().is_valid() {
            // Fails only when the span has no OpenTelemetry layer under it,
            // and then there is nothing to parent.
            let _ = span.set_parent(parent);
        }
        let context = span.context();
        let span_context = context.span().span_context().clone();
        span_context.is_valid().then(|| {
            (
                span_context.trace_id().to_string(),
                span_context.span_id().to_string(),
            )
        })
    }

    /// Adds `traceparent` (and `tracestate`) for `span` to the response.
    pub(crate) fn inject(&self, span: &tracing::Span, response: Response) -> Response {
        let mut out = Vec::new();
        self.propagator
            .inject_context(&span.context(), &mut Pairs(&mut out));
        out.into_iter().fold(response, |response, (name, value)| {
            response.without_header(&name).with_header(&name, &value)
        })
    }

    /// Exports what is buffered and stops the exporter.
    pub(crate) fn shutdown(&self) -> Result<(), String> {
        self.provider.shutdown().map_err(|e| e.to_string())
    }
}

fn env_set(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| !v.trim().is_empty())
}

/// `service.name` from the config, unless `OTEL_SERVICE_NAME` says
/// otherwise. `Resource::builder` reads that variable and
/// `OTEL_RESOURCE_ATTRIBUTES` itself.
fn resource(service: &str) -> Resource {
    let builder = Resource::builder();
    if env_set("OTEL_SERVICE_NAME") {
        builder.build()
    } else {
        builder.with_service_name(service.to_string()).build()
    }
}

struct Headers<'a>(&'a Request);

impl Extractor for Headers<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.header(key)
    }

    fn keys(&self) -> Vec<&str> {
        self.0.headers().map(|(name, _)| name).collect()
    }
}

struct Pairs<'a>(&'a mut Vec<(String, String)>);

impl Injector for Pairs<'_> {
    fn set(&mut self, key: &str, value: String) {
        self.0.push((key.to_string(), value));
    }
}
