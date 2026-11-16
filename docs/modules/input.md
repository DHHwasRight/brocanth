# `input`

A form or a JSON body, read into a struct, with every fault at once.

The wizard calls it: *What arrives: a form or a JSON body, as a struct*.

| | |
|---|---|
| Categories | **Enhancer**, Renderer |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-input`, `zod` |
| Without it | no entrance; `handle_post` reads `ctx.body` and parses it by hand |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/input.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`rsx`](rsx.md): a refused form answers HTML, not JSON |
| Contributes | `AgentsSection` `input`: how a page reads a request body |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Slots:

- src/pages/mod.rs
- AGENTS.md: `## Reading a body`

## Settings

None.

## Read more

- [The entrance](../input.md)
- Source: [`crates/brocanth-input/src/scaffold.rs`](../../crates/brocanth-input/src/scaffold.rs)
