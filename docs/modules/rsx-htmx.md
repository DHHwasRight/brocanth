# `rsx-htmx`

A click swaps what changed; the server renders from the shared layout down.

The wizard calls it: *A click swaps what changed, not the whole page*.

| | |
|---|---|
| Categories | **Renderer** |
| Wizard question | Pages |
| Default | off |
| Presets | HTML application, Static site |
| Part of | [`rsx`](rsx.md), ticked with its parent |
| Needs | none |
| After | none |
| Adapts to | [`rsx-tailwind`](rsx-tailwind.md) |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | none |

## What it writes

Files:

- `public/dpswap.min.js`
- `src/assets.rs`
- `src/pages/link.rsx`

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- Source: [`crates/brocanth-cli/src/modules/rsx.rs`](../../crates/brocanth-cli/src/modules/rsx.rs)
