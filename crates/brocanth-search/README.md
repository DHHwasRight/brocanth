# brocanth-search

`brocanth-search` is full-text and trigram search in Postgres. It needs no
search service: Postgres is already the base of every brocanth deployment.

- `Index` declares one table's search: the columns, their weights, the
  language, and the short columns that also match by trigram, so a typo
  still finds them.
- `Index::migration` writes a generated `tsvector` column and its GIN
  index. Postgres keeps the column in step with the source columns on every
  write.
- `Index::query` writes the ranked page and its count around a policy
  `Condition`. The rule goes first in the `WHERE`, so a row the rule refuses
  is never ranked, counted or shown. With `postgres`, `Index::search` runs
  both over a `brocanth_db::Db` and returns a `brocanth_pagination::Page`.
- `documents` is the site index: one table of everything a visitor can
  find, and the `search` module's `/search` page reads it.

## Getting started

In a generated project (it needs `db` and `pagination`):

```text
brocanth create my-app --with db,pagination,search
brocanth add search
```

The module writes the site index as a migration (a SQL file, or a SeaORM
file with `model`), the page `/search`, and its rules in `AGENTS.md`. Fill
the index when a thing changes:

```rust,ignore
brocanth_search::documents::upsert(ctx.db(), "post", &id, &title, &body, &url).await?;
```

By hand:

```toml
[dependencies]
brocanth-search = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

## One table's search

```rust
use brocanth_policy::{Column, Dialect, Entity, Ident, Pred};
use brocanth_search::{Index, Weight};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PostColumn { Id, Title, Body, Published }

impl Column for PostColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::Title, Self::Body, Self::Published];
    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Title => "title",
            Self::Body => "body",
            Self::Published => "published",
        }
    }
}

struct Post;
impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: PostColumn = PostColumn::Id;
}

let index = Index::<Post>::new("search")
    .source(PostColumn::Title, Weight::A)
    .source(PostColumn::Body, Weight::C)
    .trigram(PostColumn::Title);

// Into a migration, once.
let migration = index.migration().unwrap();
assert!(migration.up.contains(r#"CREATE INDEX "posts_search_idx" ON "posts" USING gin ("search");"#));

// The policy's list rule, compiled for Postgres.
let rule = Pred::eq(PostColumn::Published, true);
let condition = rule.condition::<Post>(Dialect::Postgres).unwrap();
let query = index.query(&condition, "rust async", 0, 20).unwrap();

assert!(query.count.sql.starts_with(
    r#"SELECT count(*) FROM "posts" WHERE ("posts"."published" = $1) AND ("posts"."search" @@ websearch_to_tsquery('english', $2)"#
));
assert!(query.select.sql.contains("ORDER BY search_rank DESC"));
assert_eq!(query.select.binds.len(), 2, "the rule's value, then the term");
```

The term is a bind parameter, never SQL text. `websearch_to_tsquery`
reads it as a person types it: words, `"a phrase"`, `-not`, `or`. It never
fails to parse.

## Decisions

- **Postgres, no service.** `tsvector`, `ts_rank_cd` and `pg_trgm` answer a
  site's search. Meilisearch or Algolia is a second system to run, back up
  and keep in step; an adapter can come later behind the same page.
- **A generated column, not a trigger.** `GENERATED ALWAYS AS (…) STORED`
  cannot drift from its sources, and needs no code in the application.
- **The rule first, in the `WHERE`.** A hit is ranked only among the rows
  the rule admits, and the count reads the same `WHERE`. Filtering hits
  after the page is read would give short pages and a wrong total.
- **A site index table.** One query searches every kind of thing, and the
  page needs no join and no per-model code. The cost is a copy that the
  application must update; the `AGENTS.md` rule says when.
- **Every row of the site index is public by default.** The page's rule
  is `Pred::Always`, written out in the page, where an agent changes it.
- **`pg_trgm` for typos.** A title matches by similarity too, so "postgress"
  finds "Postgres". It is a trusted extension since Postgres 13.
- **A fixed name for the SQL migration** (`20260101_000000_…`). The table
  depends on nothing, so it may run before or after any other file, and
  the generated project stays the same from one day to the next.
