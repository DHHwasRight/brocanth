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
| Needs | none |
| After | none |
| Adapts to | [`model`](model.md), [`cache`](cache.md), [`jobs`](jobs.md), [`static`](static.md), [`docker`](docker.md) |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | none |

## What it writes

Files:

- `.github/workflows/ci.yml`

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- Source: [`crates/brocanth-cli/src/modules/ci.rs`](../../crates/brocanth-cli/src/modules/ci.rs)
