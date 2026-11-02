# `docker`

A cargo-chef Dockerfile: slim Debian, a user that is not root, a HEALTHCHECK on /up.

The wizard calls it: *A container image*.

| | |
|---|---|
| Categories | **External** |
| Wizard question | Operations |
| Default | on in a non-interactive `create` |
| Presets | HTML application, JSON API, Something that does work |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |

## What it writes

Files:

- `.dockerignore`
- `Dockerfile`

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- Source: [`crates/brocanth-cli/src/modules/docker.rs`](../../crates/brocanth-cli/src/modules/docker.rs)
