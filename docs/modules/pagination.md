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
| Conflicts | none |
| Outside the runtime | yes |
| Crates | `brocanth-pagination` (feature `policy` with `policy`) |
| Without it | no paginator; each list counts its own offset and writes its own links |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/pagination.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | [`rsx`](rsx.md): writes src/pages/parts/pager.rsx, two pager components; [`jt`](jt.md): writes src/pages/parts/pager.jt, the page links as JSON; [`policy`](policy.md): turns on the crate's `policy` feature, and AGENTS.md says to AND a keyset cursor with the rule |
| Contributes | `AgentsSection` `pagination`: how a list is paginated |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `src/pages/parts/pager.rsx` (with `rsx`): `PageNav` and `KeysetNav`
- `src/pages/parts/pager.jt` (with `jt`): `PageLinks` and `KeysetLinks`, the links as JSON

Slots:

- src/pages/mod.rs (items): `ctx.page_params()`, which reads `page`,
  `per_page`, `after` and `before` and answers a bad value with a 400; `ctx.links()`,
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
- **A keyset page links to the next, the previous and the first page.**
  The previous link is `before=<cursor>`: the same query with the
  comparison and the `ORDER BY` reversed, one index seek, and the rows put
  back in order. A stack of cursors in the URL would grow with every page.
- **A cursor key of the wrong kind is a 400.** `Keyset::after` needs the
  column enum to implement `brocanth_policy::Sortable`, and checks each key
  against its column before the query. Without the check, text against an
  integer column is a database error on PostgreSQL, a 500.

## Read more

- [brocanth-pagination](../../crates/brocanth-pagination/README.md)
- Source: [`crates/brocanth-pagination/src/scaffold.rs`](../../crates/brocanth-pagination/src/scaffold.rs)
