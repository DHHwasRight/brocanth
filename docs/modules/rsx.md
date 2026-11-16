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
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `rsx` |
| Without it | no template engine; a page builds the String it returns |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/rsx.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | [`observability`](observability.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`rsx-tailwind`](rsx-tailwind.md): the layout links the stylesheet; [`rsx-htmx`](rsx-htmx.md): the layout loads dp-swap and marks its depth; [`meta`](meta.md): the root layout declares `meta: &Head` and writes the page's title, language and tags; [`flash`](flash.md): the layout carries `<div id="flash">`, where the messages go |
| Contributes | `AgentsSection` `rsx`: how views, layouts and static files are written |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/pages/about.rsx`
- `src/pages/index.rsx`
- `src/pages/layout.rsx`. With [`meta`](meta.md), it declares `meta: &Head` and writes the page's title, language and tags.
- `src/pages/users/[id].rsx`
- `src/pages/users/layout.rsx`

Slots:

- src/pages/mod.rs (the renderers)

## Settings

None.

## Read more

- [Getting started](../getting-started.md)
- Source: [`crates/brocanth-cli/src/modules/rsx.rs`](../../crates/brocanth-cli/src/modules/rsx.rs)
