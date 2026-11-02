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
| Needs | [`commands`](commands.md): `export` is a subcommand of this binary |
| After | [`commands`](commands.md) |
| Adapts to | [`rsx-tailwind`](rsx-tailwind.md), [`rsx-htmx`](rsx-htmx.md) |
| Conflicts | every module that needs a running server (all modules without *outside the runtime*) |
| Outside the runtime | yes |
| Crates | `brocanth-static` |

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
