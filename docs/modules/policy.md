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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-policy` |
| Without it | no rule algebra; authorisation is whatever each handler remembered to check |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/policy.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`realtime`](realtime.md): the docs say where a rule joins a broadcast key |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/plugins/policy/mod.rs`: the `Visitor` subject
  (`#[derive(Subject)]`, two `#[attr]` fields), and the example
  `PostPolicy`, a `#[db]` policy. An author edits a draft and may publish
  it: `update` admits a draft row, and `update_check` admits the row the
  update leaves, a draft or a published post. With [`rls`](rls.md),
  `schema()` writes these rules as database policies.

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
