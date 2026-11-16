# `observability-health`

/up for liveness, /readyz runs every plugin's check and fails first on SIGTERM.

The wizard calls it: *Health probes*.

| | |
|---|---|
| Categories | **Routes** |
| Wizard question | Operations |
| Default | on in a non-interactive `create` |
| Presets | HTML application, JSON API, Something that does work |
| Part of | [`observability`](observability.md), ticked with its parent |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-observe` (no default features) |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/observability_health.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

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

The parent writes all the code. This child adds keys to `[observability]`. The observer answers `/up` and `/readyz` before the router.

## Settings

The parent declares these keys in `[observability]` when this child is on.

Section `[observability]`. Environment variable: `$<APP>_OBSERVABILITY__<KEY>`. Flag: `--observability.<key>`. Rename or hide a flag in `src/modules/observability_health.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `live_path` | text | `"/up"` | `--observability.live_path` | Liveness. Empty turns it off. |
| `ready_path` | text | `"/readyz"` | `--observability.ready_path` | Readiness: every plugin's check. Empty turns it off. |
| `drain_delay_secs` | integer | `0` | `--observability.drain_delay_secs` | Seconds to answer "not ready" after SIGTERM, before the listener stops. |

## Read more

- [Observability](../observability.md)
- Source: [`crates/brocanth-observe/src/scaffold.rs`](../../crates/brocanth-observe/src/scaffold.rs)
