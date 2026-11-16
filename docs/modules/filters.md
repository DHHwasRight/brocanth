# `filters`

Typed filters and a sort from the query string, ANDed with the policy's rule in the WHERE.

The wizard calls it: *Filters and a sort read from the query string*.

| | |
|---|---|
| Categories | **Renderer** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-filters` |
| Without it | no typed filters; a list that filters parses its own query string |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/filters.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`policy`](policy.md): a filter is a `Pred` over a policy entity's columns, ANDed with its list rule |
| After | [`policy`](policy.md) |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`pagination`](pagination.md): the rules name `ctx.links()`, whose page links keep the filters |
| Contributes | `AgentsSection` `filters`: how a list is filtered |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files: none.

Slots:

- src/pages/mod.rs (items): `ctx.filters(&declared)`, which reads the
  query string through a `brocanth_filters::Filters` declaration and
  answers a refused value with a 400 and a `help:` line
- AGENTS.md: a `## Filters` section. With `pagination`, it also says that
  `ctx.links()` keeps the filters.

## Settings

None. The module file `src/modules/filters.rs` has no field.

## Decisions

- **No filter language.** A name in the query string is looked up in a
  list the application wrote. A name that is not there names no column.
  PLAN.md §9 says why: a misspelled field in a parsed filter string becomes
  `NULL`, and `NULL` becomes access.
- **Into the `WHERE`, beside the rule.** `Applied::condition` ANDs the
  filters with the policy's list rule, so a filter narrows what the rule
  admits and never widens it. The count and the page get one condition.
- **A refused value is a 400.** Ignoring it would answer another question
  than the one asked.

## Read more

- [brocanth-filters](../../crates/brocanth-filters/README.md)
- [Policy](../policy.md)
- Source: [`crates/brocanth-filters/src/scaffold.rs`](../../crates/brocanth-filters/src/scaffold.rs)
