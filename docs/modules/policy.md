# `policy`

Typed predicates over an entity's columns; no ORM, no driver, no core.

The wizard calls it: *Authorisation rules that compile into the WHERE clause*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | [`config`](config.md), [`model`](model.md) |
| Adapts to | [`config`](config.md), [`model`](model.md), [`realtime`](realtime.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-policy` |
| Without it | no rule algebra; authorisation is whatever each handler remembered to check |

## What it writes

Files:

- `src/plugins/policy/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the context): `let rules: &crate::plugins::policy::Rules = cx.extract();`
- src/plugins/mod.rs: `pub mod policy;`
- src/plugins/mod.rs (the provider list): `policy crate::plugins::policy::Policies`
- config/<env>.toml: `[policy]`

## Settings

Section `[policy]`. Environment variable: `$<APP>_POLICY__<KEY>`. Flag: `--policy.<key>`. Rename or hide a flag in `src/modules/policy.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `dialect` | text | `"postgres"` | `--policy.dialect` | The SQL a condition compiles to: postgres, the only dialect. |

## Read more

- [Policy](../policy.md)
- Source: [`crates/brocanth-policy/src/scaffold.rs`](../../crates/brocanth-policy/src/scaffold.rs)
