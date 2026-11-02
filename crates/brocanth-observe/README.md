# brocanth-observe

Day-2 observability for a brocanth application: a request id, structured
logs on stdout, `/up` and `/readyz` probes, Prometheus metrics, and
OpenTelemetry traces behind a feature. An `Observer` wraps the one function
`brocanth_server::Server::run` takes. So it sees every request: the 404, the
static file and the handler that panicked, not only a routed page. Use it
for any application you deploy. Do not use it for request-scoped logic
(authentication, rewriting a request): that is a hook or a middleware
layer. An application that does not name `ObservePlugin` links none of it.

| What | Feature |
| --- | --- |
| One JSON or text line per request and per `tracing` event, with a request id | `logs` (default) |
| `/up` (liveness), `/readyz` (each plugin's readiness check, with latency) | always |
| `/metrics`: requests by route template and status class, in-flight, plugin series | `metrics` (default) |
| A span per request and per job run, OTLP over HTTP, W3C `traceparent` | `otel` (off) |
| A span, a log line and a counter per job run; queue depth gauges | `jobs` (Postgres) or `jobs-sqlite` (off) |

**Reason for the defaults:** every deployed application wants logs and
metrics, and both are small. Traces need a collector, so they wait to be
asked for.

## Getting started

### In a generated project

```sh
brocanth create my_app --with observability
brocanth add observability
```

`observability` has four children. Three are on by default:
`observability-logs`, `observability-health`, `observability-metrics`.
`observability-tracing` is off; name it to turn it on. `observability-logs`
and the older `logging` module are refused together.

The module writes `src/plugins/observability/mod.rs`, registers the plugin,
and wraps the generated `serve` handler with the `Observer`. It declares
these settings in the `[observability]` section (see `docs/module-flags.md`).
Each child adds only its own keys.

| Key | Child | Default | Flag / env |
| --- | --- | --- | --- |
| `format` | logs | unset: json in production, text elsewhere | `--observability.format`, `$MY_APP_OBSERVABILITY__FORMAT` |
| `level` | logs | `info` (`RUST_LOG` wins) | `--observability.level` |
| `live_path` | health | `/up` | `--observability.live_path` |
| `ready_path` | health | `/readyz` | `--observability.ready_path` |
| `drain_delay_secs` | health | `0` | `--observability.drain_delay_secs` |
| `metrics_path` | metrics | `/metrics` | `--observability.metrics_path` |
| `metrics_token` | metrics | unset (secret: no flag) | `$MY_APP_OBSERVABILITY__METRICS_TOKEN` |
| `metrics_bind` | metrics | unset: same listener | `--observability.metrics_bind` |
| `otlp_endpoint` | tracing | unset: `OTEL_EXPORTER_OTLP_ENDPOINT`, then `localhost:4318` | `--observability.otlp_endpoint` |
| `sample_ratio` | tracing | `1.0` | `--observability.sample_ratio` |

Every key also has a variable `$<APP>_OBSERVABILITY__<KEY>`. An empty path
turns that endpoint off.

### By hand

```toml
[dependencies]
brocanth-observe = { git = "https://github.com/DHHwasRight/brocanth.git" }
# Traces too:
# brocanth-observe = { git = "...", features = ["otel"] }
```

The `scaffold` feature holds the generator module. Only the CLI turns it on.

## Examples

### Wire it into a server

Build the `Observer` before the application. The job queue takes
`observer.job_events()` when it is built, and the server takes
`observer.wrap(..)`. Then register it with `ObservePlugin`: the plugin
attaches the application's `Readiness` to `/readyz`, puts the `Observer` in
`Services`, installs the log subscriber in `before_start`, and flushes the
trace exporter in `shutdown`.

```rust,no_run
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_observe::{ObserveConfig, ObservePlugin, Observer};
use brocanth_server::{Response, Server};

# async fn f() -> Result<(), Box<dyn std::error::Error>> {
let observer = Observer::new(ObserveConfig::new("shop"))?;
let _app = AppBuilder::new()
    .plugin(ObservePlugin::new(observer.clone()))
    .build()?;

let server = Server::bind("0.0.0.0:3000").await?;
// SIGTERM: /readyz fails first, the server drains, then it stops accepting.
tokio::spawn(observer.clone().stop_on_signal(server.stopper()));
server
    .run(observer.wrap_routed(
        |request| async move { Response::text(200, format!("you asked for {}", request.path)) },
        // The route *template*, never the raw path: one label per user id
        // is a cardinality explosion.
        |r| r.path.starts_with("/users/").then(|| "/users/{id}".to_string()),
    ))
    .await?;
# Ok(())
# }
```

`no_run`: it binds a socket.

### Probes

`/up` answers 200 while the process can answer. `/readyz` runs every
readiness check under `check_timeout` and answers 503 when one fails.

```rust
use brocanth::routing::Verb;
use brocanth_observe::{ObserveConfig, Observer};
use brocanth_server::{Request, Response};

let mut config = ObserveConfig::new("shop");
config.install_subscriber = false;
let observer = Observer::new(config).unwrap();
observer
    .readiness()
    .add("database", || async { Err("connection refused".to_string()) });

let handler = observer.wrap(|_| async { Response::text(200, "page") });
let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();

assert_eq!(rt.block_on(handler(Request::new(Verb::Get, "/up"))).status, 200);
assert_eq!(rt.block_on(handler(Request::new(Verb::Get, "/readyz"))).status, 503);
assert_eq!(rt.block_on(handler(Request::new(Verb::Get, "/home"))).status, 200);
```

### A plugin's own metric

`Observer::metrics` is the registry `/metrics` writes. It takes a
`prometheus-client` metric. `on_scrape` sets a gauge whose value lives
somewhere slow, under the check timeout.

```rust
use brocanth::routing::Verb;
use brocanth_observe::{ObserveConfig, Observer};
use brocanth_server::{Request, Response};
use prometheus_client::metrics::gauge::Gauge;

let mut config = ObserveConfig::new("shop");
config.install_subscriber = false;
let observer = Observer::new(config).unwrap();

let depth: Gauge = Gauge::default();
observer.metrics().register("outbox_depth", "Rows waiting to send", depth.clone());
observer.metrics().on_scrape(move || {
    let depth = depth.clone();
    async move {
        depth.set(42);
    }
});

let handler = observer.wrap(|_| async { Response::text(404, "no") });
let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
let scrape = rt.block_on(handler(Request::new(Verb::Get, "/metrics")));
let text = String::from_utf8_lossy(scrape.body.bytes().unwrap()).into_owned();
assert!(text.contains("outbox_depth 42"));
```

Set `metrics_token` to require `Authorization: Bearer <token>`. Set
`metrics_bind` to serve `/metrics` only on a second listener
(`Observer::serve_admin`), on a port the load balancer does not expose.

### Read the log lines in a test

`logs::Memory` is a sink that keeps the lines. `Observer::subscriber`
builds the subscriber; set it for one thread so tests do not share it.

```rust
use brocanth::routing::Verb;
use brocanth_observe::logs::Memory;
use brocanth_observe::{LogFormat, ObserveConfig, Observer};
use brocanth_server::{Request, Response};

let mut config = ObserveConfig::new("shop");
config.install_subscriber = false;
config.log_format = LogFormat::Json;
let observer = Observer::new(config).unwrap();

let memory = Memory::new();
let _guard = tracing::subscriber::set_default(observer.subscriber(memory.clone()).unwrap());

let handler = observer.wrap(|_| async { Response::text(200, "ok") });
let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
let response = rt.block_on(handler(Request::new(Verb::Get, "/users/7")));

// A request id comes back on the response and is in the log line.
let id = response.header("x-request-id").unwrap().to_string();
let line = memory.lines().into_iter().find(|l| l.contains("brocanth::request")).unwrap();
assert!(line.contains(&id));
```

## Testing

```sh
cargo test -p brocanth-observe                        # logs + metrics
cargo test -p brocanth-observe --features otel        # traces
cargo test -p brocanth-observe --features jobs-sqlite # job telemetry
```

No database is needed. The trace tests use the SDK's
`InMemorySpanExporter` through `Observer::with_tracer_provider`.

An application tests the observer by calling the function `wrap` returns
with a `brocanth_server::Request`, as above. `brocanth-test`'s `TestClient`
drives the `App`, not the server function, so it does not pass through the
observer. Use `TestClient` for the pages and this for the probes, metrics
and logs. See `crates/brocanth-test/README.md`.

## Links

- `docs/observability.md`: the log format, the request id, the readiness
  seam, the shutdown order (ADR 2), the metric names, the trace export, and
  what is not done yet.
- `docs/module-flags.md`: how the settings become flags, variables and file
  keys.
