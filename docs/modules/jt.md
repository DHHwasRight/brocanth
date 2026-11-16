# `jt`

`.jt` templates compiled into the binary; a JSON answer without a struct per shape.

The wizard calls it: *JSON from templates, compiled into the binary*.

| | |
|---|---|
| Categories | **Renderer**, Routes |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `jt` |
| Without it | no JSON templates; a page that answers JSON builds the String it returns, or derives a struct for the shape |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/jt.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

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
