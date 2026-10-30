//! The observer from the outside: what a request gets, what a probe says,
//! what a scrape reads, and what a log line holds.

#![cfg(all(feature = "logs", feature = "metrics"))]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_shutdown::ShutdownReason;
use brocanth::routing::Verb;
use brocanth_observe::logs::Memory;
use brocanth_observe::{LogFormat, ObserveConfig, ObservePlugin, Observer};
use brocanth_server::{Request, Response};

fn body(response: &Response) -> String {
    String::from_utf8_lossy(response.body.bytes().expect("a whole body")).into_owned()
}

fn config() -> ObserveConfig {
    let mut config = ObserveConfig::new("test");
    config.install_subscriber = false;
    config.check_timeout = Duration::from_millis(200);
    config
}

/// Runs `f` on a current-thread runtime with `observer`'s subscriber set for
/// this thread, writing to the returned buffer.
fn with_logs<T>(observer: &Observer, f: impl std::future::Future<Output = T>) -> (T, Vec<String>) {
    let memory = Memory::new();
    let subscriber = observer.subscriber(memory.clone()).expect("a subscriber");
    let _default = tracing::subscriber::set_default(subscriber);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let out = runtime.block_on(f);
    (out, memory.lines())
}

/// A page that answers with the request id it was handed, in the body.
async fn echo_id(request: Request) -> Response {
    tracing::info!(page = "echo", "handler ran");
    let id = request.header("x-request-id").unwrap_or("none").to_string();
    Response::text(200, id)
}

/// The value of `"key":` in a JSON line, as written, up to the next comma.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let at = line.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = &line[at..];
    let end = if let Some(stripped) = rest.strip_prefix('"') {
        stripped.find('"')? + 2
    } else {
        rest.find([',', '}'])?
    };
    Some(&rest[..end])
}

// -- logs -----------------------------------------------------------------

#[test]
fn a_request_writes_one_json_line_with_the_request_fields() {
    let mut config = config();
    config.log_format = LogFormat::Json;
    let observer = Observer::new(config).unwrap();
    let handler = observer.wrap_routed(echo_id, |_| Some("/users/{id}".to_string()));

    let (response, lines) = with_logs(&observer, handler(Request::new(Verb::Get, "/users/7")));
    assert_eq!(response.status, 200);

    let request_line = lines
        .iter()
        .find(|l| l.contains("\"target\":\"brocanth::request\""))
        .expect("a request line");
    assert!(request_line.starts_with("{\"ts\":\""), "{request_line}");
    assert!(request_line.ends_with('}'), "{request_line}");
    assert_eq!(field(request_line, "level"), Some("\"info\""));
    assert_eq!(field(request_line, "msg"), Some("\"GET /users/7 -> 200\""));
    assert_eq!(field(request_line, "method"), Some("\"GET\""));
    assert_eq!(field(request_line, "path"), Some("\"/users/7\""));
    assert_eq!(field(request_line, "route"), Some("\"/users/{id}\""));
    assert_eq!(field(request_line, "status"), Some("200"));
    let id = field(request_line, "request_id").expect("a request id");
    assert_eq!(id.trim_matches('"'), body(&response));
    let duration: f64 = field(request_line, "duration_ms")
        .expect("a duration")
        .parse()
        .expect("a number");
    assert!(duration >= 0.0);
    // Not the probe fields of a tracer that is off.
    assert!(!request_line.contains("trace_id"), "{request_line}");

    // A line the handler wrote is joined to its request by the span.
    let handler_line = lines
        .iter()
        .find(|l| l.contains("handler ran"))
        .expect("the handler's line");
    assert_eq!(field(handler_line, "request_id"), Some(id));
    assert_eq!(field(handler_line, "page"), Some("\"echo\""));
}

#[test]
fn a_server_error_is_logged_at_error() {
    let mut config = config();
    config.log_format = LogFormat::Json;
    let observer = Observer::new(config).unwrap();
    let handler = observer.wrap(|_| async { Response::text(503, "down") });
    let (_, lines) = with_logs(&observer, handler(Request::new(Verb::Post, "/x")));
    assert_eq!(field(&lines[0], "level"), Some("\"error\""));
}

#[test]
fn a_text_line_reads_left_to_right() {
    let observer = Observer::new(config()).unwrap();
    let handler = observer.wrap(|_| async { Response::text(404, "no") });
    let (_, lines) = with_logs(
        &observer,
        handler(Request::new(Verb::Get, "/missing").with_header("x-request-id", "abc")),
    );
    let line = &lines[0];
    assert!(
        line.contains("  INFO GET /missing -> 404 request_id=abc method=GET path=/missing route=unmatched status=404 duration_ms="),
        "{line}"
    );
}

#[test]
fn probes_are_not_logged_unless_asked() {
    let observer = Observer::new(config()).unwrap();
    let handler = observer.wrap(echo_id);
    let (_, lines) = with_logs(&observer, handler(Request::new(Verb::Get, "/up")));
    assert!(lines.is_empty(), "{lines:?}");
}

// -- request id -----------------------------------------------------------

#[test]
fn an_incoming_request_id_reaches_the_handler_and_comes_back() {
    let observer = Observer::new(config()).unwrap();
    let handler = observer.wrap(echo_id);
    let (response, _) = with_logs(
        &observer,
        handler(Request::new(Verb::Get, "/").with_header("X-Request-Id", "from-the-proxy-1")),
    );
    assert_eq!(body(&response), "from-the-proxy-1");
    assert_eq!(response.header("x-request-id"), Some("from-the-proxy-1"));
}

#[test]
fn a_missing_or_unsafe_request_id_is_replaced_by_a_new_one() {
    let observer = Observer::new(config()).unwrap();
    let handler = observer.wrap(echo_id);
    for request in [
        Request::new(Verb::Get, "/"),
        Request::new(Verb::Get, "/").with_header("x-request-id", "two words"),
    ] {
        let (response, _) = with_logs(&observer, handler(request));
        let id = body(&response);
        assert_eq!(id.len(), 32, "{id}");
        assert_eq!(response.header("x-request-id"), Some(id.as_str()));
    }
}

#[test]
fn the_header_is_configurable() {
    let mut config = config();
    config.request_id_header = "x-correlation-id".to_string();
    let observer = Observer::new(config).unwrap();
    let handler = observer.wrap(|r: Request| async move {
        Response::text(200, r.header("x-correlation-id").unwrap_or("").to_string())
    });
    let (response, _) = with_logs(
        &observer,
        handler(Request::new(Verb::Get, "/").with_header("x-correlation-id", "c-1")),
    );
    assert_eq!(body(&response), "c-1");
    assert_eq!(response.header("x-correlation-id"), Some("c-1"));
}

// -- probes -----------------------------------------------------------------

/// A plugin with one readiness check, whose answer the test controls.
struct Database {
    up: Arc<Mutex<bool>>,
}

impl Plugin for Database {
    type Config = Arc<Mutex<bool>>;
    type Provides = NoContext;
    const NAME: &'static str = "database";

    fn new(up: Self::Config) -> Self {
        Self { up }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let up = Arc::clone(&self.up);
        lc.ready_when(move || {
            let up = *up.lock().unwrap();
            async move {
                if up {
                    Ok(())
                } else {
                    Err("connection refused".to_string())
                }
            }
        });
        Ok(())
    }
}

#[test]
fn up_answers_without_asking_anybody() {
    let observer = Observer::new(config()).unwrap();
    observer
        .readiness()
        .add("never", || async { Err("down".to_string()) });
    let handler = observer.wrap(echo_id);
    let (response, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/up")));
    assert_eq!(response.status, 200);
    assert_eq!(body(&response), r#"{"status":"ok"}"#);
    assert_eq!(response.header("cache-control"), Some("no-store"));
}

#[test]
fn readyz_reports_each_plugin_check_and_fails_when_one_fails() {
    let up = Arc::new(Mutex::new(true));
    let observer = Observer::new(config()).unwrap();
    let app = AppBuilder::new()
        .plugin(ObservePlugin::new(observer.clone()))
        .plugin(Database::new(Arc::clone(&up)))
        .build()
        .expect("builds");
    let handler = observer.wrap(echo_id);

    let (ready, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/readyz")));
    assert_eq!(ready.status, 200, "{}", body(&ready));
    let text = body(&ready);
    assert!(
        text.starts_with(
            r#"{"status":"ready","checks":[{"name":"database","status":"ok","latency_ms":"#
        ),
        "{text}"
    );

    *up.lock().unwrap() = false;
    let (unready, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/readyz")));
    assert_eq!(unready.status, 503);
    let text = body(&unready);
    assert!(text.contains(r#""status":"unready""#), "{text}");
    assert!(
        text.contains(r#""name":"database","status":"fail""#),
        "{text}"
    );
    assert!(text.contains(r#""error":"connection refused""#), "{text}");
    drop(app);
}

#[test]
fn a_check_that_hangs_fails_at_the_timeout_and_does_not_hang_the_probe() {
    let observer = Observer::new(config()).unwrap();
    observer.readiness().add("slow", || async {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Ok(())
    });
    let report = with_logs(&observer, observer.ready()).0;
    assert!(!report.is_ready());
    assert_eq!(
        report.checks[0].outcome,
        Err("timed out after 200 ms".to_string())
    );
}

#[test]
fn readyz_fails_once_shutdown_begins_and_before_any_plugin_drains() {
    let up = Arc::new(Mutex::new(true));
    let observer = Observer::new(config()).unwrap();
    let mut app = AppBuilder::new()
        .plugin(ObservePlugin::new(observer.clone()))
        .plugin(Database::new(up))
        .build()
        .expect("builds");
    let handler = observer.wrap(echo_id);

    let (before, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/readyz")));
    assert_eq!(before.status, 200);

    let report = app.shutdown(ShutdownReason::Signal, Some(Duration::from_secs(1)));
    assert!(report.is_clean(), "{report:?}");

    let (after, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/readyz")));
    assert_eq!(after.status, 503);
    assert_eq!(body(&after), r#"{"status":"draining","checks":[]}"#);
    // Liveness does not change: the process is stopping, not dead.
    let (live, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/up")));
    assert_eq!(live.status, 200);
}

#[test]
fn probe_paths_are_configurable_and_can_be_turned_off() {
    let mut config = config();
    config.live_path = Some("/livez".to_string());
    config.ready_path = None;
    let observer = Observer::new(config).unwrap();
    let handler = observer.wrap(|_| async { Response::text(200, "the app") });
    let (live, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/livez")));
    assert_eq!(body(&live), r#"{"status":"ok"}"#);
    let (ready, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/readyz")));
    assert_eq!(body(&ready), "the app");
}

// -- metrics ----------------------------------------------------------------

#[test]
fn metrics_count_requests_by_route_template_and_status_class() {
    let observer = Observer::new(config()).unwrap();
    let handler = observer.wrap_routed(
        |r: Request| async move {
            if r.path == "/users/404" {
                Response::text(404, "no")
            } else {
                Response::text(200, "yes")
            }
        },
        |r| {
            r.path
                .starts_with("/users/")
                .then(|| "/users/{id}".to_string())
        },
    );
    let (scrape, _) = with_logs(&observer, async {
        for path in ["/users/1", "/users/2", "/users/404", "/elsewhere"] {
            handler(Request::new(Verb::Get, path)).await;
        }
        handler(Request::new(Verb::Get, "/metrics")).await
    });

    assert_eq!(scrape.status, 200);
    assert_eq!(
        scrape.header("content-type"),
        Some(brocanth_observe::metrics::CONTENT_TYPE)
    );
    let text = body(&scrape);
    for expected in [
        r#"http_requests_total{method="GET",route="/users/{id}",status="2xx"} 2"#,
        r#"http_requests_total{method="GET",route="/users/{id}",status="4xx"} 1"#,
        r#"http_requests_total{method="GET",route="unmatched",status="2xx"} 1"#,
        r#"http_request_duration_seconds_count{method="GET",route="/users/{id}",status="2xx"} 2"#,
        r#"http_request_duration_seconds_bucket{le="0.005",method="GET",route="/users/{id}",status="2xx"}"#,
        "# TYPE http_requests_in_flight gauge",
        "http_requests_in_flight 0",
        "# EOF",
    ] {
        assert!(text.contains(expected), "missing `{expected}` in:\n{text}");
    }
    // The raw path is never a label, and the scrape is not counted.
    assert!(!text.contains("/users/1"), "{text}");
    assert!(!text.contains(r#"route="/metrics""#), "{text}");
}

#[test]
fn a_plugin_series_and_its_scrape_refresh_reach_the_output() {
    let observer = Observer::new(config()).unwrap();
    let depth: prometheus_client::metrics::gauge::Gauge = Default::default();
    observer
        .metrics()
        .register("example_queue_depth", "Rows waiting", depth.clone());
    observer.metrics().on_scrape(move || {
        let depth = depth.clone();
        async move {
            depth.set(42);
        }
    });
    let handler = observer.wrap(echo_id);
    let (scrape, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/metrics")));
    assert!(
        body(&scrape).contains("example_queue_depth 42"),
        "{}",
        body(&scrape)
    );
}

#[test]
fn a_metrics_token_keeps_out_a_scrape_without_it() {
    let mut config = config();
    config.metrics_token = Some("s3cret".to_string());
    let observer = Observer::new(config).unwrap();
    let handler = observer.wrap(echo_id);
    let (refused, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/metrics")));
    assert_eq!(refused.status, 401);
    let (allowed, _) = with_logs(
        &observer,
        handler(Request::new(Verb::Get, "/metrics").with_header("authorization", "Bearer s3cret")),
    );
    assert_eq!(allowed.status, 200);
}

#[test]
fn with_a_metrics_bind_the_main_listener_does_not_serve_metrics() {
    let mut config = config();
    config.metrics_bind = Some("127.0.0.1:0".to_string());
    let observer = Observer::new(config).unwrap();
    let handler = observer.wrap(|_| async { Response::text(404, "the app's own 404") });
    let (main, _) = with_logs(&observer, handler(Request::new(Verb::Get, "/metrics")));
    assert_eq!(body(&main), "the app's own 404");
}

// -- over a socket ------------------------------------------------------------

async fn get(addr: std::net::SocketAddr, path: &str) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nhost: x\r\nconnection: close\r\n\r\n").as_bytes(),
        )
        .await?;
    let mut out = String::new();
    stream.read_to_string(&mut out).await?;
    Ok(out)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_fails_readiness_first_keeps_serving_for_the_delay_then_stops() {
    let mut config = config();
    config.drain_delay = Duration::from_millis(400);
    let observer = Observer::new(config).unwrap();
    let server = brocanth_server::Server::bind("127.0.0.1:0").await.unwrap();
    let addr = server.local_addr().unwrap();
    let stopper = server.stopper();
    let serving = tokio::spawn(server.run(observer.wrap(echo_id)));

    let ready = get(addr, "/readyz").await.unwrap();
    assert!(ready.starts_with("HTTP/1.1 200"), "{ready}");

    let stopping = {
        let observer = observer.clone();
        tokio::spawn(async move { observer.drain_then_stop(stopper).await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Inside the delay: still answering, and answering "not ready".
    let draining = get(addr, "/readyz").await.unwrap();
    assert!(draining.starts_with("HTTP/1.1 503"), "{draining}");
    assert!(
        draining.contains(r#"{"status":"draining","checks":[]}"#),
        "{draining}"
    );
    let page = get(addr, "/").await.unwrap();
    assert!(page.starts_with("HTTP/1.1 200"), "{page}");
    assert!(
        page.to_ascii_lowercase().contains("x-request-id: "),
        "{page}"
    );

    stopping.await.unwrap();
    serving.await.unwrap().unwrap();
    assert!(get(addr, "/up").await.is_err(), "the listener is closed");
}
