# `observability-tracing`

An OTLP span per request and per job, W3C traceparent in and out; needs a collector.

The wizard calls it: *Traces over OTLP (needs a collector)*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Operations |
| Default | off |
| Presets | none |
| Part of | [`observability`](observability.md), off until you tick it |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-observe` (features `otel`; no default features) |

## What it writes

No file and no slot of its own.

The parent writes all the code. This child turns on a cargo feature and adds keys to `[observability]`.

## Settings

The parent declares these keys in `[observability]` when this child is on.

Section `[observability]`. Environment variable: `$<APP>_OBSERVABILITY__<KEY>`. Flag: `--observability.<key>`. Rename or hide a flag in `src/modules/observability_tracing.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `otlp_endpoint` | text | `"http://localhost:4318"` (commented out) | `--observability.otlp_endpoint` | The collector. Empty: OTEL_EXPORTER_OTLP_ENDPOINT, then localhost:4318. |
| `sample_ratio` | float | `1.0` | `--observability.sample_ratio` | The share of new traces kept, from 0.0 to 1.0. |

## Read more

- [Observability](../observability.md)
- Source: [`crates/brocanth-observe/src/scaffold.rs`](../../crates/brocanth-observe/src/scaffold.rs)
