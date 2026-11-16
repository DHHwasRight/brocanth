# `search`

Postgres full-text and trigram search: a site index, its migration, a page at /search.

The wizard calls it: *Full-text search over a site index, with a page*.

| | |
|---|---|
| Categories | **Routes** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-search` |
| Without it | no search; a visitor finds a page by its link or not at all |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/search.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`pagination`](pagination.md): the page reads `page` with `ctx.page_params()` and draws its pager with the module's helpers |
| After | [`pagination`](pagination.md) |
| Provides | none |
| Requires | `PgPool` (a Postgres pool): the index is a table and the page queries it through the `db` pool; providers: [`db`](db.md) |
| Uses | none |
| Adapts to | [`model`](model.md): the migration is a SeaORM file that `migrate sync` lists, not a SQL file; [`rsx`](rsx.md): the page is drawn by src/pages/search.rsx and the `PageNav` component |
| Contributes | `AgentsSection` `search`: how a model is searched |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | `per_page` = `20`: Results per page on /search. |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `migrations/20260101_000000_search_documents.sql`: the site index
  `search_documents`, its generated `tsvector` column, its GIN index and a
  trigram index on the title. With `model`,
  `migrations/m20260101_000000_search_documents.rs` instead: run
  `migrate sync` once so `migrations/mod.rs` lists it.
- `src/pages/search.rs`: `GET /search?q=…&page=…`
- `src/pages/search.rsx` (with `rsx`): the form, the ranked results and
  the pager. Without `rsx`, `search.rs` writes the HTML itself.

Slots:

- src/pages/mod.rs (the route table test): `/search`
- AGENTS.md: a `## Search` section

## Settings

None.

## Module file

`src/modules/search.rs`:

| Field | Kind | Default | Help |
|---|---|---|---|
| `per_page` | `u64` | `20` | Results per page on /search. |

## Decisions

- **Postgres, no service.** `tsvector`, `ts_rank_cd` and `pg_trgm` answer
  a site's search. A search service is a second system to run, back up and
  keep in step.
- **A site index table.** One query searches every kind of thing, so the
  page needs no join. The cost is a copy: write a row with
  `documents::upsert` when a thing changes.
- **Every row is public by default.** The page's rule is `Pred::Always`,
  written in `src/pages/search.rs`. A rule there goes into the `WHERE` of
  the page and of the count, so a hidden row is never ranked or counted.
- **A fixed migration name.** The table depends on nothing, and the
  generated project stays the same from one day to the next.

## Read more

- [brocanth-search](../../crates/brocanth-search/README.md)
- Source: [`crates/brocanth-search/src/scaffold.rs`](../../crates/brocanth-search/src/scaffold.rs)
