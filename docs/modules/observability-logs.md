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
| Conflicts | [`logging`](logging.md): both write a line per request; `observability-logs` writes JSON on stdout; [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-observe` (features `logs`; no default features) |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/observability_logs.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

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

Section `[observability]`. Environment variable: `$<APP>_OBSERVABILITY__<KEY>`. Flag: `--observability.<key>`. Rename or hide a flag in `src/modules/observability_logs.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `format` | text | `"json"` (commented out) | `--observability.format` | json or text. Empty: json in production, text elsewhere. |
| `level` | text | `"info"` | `--observability.level` | A level, or per-target levels: info,sqlx=warn. RUST_LOG wins when set. |

## Read more

- [Observability](../observability.md)
- Source: [`crates/brocanth-observe/src/scaffold.rs`](../../crates/brocanth-observe/src/scaffold.rs)
