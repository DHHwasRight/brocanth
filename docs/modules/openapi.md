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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-openapi` (feature `zod`; `admin` with that module) |
| Without it | no document; a client learns this API by reading the handlers |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/openapi.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | `Mount` (a mounted path): every `Backoffice` mount, such as the admin, is hidden from the document |
| Adapts to | [`input`](input.md): the document lists the declared bodies; [`auth-oauth`](auth-oauth.md): the sign-in routes it mounts are hidden from the document; [`oauth-client`](oauth-client.md): the connection routes it mounts are hidden from the document |
| Contributes | `AdminPanel` `api`, on: an `API` tab in the admin, with every operation |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[openapi]`: `title`, `version` |
<!-- end generated: capabilities -->

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
