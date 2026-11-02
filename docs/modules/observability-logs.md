# `observability-logs`

A JSON line per request on stdout in production, text elsewhere; `tracing` and nothing else.

The wizard calls it: *Logs on stdout*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Operations |
| Default | on in a non-interactive `create` |
| Presets | HTML application, JSON API, Something that does work |
| Part of | [`observability`](observability.md), ticked with its parent |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`logging`](logging.md): both write a line per request; [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-observe` (features `logs`; no default features) |

## What it writes

No file and no slot of its own.

The parent writes all the code. This child turns on a cargo feature and adds keys to `[observability]`.

## Settings

The parent declares these keys in `[observability]` when this child is on.

Section `[observability]`. Environment variable: `$<APP>_OBSERVABILITY__<KEY>`. Flag: `--observability.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `format` | text | `"json"` (commented out) | `--observability.format` | json or text. Empty: json in production, text elsewhere. |
| `level` | text | `"info"` | `--observability.level` | A level, or per-target levels: info,sqlx=warn. RUST_LOG wins when set. |

## Read more

- [Observability](../observability.md)
- Source: [`crates/brocanth-observe/src/scaffold.rs`](../../crates/brocanth-observe/src/scaffold.rs)
