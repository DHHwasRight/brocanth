# `commands`

This project's own command line, `serve` plus whatever you add.

The wizard calls it: *Your own command line: `serve`, and whatever you add*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Foundations |
| Default | on in a non-interactive `create` |
| Presets | HTML application, JSON API, Something that does work, Static site |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | [`input`](input.md) |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-command` |

## What it writes

Files:

- `src/commands/routes.rs`

Slots:

- src/commands/mod.rs: `pub mod routes;`

`create.rs` writes `src/commands/mod.rs`, `serve.rs` and `settings.rs`. The module adds the `routes` command. With `commands`, `cargo run` alone prints the help: run `cargo run -- serve` or `brocanth serve`.

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- Source: [`crates/brocanth-cli/src/modules/commands.rs`](../../crates/brocanth-cli/src/modules/commands.rs)
