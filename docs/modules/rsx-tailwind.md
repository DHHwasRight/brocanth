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
| Needs | [`commands`](commands.md): the CSS build is `brocanth css`, a subcommand, and a project with no command line has nowhere to put it |
| After | [`commands`](commands.md) |
| Adapts to | [`rsx-htmx`](rsx-htmx.md) |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | none |

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
