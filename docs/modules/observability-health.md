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
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-observe` (no default features) |

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
