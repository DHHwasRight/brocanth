# `errors`

HTML 404 and 500 pages in your layouts, the cause in a debug build, a reporter hook.

The wizard calls it: *HTML pages for a missing page and a failure*.

| | |
|---|---|
| Categories | **Renderer**, Middleware |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | [`rsx`](rsx.md) |
| Adapts to | [`config`](config.md), [`rsx`](rsx.md), [`i18n-routing`](i18n-routing.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-errors` (feature `config` with [`config`](config.md)) |
| Without it | a missing page answers a plain `Not Found`, a failure a plain `Internal Server Error` |

## What it writes

Files:

- `src/plugins/errors/mod.rs`: `catching`, which wraps every page, and
  `caught`, which turns a failure into the nearest error page. Its tests
  ask for a missing path with HTML and with JSON, and check that a 500
  reaches a reporter and shows its cause only in a debug build.
- `src/pages/not-found.rsx` and `src/pages/error.rsx`, with
  [`rsx`](rsx.md).

Slots:

- src/plugins/mod.rs: `pub mod errors;`
- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `errors brocanth_errors::Errors`
- src/plugins/mod.rs (the hook record): `brocanth_errors::Errors`
- src/pages/mod.rs: `pub use crate::plugins::errors::{..}`
- config/<env>.toml: `[errors]`
- AGENTS.md: `## Errors`

The generator writes two more lines, because both files are its own: the
`errors` word in `pages!("src/pages", errors)`, and the 404 arm of
`dispatch` in `src/app.rs`, which calls `pages::not_found`.

`not-found.rsx` answers a 404 for every page in its directory and below
it, and for a path under it that no route matched. `error.rsx` answers
every other failure. The nearest file wins, inside the layouts above it.
A client that does not accept HTML, and an error that names its own
`content-type`, keep the plain answer.

A 5xx is logged and told to every `brocanth_errors::Reporter`. A plugin
adds its reporter to the `Reporters` service in `after_config`. The
`report` hook (on) tells the reporters about a hook's failure too; turn
it off in `src/modules/errors.rs`.

With [`i18n-routing`](i18n-routing.md), its tests ask under `/en`, which
the redirect leaves alone.

## Settings

Section `[errors]`. Environment variable: `$<APP>_ERRORS__<KEY>`. Flag: `--errors.<key>`. Rename or hide a flag in `src/modules/errors.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `details` | bool | `true` | `--errors.details` | Show the cause of a 500 in a debug build. Turn it off to see `error.rsx` while you design it. |

**Decision: a release build never shows the cause.** The check is
`cfg!(debug_assertions)`, not the setting alone, so a wrong value in a
production file cannot leak a stack of internals.

**Decision: the development page is outside the layouts.** The layout can
be what failed.

## Read more

- [`brocanth-errors`](../../crates/brocanth-errors/README.md)
- Source: [`crates/brocanth-errors/src/scaffold.rs`](../../crates/brocanth-errors/src/scaffold.rs)
