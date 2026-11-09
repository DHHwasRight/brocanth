# `logging`

A line per request; the core assumes no log layer.

The wizard calls it: *A line per request*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Foundations |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | [`config`](config.md) |
| Conflicts | [`observability-logs`](observability-logs.md): both write a line per request; `observability-logs` writes JSON on stdout; [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |

## What it writes

Files:

- `src/plugins/logging/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain): `plugin_hooking`, because the plugin installs a request hook
- src/plugins/mod.rs: `pub mod logging;`
- src/plugins/mod.rs (the provider list): `logging crate::plugins::logging::Logging`
- src/plugins/mod.rs (the hook record): `crate::plugins::logging::Logging`
- config/<env>.toml: `[logging]`

The line is written by the request hook's `after_dispatch`, so every
response that reaches `handle` is logged: a page, a 404, a 405, a CORS
preflight, and a 401 or a 429 that another request hook answered. Static
files are not logged. It is plain text on stderr, through `plugins::say!`
for its startup line.

## Settings

Section `[logging]`. Environment variable: `$<APP>_LOGGING__<KEY>`. Flag: `--logging.<key>`. Rename or hide a flag in `src/modules/logging.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `level` | text | `"debug"` | `--logging.level` | The least severe line written: error, warn, info or debug. |

## Read more

- [Observability](../observability.md)
- [Middleware](../middleware.md)
- Source: [`crates/brocanth-cli/src/modules/logging.rs`](../../crates/brocanth-cli/src/modules/logging.rs)
