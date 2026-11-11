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
| Needs | [`policy`](policy.md): a filter is a `Pred` over a policy entity's columns, ANDed with its list rule |
| After | [`policy`](policy.md) |
| Adapts to | [`pagination`](pagination.md) |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-filters` |
| Without it | no typed filters; a list that filters parses its own query string |

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
