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
| Needs | none |
| After | none |
| Adapts to | [`rsx`](rsx.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-input`, `zod` |
| Without it | no entrance; `handle_post` reads `ctx.body` and parses it by hand |

## What it writes

Slots:

- src/pages/mod.rs
- AGENTS.md: `## Reading a body`

## Settings

None.

## Read more

- [The entrance](../input.md)
- Source: [`crates/brocanth-input/src/scaffold.rs`](../../crates/brocanth-input/src/scaffold.rs)
