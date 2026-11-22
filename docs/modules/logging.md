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
| Conflicts | [`observability-logs`](observability-logs.md): both write a line per request; `observability-logs` writes JSON on stdout; [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/logging.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

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
| Hooks | `line` (Identify, required): write one line per response on stderr |
| Options (build time) | none |
| Settings (run time) | `[logging]`: `level`, `format` |
<!-- end generated: capabilities -->

Hooks: `logging.line`, required: it is the module.

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

`format` picks the shape of a line:

| `format` | A line |
|---|---|
| `line` (default) | `GET /a -> 200` |
| `common` | Apache Common Log Format: `203.0.113.9 - - [10/Oct/2000:20:55:36 +0000] "GET /a?b=1 HTTP/1.1" 200 5` |
| `combined` | `common`, then `"<Referer>" "<User-Agent>"` |

The time is UTC. The host is the socket's peer; behind a proxy, read
`brocanth_middleware::ClientInfo` from the request state in
`Line::after_dispatch`. The byte count is `-` for an empty body and for a
stream. A quote, a backslash and a byte outside printable ASCII are
escaped as Apache escapes them, so a client cannot forge a line. For a
shape of your own, change `Line::after_dispatch`: the file is yours.

## Settings

Section `[logging]`. Environment variable: `$<APP>_LOGGING__<KEY>`. Flag: `--logging.<key>`. Rename or hide a flag in `src/modules/logging.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `level` | text | `"debug"` | `--logging.level` | The least severe line written: error, warn, info or debug. |
| `format` | text | `"line"` | `--logging.format` | The shape of a line: line, common or combined. |

## Read more

- [Observability](../observability.md)
- [Middleware](../middleware.md)
- Source: [`crates/brocanth-cli/src/modules/logging.rs`](../../crates/brocanth-cli/src/modules/logging.rs)
