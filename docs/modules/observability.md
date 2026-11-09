# `observability`

A request id on every request and a SIGTERM that fails /readyz first; logs, probes, metrics and traces below.

The wizard calls it: *Observability: logs, health probes, metrics (traces off)*.

| | |
|---|---|
| Categories | **Middleware**, Enhancer, Routes |
| Wizard question | Operations |
| Default | on in a non-interactive `create` |
| Presets | HTML application, JSON API, Something that does work |
| Part of | nothing |
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md), [`observability-logs`](observability-logs.md), [`observability-health`](observability-health.md), [`observability-metrics`](observability-metrics.md), [`observability-tracing`](observability-tracing.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-observe` (no default features) |
| Without it | no request log, no /up or /readyz, no /metrics; a deploy cannot tell ready from alive |

## What it writes

Files:

- `src/plugins/observability/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod observability;`
- src/plugins/mod.rs (the provider list): `observability crate::plugins::observability::Observe`
- config/<env>.toml: `[observability]`

## Settings

Section `[observability]`. Environment variable: `$<APP>_OBSERVABILITY__<KEY>`. Flag: `--observability.<key>`. Rename or hide a flag in `src/modules/observability.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `format` | text | `"json"` (commented out) | `--observability.format` | json or text. Empty: json in production, text elsewhere. |
| `level` | text | `"info"` | `--observability.level` | A level, or per-target levels: info,sqlx=warn. RUST_LOG wins when set. |
| `live_path` | text | `"/up"` | `--observability.live_path` | Liveness. Empty turns it off. |
| `ready_path` | text | `"/readyz"` | `--observability.ready_path` | Readiness: every plugin's check. Empty turns it off. |
| `drain_delay_secs` | integer | `0` | `--observability.drain_delay_secs` | Seconds to answer "not ready" after SIGTERM, before the listener stops. |
| `metrics_path` | text | `"/metrics"` | `--observability.metrics_path` | Prometheus text. Empty turns it off. |
| `metrics_token` | text | `"a long random string"` (commented out) | none (secret) | When set, /metrics needs `Authorization: Bearer <token>`. |
| `metrics_bind` | text | `"127.0.0.1:9100"` (commented out) | `--observability.metrics_bind` | When set, /metrics is served on this address, not the main listener. |
| `otlp_endpoint` | text | `"http://localhost:4318"` (commented out) | `--observability.otlp_endpoint` | The collector. Empty: OTEL_EXPORTER_OTLP_ENDPOINT, then localhost:4318. |
| `sample_ratio` | float | `1.0` | `--observability.sample_ratio` | The share of new traces kept, from 0.0 to 1.0. |

## Read more

- [Observability](../observability.md)
- Source: [`crates/brocanth-observe/src/scaffold.rs`](../../crates/brocanth-observe/src/scaffold.rs)
