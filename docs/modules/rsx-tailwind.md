# `rsx-tailwind`

A CSS build over the class names in your templates; needs node.

The wizard calls it: *Tailwind, built from the class names in your templates*.

| | |
|---|---|
| Categories | **External**, Enhancer |
| Wizard question | Pages |
| Default | off |
| Presets | HTML application, Static site |
| Part of | [`rsx`](rsx.md), ticked with its parent |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | none |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/rsx_tailwind.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`commands`](commands.md): the CSS build is `brocanth css`, a subcommand, and a project with no command line has nowhere to put it |
| After | [`commands`](commands.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `rsx-tailwind`: where the CSS comes from, and how it is built |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `assets/css/_tailwind.css`
- `assets/css/site.css`
- `package.json`
- `src/assets.rs`
- `src/commands/css.rs`

Slots:

- src/commands/mod.rs: `pub mod css;`
- .gitignore: `/node_modules`
- mise.toml: `[tasks.css]`

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- Source: [`crates/brocanth-cli/src/modules/rsx.rs`](../../crates/brocanth-cli/src/modules/rsx.rs)
