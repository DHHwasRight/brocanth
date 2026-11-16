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

- `src/commands/serve.rs`
- `src/commands/routes.rs`

Slots:

- src/commands/mod.rs: `pub mod serve;`, `pub mod routes;`
- AGENTS.md: the `Tasks` section, on how to add a command

With `commands`, `cargo run` alone prints the help: run `cargo run -- serve`
or `brocanth serve`.

## Who owns what

The module owns every file and section that only a project with `commands`
has: `serve.rs`, `routes.rs` and the AGENTS.md section. `create.rs` keeps
six things. The move stops here, on purpose.

| File | Owner | Why |
|---|---|---|
| `src/main.rs` | `create.rs` | Every project has one. `commands` picks one of two whole shapes (a command line, or `serve` only), not a piece to add. |
| `src/commands/mod.rs` | `create.rs` | It lists the `CommandsMod` lines of every module, so only the finished build can write it. |
| `src/commands/settings.rs` | `create.rs` | It exists when any module declares a setting, a fact of the finished build. |
| `mise.toml`, task `serve` | `create.rs` | Every project has the task. `commands` changes its `run` line (`cargo run -- serve` or `cargo run`). |
| the next steps `create` prints | `create.rs` | They are the CLI's output, not a file of the project. |
| `brocanth.toml` | `create.rs` | The marker every project has, with or without `commands`. |

A slot cannot carry any of these: each one is a choice between two whole
texts, or needs every module's contribution first. Moving them would mean a
module that rewrites the files of the others.

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- Source: [`crates/brocanth-cli/src/modules/commands.rs`](../../crates/brocanth-cli/src/modules/commands.rs)
