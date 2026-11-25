# `debug`

development tools, refused by `check --deploy`; pick them below.

The wizard calls it: *Development tools*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Operations |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/debug.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

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

Nothing. It is the line in the chooser where the development tools sit,
as [`ci`](ci.md) is where the CI providers sit. Its child today is
[`debug-toolbar`](debug-toolbar.md).

**Development only.** `brocanth check --deploy` reports it as
`deploy/development-only` and prints why. Remove it before a deploy:
`brocanth remove debug-toolbar debug`.

## Settings

None.

## Read more

- [`debug-toolbar`](debug-toolbar.md)
- Source: [`crates/brocanth-debug/src/scaffold.rs`](../../crates/brocanth-debug/src/scaffold.rs)
