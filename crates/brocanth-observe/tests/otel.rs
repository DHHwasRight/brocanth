//! W3C trace context in and out, with spans read back from an in-memory
//! exporter.

#![cfg(feature = "otel")]

use opentelemetry::trace::{SpanKind, Status};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

use brocanth::routing::Verb;
use brocanth_observe::logs::Memory;
use brocanth_observe::{LogFormat, ObserveConfig, Observer};
use brocanth_server::{Request, Response};

const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const PARENT_ID: &str = "00f067aa0ba902b7";

fn observer() -> (Observer, InMemorySpanExporter, SdkTracerProvider) {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let mut config = ObserveConfig::new("shop");
    config.install_subscriber = false;
    config.log_format = LogFormat::Json;
    (
        Observer::with_tracer_provider(config, provider.clone()),
        exporter,
        provider,
    )
}

fn run<T>(observer: &Observer, f: impl std::future::Future<Output = T>) -> (T, Vec<String>) {
    let memory = Memory::new();
    let _default = tracing::subscriber::set_default(observer.subscriber(memory.clone()).unwrap());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    (runtime.block_on(f), memory.lines())
}

async fn page(_: Request) -> Response {
    let _child = tracing::info_span!("render").entered();
    Response::text(200, "ok")
}

#[test]
fn an_incoming_traceparent_is_the_parent_and_the_response_names_the_child() {
    let (observer, exporter, provider) = observer();
    let handler = observer.wrap_routed(page, |_| Some("/users/{id}".to_string()));
    let request = Request::new(Verb::Get, "/users/7")
        .with_header("traceparent", &format!("00-{TRACE_ID}-{PARENT_ID}-01"));

    let (response, lines) = run(&observer, handler(request));
    provider.force_flush().unwrap();

    let spans = exporter.get_finished_spans().unwrap();
    let server = spans
        .iter()
        .find(|s| s.name == "GET /users/{id}")
        .expect("the request span");
    assert_eq!(server.span_kind, SpanKind::Server);
    assert_eq!(server.span_context.trace_id().to_string(), TRACE_ID);
    assert_eq!(server.parent_span_id.to_string(), PARENT_ID);
    assert_eq!(server.status, Status::Unset);

    // A span the handler opened is the request span's child.
    let render = spans
        .iter()
        .find(|s| s.name == "render")
        .expect("the child");
    assert_eq!(render.parent_span_id, server.span_context.span_id());

    // Out: the response carries the request span, in the same trace.
    let traceparent = response.header("traceparent").expect("a traceparent");
    assert_eq!(
        traceparent,
        format!("00-{TRACE_ID}-{}-01", server.span_context.span_id())
    );

    // And the log line carries both ids.
    let line = lines
        .iter()
        .find(|l| l.contains("brocanth::request"))
        .expect("a request line");
    assert!(
        line.contains(&format!("\"trace_id\":\"{TRACE_ID}\"")),
        "{line}"
    );
    assert!(
        line.contains(&format!(
            "\"span_id\":\"{}\"",
            server.span_context.span_id()
        )),
        "{line}"
    );
    assert!(!line.contains("otel."), "{line}");
}

#[test]
fn without_a_traceparent_a_new_trace_starts_and_is_returned() {
    let (observer, exporter, provider) = observer();
    let handler = observer.wrap(page);
    let (response, _) = run(&observer, handler(Request::new(Verb::Get, "/")));
    provider.force_flush().unwrap();

    let spans = exporter.get_finished_spans().unwrap();
    let server = spans
        .iter()
        .find(|s| s.name == "GET unmatched")
        .expect("the request span");
    assert!(server.parent_span_id == opentelemetry::trace::SpanId::INVALID);
    let traceparent = response.header("traceparent").unwrap();
    assert!(
        traceparent.contains(&server.span_context.trace_id().to_string()),
        "{traceparent}"
    );
}

#[test]
fn a_server_error_marks_the_span_as_an_error() {
    let (observer, exporter, provider) = observer();
    let handler = observer.wrap(|_| async { Response::text(500, "boom") });
    run(&observer, handler(Request::new(Verb::Get, "/")));
    provider.force_flush().unwrap();
    let spans = exporter.get_finished_spans().unwrap();
    assert!(
        matches!(spans[0].status, Status::Error { .. }),
        "{:?}",
        spans[0].status
    );
}
