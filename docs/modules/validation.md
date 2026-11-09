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
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-schema` |
| Without it | no schema vocabulary; a body is checked by hand, or it is not checked |

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
