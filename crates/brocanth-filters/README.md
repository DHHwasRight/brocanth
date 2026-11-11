# brocanth-filters

`brocanth-filters` reads typed list filters and a sort from the query
string, and turns them into a `brocanth-policy` predicate. The predicate is
ANDed with the policy's list rule and compiled once, so the filters go into
the `WHERE` of the count and of the page. A list never drops rows after it
has read a page.

It is django-filter's idea with the parser removed. Each name in the query
string is looked up in a declaration the application wrote, over the
entity's own column enum. A name that is not declared names no column. A
value is parsed into the column's kind before it becomes a bind parameter.

## Getting started

In a generated project (it needs `policy`):

```text
brocanth create my-app --with policy,filters
brocanth add filters
```

The module adds `ctx.filters(&declared)` to the context: it parses the
query string and answers a refused value with a 400 and a `help:` line.

By hand:

```toml
[dependencies]
brocanth-filters = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

## Declare, parse, compile

```rust
use brocanth_filters::{Direction, Filters};
use brocanth_policy::{Column, Dialect, Entity, Ident, Pred};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PostColumn { Id, Status, Score }

impl Column for PostColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::Status, Self::Score];
    fn name(&self) -> &'static str {
        match self { Self::Id => "id", Self::Status => "status", Self::Score => "score" }
    }
}

struct Post;
impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: PostColumn = PostColumn::Id;
}

let filters = Filters::new()
    .choice("status", PostColumn::Status, &["draft", "published"])
    .int("score", PostColumn::Score)
    .sort("score", PostColumn::Score)
    .default_sort("id", PostColumn::Id, Direction::Ascending);

let applied = filters.parse("status=published&score__ge=5&sort=-score&page=2").unwrap();

// The policy's list rule. A filter narrows it and never widens it.
let rule = Pred::eq(PostColumn::Score, 7_i64).or(Pred::gt(PostColumn::Score, 3_i64));
let condition = applied.condition::<Post>(rule, Dialect::Postgres).unwrap();
assert_eq!(
    condition.sql(),
    r#"(("posts"."score" = $1) OR ("posts"."score" > $2)) AND ("posts"."score" >= $3) AND ("posts"."status" = $4)"#
);
assert_eq!(
    applied.order_by::<Post>(PostColumn::Id).unwrap(),
    r#""posts"."score" DESC, "posts"."id" DESC"#
);
```

Give the count and the page the same condition, so the total counts the
rows the pages show. `page` is not a filter: `brocanth-pagination` reads
it, and its links keep every filter.

A value the declaration refuses is an error, not an ignored filter:

```rust
# use brocanth_filters::Filters;
# #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] enum C { Status }
# impl brocanth_policy::Column for C { const ALL: &'static [Self] = &[C::Status]; fn name(&self) -> &'static str { "status" } }
let filters = Filters::new().choice("status", C::Status, &["draft", "published"]);
let error = filters.parse("status=archived").unwrap_err().to_string();
assert!(error.contains("one of draft, published"));
assert!(error.contains("help:"));
```

## The query string

| Key | Means | Kinds |
|---|---|---|
| `name=v` | equal; repeated, it is `IN` (a group of checkboxes) | all |
| `name__ne=v` | not equal, null-safe: a row with no value is kept | all but bool |
| `name__lt`, `__le`, `__gt`, `__ge` | compared | int, float |
| `name__in=a,b` | one of | all but bool |
| `name__null=true` | absent; `false` for present | after `.nullable()` |
| `sort=name`, `sort=-name` | ordered, smallest or largest first | declared with `.sort()` |

An empty value is no filter, because a GET form sends `status=` for a
select left on "any". A name that is not declared is left alone, because
`page`, `q` and other keys share the query string.

## Decisions

- **No expression language.** PocketBase parses a filter string, and a
  misspelled field in a negated filter becomes `NULL`, then access
  (PLAN.md §9). Here a key is looked up in a declaration, and the result is
  a `Pred` over the column enum. There is nothing to misspell into.
- **ANDed, never alone.** `Applied::condition` takes the rule as an
  argument, so compiling the filters without it means writing it out.
- **A refused value is a 400.** Ignoring it answers a different question
  than the one that was asked, and the list then shows rows the user did
  not ask for.
- **`__ne` is null-safe.** "Not draft" keeps the rows with no status,
  which is what a person who asks for it means. SQL's `<>` drops them.
- **No `contains`.** A word inside a text column is a search. It needs an
  index to be fast, and `brocanth-search` builds one. A `LIKE` filter here
  would scan the table on every page.
- **The sort ends with the primary key.** `Applied::order_by` and
  `Applied::keyset` add it, so two pages never share a row and a keyset
  page has a total order.
