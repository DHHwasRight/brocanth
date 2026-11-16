# `validation`

A zod-shaped vocabulary; a handler takes Valid<T> or does not compile.

The wizard calls it: *A handler takes `Valid<T>`, or it does not compile*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | JSON API |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-schema` |
| Without it | no schema vocabulary; a body is checked by hand, or it is not checked |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/validation.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

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
| Settings (run time) | `[validation]`: `report` |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/plugins/validation/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the context): `let checks: &brocanth_schema::Validation = cx.extract();`
- src/plugins/mod.rs: `pub mod validation;`
- src/plugins/mod.rs (the provider list): `validation crate::plugins::validation::Validation`
- config/<env>.toml: `[validation]`

## Settings

Section `[validation]`. Environment variable: `$<APP>_VALIDATION__<KEY>`. Flag: `--validation.<key>`. Rename or hide a flag in `src/modules/validation.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `report` | text | `"json"` | `--validation.report` | How a failed check is reported: json or form. |

## Read more

- [The entrance](../input.md)
- [brocanth-schema](../../crates/brocanth-schema/README.md)
- Source: [`crates/brocanth-schema/src/scaffold.rs`](../../crates/brocanth-schema/src/scaffold.rs)
