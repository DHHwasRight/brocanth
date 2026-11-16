# `static`

`export` writes every GET page and every asset to dist/; no server, so no runtime modules.

The wizard calls it: *Compile the pages to files any static host serves*.

| | |
|---|---|
| Categories | **External**, Enhancer |
| Wizard question | Pages |
| Default | off |
| Presets | Static site |
| Part of | nothing |
| Conflicts | every module that needs a running server (all modules without *outside the runtime*) |
| Outside the runtime | yes |
| Crates | `brocanth-static` |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/static.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`commands`](commands.md): `export` is a subcommand of this binary |
| After | [`commands`](commands.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`rsx-tailwind`](rsx-tailwind.md): the export writes the compiled stylesheet; [`rsx-htmx`](rsx-htmx.md): the export writes dp-swap and the partials |
| Contributes | `AgentsSection` `static`: the project builds files, not a server |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/commands/export.rs`

Slots:

- src/commands/mod.rs: `pub mod export;`
- .gitignore: `/dist`
- mise.toml: `[tasks.export]`
- AGENTS.md: `## Static site`

## Settings

None.

## Read more

- [Static export](../static-export.md)
- Source: [`crates/brocanth-cli/src/modules/static_site.rs`](../../crates/brocanth-cli/src/modules/static_site.rs)
