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
| Conflicts | [`observability-logs`](observability-logs.md): both write a line per request; [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |

## What it writes

Files:

- `src/plugins/logging/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/app.rs (setup): `setup.router().mount("/{*rest}", "request_log");`
- src/plugins/mod.rs: `pub mod logging;`
- src/plugins/mod.rs (the provider list): `logging crate::plugins::logging::Logging`
- src/plugins/mod.rs (layer): `"request_log" => Some(logging::layer),`
- config/<env>.toml: `[logging]`

## Settings

Section `[logging]`. Environment variable: `$<APP>_LOGGING__<KEY>`. Flag: `--logging.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `level` | text | `"debug"` | `--logging.level` | The least severe line written: error, warn, info or debug. |

## Read more

- [Observability](../observability.md)
- [Middleware](../middleware.md)
- Source: [`crates/brocanth-cli/src/modules/logging.rs`](../../crates/brocanth-cli/src/modules/logging.rs)
