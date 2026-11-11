# `pagination`

Offset and keyset pages, with links that keep the query; no dependency.

The wizard calls it: *Pages of rows, with links that keep the query*.

| | |
|---|---|
| Categories | **Renderer** |
| Wizard question | Pages |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | [`rsx`](rsx.md), [`jt`](jt.md), [`policy`](policy.md) |
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-pagination` (feature `policy` with `policy`) |
| Without it | no paginator; each list counts its own offset and writes its own links |

## What it writes

Files:

- `src/pages/parts/pager.rsx` (with `rsx`): `PageNav` and `KeysetNav`
- `src/pages/parts/pager.jt` (with `jt`): `PageLinks` and `KeysetLinks`, the links as JSON

Slots:

- src/pages/mod.rs (items): `ctx.page_params()`, which reads `page`,
  `per_page` and `after` and answers a bad value with a 400; `ctx.links()`,
  which writes links that keep every other key of the query string
- src/pages/mod.rs (items, without `rsx`): `pager_html(&pager)`, the offset
  pager as escaped HTML
- AGENTS.md: a `## Pagination` section

## Settings

None. The module file `src/modules/pagination.rs` has no field.

## Decisions

- **One crate, no dependency.** `brocanth-admin` and `brocanth-resource`
  had a paginator each. Both now use this one, so a fix to an off-by-one
  reaches every list.
- **A bad number is a 400, not page one.** Page one for `page=two` is a
  quiet repair that makes a paginator untrustworthy.
- **Keyset pages link forward and to the first page only.** A previous
  link needs the query run backwards. pagy's keyset does the same.
- **A cursor key of the wrong kind is a 400.** `Keyset::after` needs the
  column enum to implement `brocanth_policy::Sortable`, and checks each key
  against its column before the query. Without the check, text against an
  integer column is a database error on PostgreSQL, a 500.

## Read more

- [brocanth-pagination](../../crates/brocanth-pagination/README.md)
- Source: [`crates/brocanth-pagination/src/scaffold.rs`](../../crates/brocanth-pagination/src/scaffold.rs)
