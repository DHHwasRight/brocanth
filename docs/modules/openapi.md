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
| Adapts to | [`config`](config.md), [`input`](input.md), [`auth-oauth`](auth-oauth.md), [`oauth-client`](oauth-client.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-openapi` (feature `zod`; `admin` with that module) |
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
- the admin panels, with `admin`: `brocanth_openapi::admin::ApiPanel`

The document hides its own two routes. It also hides the routes that
`admin`, `auth-oauth` and `oauth-client` mount, read from their module
files: the document is public, and those routes are not this API.

## Choices

| Choice | Default | What it does |
|---|---|---|
| integration `admin` | on | A read-only tab at `/admin/api` with every operation. Without `admin` it does nothing. |

## Settings

Section `[openapi]`. Environment variable: `$<APP>_OPENAPI__<KEY>`. Flag: `--openapi.<key>`. Rename or hide a flag in `src/modules/openapi.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `title` | text | `"<Title>"` | `--openapi.title` | The name of the API. |
| `version` | text | `"0.1.0"` | `--openapi.version` | The version of the contract. |

## Read more

- [The surface](../surface.md)
- [brocanth-openapi](../../crates/brocanth-openapi/README.md)
- Source: [`crates/brocanth-openapi/src/scaffold.rs`](../../crates/brocanth-openapi/src/scaffold.rs)
