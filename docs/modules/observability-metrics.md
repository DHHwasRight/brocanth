# `observability-metrics`

Prometheus text on /metrics: requests by route and status, in-flight, job queues.

The wizard calls it: *Prometheus metrics*.

| | |
|---|---|
| Categories | **Routes** |
| Wizard question | Operations |
| Default | on in a non-interactive `create` |
| Presets | HTML application, JSON API, Something that does work |
| Part of | [`observability`](observability.md), ticked with its parent |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-observe` (features `metrics`; no default features) |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/observability_metrics.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

No file and no slot of its own.

The parent writes all the code. This child turns on a cargo feature and adds keys to `[observability]`.

## Settings

The parent declares these keys in `[observability]` when this child is on.

Section `[observability]`. Environment variable: `$<APP>_OBSERVABILITY__<KEY>`. Flag: `--observability.<key>`. Rename or hide a flag in `src/modules/observability_metrics.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `metrics_path` | text | `"/metrics"` | `--observability.metrics_path` | Prometheus text. Empty turns it off. |
| `metrics_token` | text | `"a long random string"` (commented out) | none (secret) | When set, /metrics needs `Authorization: Bearer <token>`. |
| `metrics_bind` | text | `"127.0.0.1:9100"` (commented out) | `--observability.metrics_bind` | When set, /metrics is served on this address, not the main listener. |

## Read more

- [Observability](../observability.md)
- Source: [`crates/brocanth-observe/src/scaffold.rs`](../../crates/brocanth-observe/src/scaffold.rs)
