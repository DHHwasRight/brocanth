# `jt`

`.jt` templates compiled into the binary; a JSON answer without a struct per shape.

The wizard calls it: *JSON from templates, compiled into the binary*.

| | |
|---|---|
| Categories | **Renderer** |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `jt` |
| Without it | no JSON templates; a page that answers JSON builds the String it returns, or derives a struct for the shape |

## What it writes

Files:

- `src/pages/status.jt`
- `src/pages/status.rs`

Slots:

- src/pages/mod.rs (the renderers)
- src/pages/mod.rs (the route table test): `/status`

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- [What a module is](../modules.md)
- Source: [`crates/brocanth-cli/src/modules/jt.rs`](../../crates/brocanth-cli/src/modules/jt.rs)
