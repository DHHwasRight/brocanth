# `ci-github`

.github/workflows/ci.yml: fmt, clippy, tests, a Postgres service when there is a database, the image.

The wizard calls it: *GitHub Actions*.

| | |
|---|---|
| Categories | **External** |
| Wizard question | Operations |
| Default | on in a non-interactive `create` |
| Presets | HTML application, JSON API, Something that does work, Static site |
| Part of | [`ci`](ci.md), ticked with its parent |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | none |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/ci_github.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | `PgPool` (a Postgres pool): CI starts a database for the tests |
| Adapts to | [`static`](static.md): CI keeps the exported site as an artifact; [`docker`](docker.md): CI builds the image |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `.github/workflows/ci.yml`

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- Source: [`crates/brocanth-cli/src/modules/ci.rs`](../../crates/brocanth-cli/src/modules/ci.rs)
