# `openapi`

An OpenAPI 3.1 document derived from src/pages, served at /openapi.json.

The wizard calls it: *An OpenAPI document, derived from your routes*.

| | |
|---|---|
| Categories | **Routes**, Enhancer |
| Wizard question | Doors |
| Default | off |
| Presets | JSON API |
| Part of | nothing |
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md), [`input`](input.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-openapi` (features `zod`) |
| Without it | no document; a client learns this API by reading the handlers |

## What it writes

Files:

- `src/pages/openapi.json.rs`
- `src/pages/openapi.yaml.rs`
- `src/plugins/openapi/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod openapi;`
- src/plugins/mod.rs (the provider list): `openapi crate::plugins::openapi::OpenApi`
- src/pages/mod.rs (the route table test): `/openapi.json` (2 entries)
- config/<env>.toml: `[openapi]`

## Settings

Section `[openapi]`. Environment variable: `$<APP>_OPENAPI__<KEY>`. Flag: `--openapi.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `title` | text | `"<Title>"` | `--openapi.title` | The name of the API. |
| `version` | text | `"0.1.0"` | `--openapi.version` | The version of the contract. |

## Read more

- [The surface](../surface.md)
- [brocanth-openapi](../../crates/brocanth-openapi/README.md)
- Source: [`crates/brocanth-openapi/src/scaffold.rs`](../../crates/brocanth-openapi/src/scaffold.rs)
