# brocanth-pagination

`brocanth-pagination` is the one paginator every brocanth list uses. It
has two kinds of page:

- an **offset page** (`page=3`): a number, a size and a total, with
  numbered links. Use it for a list a person browses: an admin table, a
  search result.
- a **keyset page** (`after=<cursor>`): the rows after the last row of the
  previous page. Use it for a feed, an export or an API read to the end. An
  index answers it in one seek, and a new row does not shift it.

Both write links that keep the rest of the query string, so a filter set on
page one is still set on page three. The crate has no dependency.
`brocanth-admin` and `brocanth-resource` page with it.

## Getting started

In a generated project:

```text
brocanth create my-app --with pagination
brocanth add pagination
```

The module adds `ctx.page_params()` and `ctx.links()` to the context. With
`rsx` it writes `src/pages/parts/pager.rsx` (`PageNav`, `KeysetNav`); with
`jt`, `src/pages/parts/pager.jt` (`PageLinks`, `KeysetLinks`); with
neither, `pager_html` in `src/pages/mod.rs`. With `policy` it turns on the
crate's `policy` feature.

By hand:

```toml
[dependencies]
brocanth-pagination = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

## An offset page

Count with the same `WHERE` as the page, then build the page from the
count. `Page` clamps: page 0 is page 1, a page past the end is the last.

```rust
use brocanth_pagination::{Limits, Links, Pager, Params};

let params = Params::parse("status=open&page=9&per_page=10").unwrap();
let total = 42; // SELECT count(*) ... WHERE <the same condition>
let page = Limits::new(25, 100).page(params.page, params.per_page, total);

assert_eq!(page.number(), 5, "page 9 of 5 is clamped to the last");
assert_eq!((page.offset(), page.limit()), (40, 10));

let pager = Pager::new(page, &Links::new("/tickets", "status=open&page=9&per_page=10"));
assert_eq!(pager.previous.as_deref(), Some("/tickets?status=open&per_page=10&page=4"));
assert_eq!(pager.next, None);
assert_eq!(pager.summary(), "41–42 of 42 · page 5 of 5");
```

A value that is not a number is an error with a `help:` line, not page one:

```rust
let error = brocanth_pagination::Params::parse("page=two").unwrap_err();
assert!(error.to_string().contains("help:"));
```

## A keyset page

Read one row more than the page shows. `KeysetPage::from_rows` drops it and
makes the cursor of the next page from the last row that stays.

```rust
use brocanth_pagination::{Cursor, Key, KeysetPage, KeysetPager, Links};

let rows = vec![(9, "c"), (8, "b"), (7, "a")]; // LIMIT 2 + 1, newest first
let page = KeysetPage::from_rows(rows, 2, |(id, _)| Cursor::new([Key::Int(*id)]));
assert_eq!(page.items.len(), 2);

let next = page.next.as_ref().unwrap();
assert_eq!(next.token(), "i8");
let pager = KeysetPager::new(&Links::new("/feed", ""), None, Some(next));
assert_eq!(pager.next.as_deref(), Some("/feed?after=i8"));
```

With the `policy` feature, `Keyset` turns a cursor into a `Pred`. AND it
with the policy's rule and compile once, so the cursor and the rule are in
one `WHERE`:

```rust,ignore
let keyset = Keyset::new(PostColumn::CreatedAt, PostColumn::Id, Direction::Descending);
let pred = policy.list(&who).and(keyset.after(params.after.as_ref())?);
let condition = pred.condition::<Post>(Dialect::Postgres)?;
let order = keyset.order_by::<Post>()?; // "posts"."created_at" DESC, "posts"."id" DESC
// SELECT ... WHERE <condition> ORDER BY <order> LIMIT Keyset::fetch(per_page)
```

A cursor is not a secret and not a permission. A client can forge one; it
only moves the window, because the rule stays in the `WHERE`.

## Decisions

- **One crate, no dependency.** The admin and `brocanth-resource` had a
  paginator each, with the same clamps written twice. Neither may depend
  on the other, and a paginator is not authorisation, so it is not in
  `brocanth-policy`.
- **`u64` throughout.** A row count comes from the database, and a table
  can hold more rows than a 32-bit `usize` counts.
- **A bad number is a 400.** Page one for `page=two` is a quiet repair
  that makes a paginator untrustworthy.
- **Page one has one address.** `Links::page(1)` writes no `page=1`, so a
  cache and a crawler see one URL for it.
- **Keyset pages link forward and to the first page only.** A previous
  link runs the query backwards and reverses the rows; few people read a
  feed backwards. pagy's keyset does the same.
- **The keyset tie-breaker is the primary key**, and the sort column must
  not be null: `NULL > $1` is unknown, so a null row drops out of every
  page after the first.
