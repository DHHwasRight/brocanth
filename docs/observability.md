# Observability: logs, probes, metrics, traces

`brocanth-observe` closes the four d2s day-2 gaps. The core assumes none of
them (PLAN.md §8). An application that does not name `ObservePlugin` links
none of this.

| Concern | What you get | Feature |
| --- | --- | --- |
| Logs | One JSON or text line per request and per `tracing` event, on stdout, with a request id | `logs` (default) |
| Probes | `/up` (liveness), `/readyz` (each plugin's check, with latency) | always |
| Metrics | `/metrics`, Prometheus text | `metrics` (default) |
| Traces | A span per request and per job run, OTLP export, W3C `traceparent` | `otel` (off) |
| Jobs | A span, a log line and a counter per job run; queue depth gauges | `jobs` / `jobs-sqlite` (off) |

## Where it attaches, and why a new crate

At the one function `brocanth_server::Server::run` takes. `Observer::wrap`
returns a function of the same shape. So the observer sees every request:
the 404, the static file and the route that panicked, not only a page the
router matched. A middleware layer would miss all three, because the chain
runs only for a matched route.

No existing crate owns this. `brocanth-server` owns sockets and framing and
names no log layer. The `logging` module is a project file with no crate.
`brocanth-middleware` layers run inside the chain. One concern with one
input — a request in, a response out — is one crate.

```rust
let observer = Observer::new(config)?;           // before the app
let app = AppBuilder::new()
    .plugin(ObservePlugin::new(observer.clone())) // readiness, services, subscriber
    /* ... */
    .build()?;

let server = Server::bind(addr).await?;
tokio::spawn(observer.clone().stop_on_signal(server.stopper()));
server
    .run(observer.wrap_routed(handler, move |r| {
        router.at(&r.path).map(|m| m.template().to_string())
    }))
    .await?;
app.shutdown(ShutdownReason::Signal, Some(grace));
```

`ObservePlugin` is built from an `Observer`, not from a config. The
application needs the observer before the plugins exist: the job queue takes
`observer.job_events()` when it is built, and the server takes
`observer.wrap(..)`.

## Logs

A `tracing-subscriber` layer of our own, in `src/logs.rs`. The stock JSON
format names its fields `timestamp` and `fields.message` and cannot add a
trace id, and it pulls `serde_json`. Ours is two hundred lines and no
`serde`.

```json
{"ts":"2026-10-30T12:00:00.123Z","level":"info","msg":"GET /users/7 -> 200","target":"brocanth::request","request_id":"9f0c…","method":"GET","path":"/users/7","route":"/users/{id}","status":200,"duration_ms":1.234}
```

With `otel` and a sampled span, the line also has `trace_id` and `span_id`.

```text
2026-10-30T12:00:00.123Z  INFO GET /users/7 -> 200 request_id=9f0c… method=GET path=/users/7 route=/users/{id} status=200 duration_ms=1.234
```

- **Format.** `json` or `text`. Unset: `json` when the environment is
  `production`, `text` elsewhere (`LogFormat::resolve`).
- **Level.** `info`, or per target: `info,sqlx=warn`. `RUST_LOG` wins.
- **Joined lines.** The request span carries `request_id`, `method`,
  `path` and `route`. A `tracing::info!` in a handler is written with them.
- **Status.** `info` below 500, `error` from 500.
- **Probes are quiet.** `/up`, `/readyz` and `/metrics` are not logged
  unless `log_probes` is on. A probe every few seconds buries real traffic.

### Request id

The observer reads `X-Request-Id` (the header is configurable). It keeps a
value of visible ASCII, at most 200 bytes. Anything else, or nothing, gets
a new 32-hex id. The handler sees the id in its request headers, and the
response echoes it.

## Probes

**`/up`** answers `200 {"status":"ok"}` and asks nothing. A liveness probe
that pings the database restarts every pod when the database blinks.

**`/readyz`** runs every readiness check at once, each under
`check_timeout` (2 s), each as its own task. A check that panics is a
failed check, not a failed probe.

```json
{"status":"unready","checks":[{"name":"jobs","status":"ok","latency_ms":0.8},{"name":"cache","status":"fail","latency_ms":2000,"error":"timed out after 2000 ms"}]}
```

`200` when every check passes. `503` otherwise, or `{"status":"draining"}`
once shutdown began.

### The readiness seam is in the core

`brocanth::plugin_health`. A plugin registers one check:

```rust
fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
    let queue = self.queue.clone();
    lc.ready_when(move || {
        let queue = queue.clone();
        async move { queue.inspect(JobId(0)).await.map(drop).map_err(|e| e.to_string()) }
    });
    Ok(())
}
```

The core keeps the list and one bit. It never runs a check, because it has
no runtime. A check returns the same `BoxFuture` an async route returns.
The seam is in the core because a jobs plugin must not name an
observability crate, and an observability crate cannot name every plugin.
The application adds its own check through `Setup::readiness()` or
`App::readiness().add(..)`.

Checks today:

| Plugin | Check |
| --- | --- |
| `JobsPlugin` | a primary-key lookup in the jobs table (`stats` would count every row on every probe) |
| `CachePlugin` | one `get` of a probe key; a miss is a pass |

### Shutdown order (ADR 2)

A load balancer routes to a pod until its readiness probe fails. So
readiness fails **first**:

1. SIGTERM. `Observer::stop_on_signal` calls `Readiness::drain`.
2. The server keeps serving for `drain_delay`. `/readyz` answers 503.
   Kubernetes removes the pod from its endpoints.
3. `Stopper::stop`. The listener closes and in-flight requests finish.
4. `App::shutdown` calls `drain` again (harmless), then `begin_shutdown`
   and `shutdown` for every plugin.

A stop that skips the observer still reports draining before any plugin
drains, because `App::shutdown` flips the bit before `begin_shutdown`.

## Metrics

`prometheus-client`, over `metrics` + `metrics-exporter-prometheus`. It adds
four small crates (dtoa, itoa, parking_lot, a derive). The exporter brings
its own hyper server, `quanta` and a process-global recorder. A registry
that belongs to one `Observer` also keeps ADR 1: two applications in one
process do not count each other's requests.

| Series | Kind | Labels |
| --- | --- | --- |
| `http_requests_total` | counter | `method`, `route`, `status` |
| `http_request_duration_seconds` | histogram, 5 ms to 10 s | `method`, `route`, `status` |
| `http_requests_in_flight` | gauge | — |
| `brocanth_jobs_runs_total` | counter | `job`, `outcome` (`completed`, `retrying`, `dead`) |
| `brocanth_jobs_run_duration_seconds` | histogram | `job` |
| `brocanth_jobs_ready`, `_retrying`, `_scheduled`, `_running` | gauge | `queue` |
| `brocanth_jobs_dead` | gauge | — |
| `brocanth_jobs_oldest_ready_age_seconds` | gauge | — |

- **`route` is the template**, `/users/{id}`, never the raw path. One label
  value per user id takes a Prometheus server down. No match: `unmatched`.
- **`status` is the class**, `2xx`, for the same reason.
- **A plugin adds series** with `observer.metrics().register(name, help,
  metric)`, and slow values with `on_scrape(async refresh)`. The refresh
  runs when Prometheus asks, under the check timeout.
- **The queue gauges** are registered by `ObservePlugin::after_config`
  when a `brocanth_jobs::Queue` is in the services. They read `Queue::stats`
  at scrape time.
- **Auth.** `metrics_token` requires `Authorization: Bearer <token>`.
- **Bind.** `metrics_bind` moves `/metrics` off the main listener. Serve it
  with `observer.serve_admin(Server::bind(addr).await?)`, which also answers
  the probes.

The body is OpenMetrics text (`application/openmetrics-text`), which
Prometheus reads natively.

## Traces (`otel`)

`tracing` spans, turned into OpenTelemetry spans by `tracing-opentelemetry`,
exported over OTLP/HTTP with protobuf (port 4318). Not gRPC: `tonic` would
double the tree.

- **Request span.** Name `GET /users/{id}`, kind `server`, status `ERROR`
  for a 5xx. A span the handler or `sqlx` opens inside is its child.
- **Propagation.** An incoming `traceparent` is the parent. The response
  carries the request span's own `traceparent`.
- **Job span.** `JobTelemetry` opens a root span on `Started` and closes it
  on `Completed`, `Retrying` or `DeadLettered`. Kind `consumer`.
- **Config.** `tracing.enabled`, `tracing.endpoint`, `tracing.sample_ratio`
  (parent-based, so a sampled caller is always kept). The service name is
  `ObserveConfig::service`.
- **Environment wins.** `OTEL_EXPORTER_OTLP_ENDPOINT`,
  `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`, `OTEL_SERVICE_NAME`,
  `OTEL_RESOURCE_ATTRIBUTES`, `OTEL_TRACES_SAMPLER`,
  `OTEL_TRACES_SAMPLER_ARG`. An operator changes a deployment without a
  rebuild.
- **Flush.** `ObservePlugin::shutdown` shuts the provider down, which
  exports what is buffered.
- **Tests** use `Observer::with_tracer_provider` and the SDK's
  `InMemorySpanExporter`.

## Config, in a generated project

`brocanth_observe::scaffold::Observe` writes `src/plugins/observe/mod.rs` and
this section:

```toml
[observe]
# format = "json"        # unset: json in production
level = "info"
live_path = "/up"
ready_path = "/readyz"
metrics_path = "/metrics"
# metrics_token = "a long random string"
# metrics_bind = "127.0.0.1:9100"
drain_delay_secs = 0
```

An empty path turns that endpoint off.

## Not done yet

- **The CLI does not offer `observe` yet**, and the generated `serve` does
  not wrap its handler, listen for SIGTERM or call `App::shutdown`. The lines
  it needs are above, under "Where it attaches".
- **The `logging` module is not retired.** It still writes plain lines to
  stderr. `observe` replaces it; the two should not both be chosen.
- **No database readiness check.** `brocanth-model`'s plugin opens no pool,
  so there is nothing to ping. The same gap blocks pool stats.
- **The generated cache and jobs plugins** (`brocanth-cache`'s and
  `brocanth-jobs`' scaffold files) do not call `ready_when`. Only the library
  plugins do.
- **A job's `perform` does not run inside its span**, and the enqueuing
  request's trace does not reach the job. Both need `brocanth-jobs` to carry
  a trace context, which it deliberately does not link.
- **Logs are not exported over OTLP.** stdout is the contract; a collector
  tails it.
