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
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-command` |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/commands.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`input`](input.md): `routes` prints the body each route takes |
| Contributes | `AgentsSection` `commands`: how a command is added |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

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
