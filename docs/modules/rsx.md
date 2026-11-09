# `rsx`

`.rsx` templates compiled into the binary; without it a page builds a String.

The wizard calls it: *Templates compiled into the binary*.

| | |
|---|---|
| Categories | **Renderer** |
| Wizard question | Pages |
| Default | off |
| Presets | HTML application, Static site |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | [`rsx-tailwind`](rsx-tailwind.md), [`rsx-htmx`](rsx-htmx.md), [`flash`](flash.md) |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `rsx` |
| Without it | no template engine; a page builds the String it returns |

## What it writes

Files:

- `src/pages/about.rsx`
- `src/pages/index.rsx`
- `src/pages/layout.rsx`
- `src/pages/users/[id].rsx`
- `src/pages/users/layout.rsx`

Slots:

- src/pages/mod.rs (the renderers)

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- Source: [`crates/brocanth-cli/src/modules/rsx.rs`](../../crates/brocanth-cli/src/modules/rsx.rs)
