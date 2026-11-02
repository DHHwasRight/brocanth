# brocanth-policy

Authorization as a typed predicate over an entity's columns, with a
witness for the write. A rule is a `Pred`. It compiles to a SQL `WHERE`
fragment with an ordered bind list (`condition()`), so a list query asks
the database for the rows the subject may see. Before a write,
`admission()` checks one row. The check gives a `Verified`, and the write
consumes that `Verified`: the `UPDATE` or `DELETE` re-applies the same
condition, and zero affected rows is an error. A column is the entity's
own enum, never a string, so a typo does not compile. An absent value
(`None`) is a decision that the author writes: `Bound::or_deny`,
`or_allow` or `or_null`. Use this crate for every authorization decision
about rows. It does not authenticate anybody (that is `brocanth-auth`),
and it opens no database connection: it emits SQL text and `Value`s, and
your code runs them.

## The two rules to follow

1. **A list uses `condition()`.** Put the condition in the query's
   `WHERE` clause. Do not load a page of rows and filter it afterwards: a
   paginator that filters after the load shows wrong totals and short
   pages.
2. **A write consumes `Verified`.** Get it from `admission(..).resolve(rows)`,
   then call `update`, `delete` or `release` on it. Each method takes the
   witness by value. `Write::applied(0)` is `Denied::RaceLost`, not
   success.

## Getting started

```toml
[dependencies]
brocanth-policy = { git = "https://github.com/DHHwasRight/brocanth.git" }
# Features, all off by default:
#   macro               - `policy!`: an entity and its rules in one block
#   agreement-proptest  - the property test that proves eval() agrees with condition()
#   scaffold            - the `brocanth create` module; only the CLI turns it on
```

Inside this workspace: `brocanth-policy.workspace = true`. With no
feature, the only dependency is `thiserror`.

### As a module

```text
brocanth create my-app --with policy
brocanth add policy
```

The module writes `src/plugins/policy/mod.rs`. Its plugin reads the SQL
dialect once at startup and puts a `Rules` value on the context, so every
call site reads `rules.dialect` from one place.

| Key | Default | Flag | Variable |
|---|---|---|---|
| `dialect` | `sqlite` with `model`, else `postgres` | `--policy.dialect` | `$<APP>_POLICY__DIALECT` |

With the `config` module, the key is `dialect` in `[policy]` in
`config/<env>.toml`. Keep it in step with the database: a condition for
the wrong dialect is valid SQL that the engine rejects.

## Examples

All three examples use this entity. The `policy!` macro writes the same
code for you (see `crates/brocanth-policy-macros/README.md`).

```rust
use brocanth_policy::{Column, Entity, Ident};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostColumn { Id, AuthorId, Status }

impl Column for PostColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status];
    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::AuthorId => "author_id",
            Self::Status => "status",
        }
    }
}

pub struct Post;

impl Entity for Post {
    type Column = PostColumn;
    // Checked while the program is built: a name that is not an identifier
    // stops the build.
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}
```

### A list: the policy becomes the `WHERE` clause

```rust
# use brocanth_policy::{Column, Entity, Ident};
# #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
# pub enum PostColumn { Id, AuthorId, Status }
# impl Column for PostColumn {
#     const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status];
#     fn name(&self) -> &'static str {
#         match self { Self::Id => "id", Self::AuthorId => "author_id", Self::Status => "status" }
#     }
# }
# pub struct Post;
# impl Entity for Post {
#     type Column = PostColumn;
#     const TABLE: Ident = Ident::must("posts");
#     const PRIMARY_KEY: Self::Column = PostColumn::Id;
# }
use brocanth_policy::{Bound, Dialect, EntityPred, Policy, Pred, Value};

/// What the application knows about whoever asks. Anonymous is `None`.
pub struct Visitor { user_id: Option<i64> }

pub struct PostPolicy;

impl Policy for PostPolicy {
    type Entity = Post;
    type Subject = Visitor;

    /// Published posts, or your own.
    fn list(&self, who: &Visitor) -> EntityPred<Post> {
        Pred::any([
            Pred::eq(PostColumn::Status, "published"),
            // `or_deny`: an anonymous visitor owns no post.
            Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id)),
        ])
    }

    /// Only the author may edit. `create` and `delete` keep the default,
    /// which denies.
    fn update(&self, who: &Visitor) -> EntityPred<Post> {
        Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id))
    }
}

let ada = Visitor { user_id: Some(7) };
let condition = PostPolicy.list(&ada).condition::<Post>(Dialect::Postgres).unwrap();
assert_eq!(
    condition.sql(),
    r#"("posts"."status" = $1) OR ("posts"."author_id" = $2)"#
);
assert_eq!(condition.binds(), [Value::Text("published".into()), Value::Int(7)].as_slice());

// Your query: SELECT ... FROM posts WHERE <condition.sql()> ORDER BY ... LIMIT ...
// with `condition.binds()` bound in order. Do not filter the rows afterwards.

// An anonymous visitor's update rule is a constant. It matches no row.
let anonymous = Visitor { user_id: None };
let condition = PostPolicy.update(&anonymous).condition::<Post>(Dialect::Postgres).unwrap();
assert_eq!(condition.sql(), "FALSE");
```

### A write: check one row, then write with the same condition

```rust
# use brocanth_policy::{Column, Entity, Ident};
# #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
# pub enum PostColumn { Id, AuthorId, Status }
# impl Column for PostColumn {
#     const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status];
#     fn name(&self) -> &'static str {
#         match self { Self::Id => "id", Self::AuthorId => "author_id", Self::Status => "status" }
#     }
# }
# pub struct Post;
# impl Entity for Post {
#     type Column = PostColumn;
#     const TABLE: Ident = Ident::must("posts");
#     const PRIMARY_KEY: Self::Column = PostColumn::Id;
# }
use brocanth_policy::{Denied, Dialect, Pred, Value};

let rule = Pred::eq(PostColumn::AuthorId, 7i64); // `PostPolicy.update(&who)` in an application

// 1. The admission query: SELECT 1 FROM posts WHERE id = ? AND (<condition>) LIMIT 1.
let admission = rule.admission::<Post>(Dialect::Sqlite, 42i64).unwrap();
let _sql = admission.query().sql(); // run it, and count the rows it returned

// 2. Zero rows is NotAuthorised. The crate does not say whether the row was
//    missing or forbidden: that difference is itself a disclosure.
let refused = rule.admission::<Post>(Dialect::Sqlite, 42i64).unwrap().resolve(0);
assert!(matches!(refused, Err(Denied::NotAuthorised)));

// 3. One row gives the witness. The write consumes it, and re-applies the
//    condition inside the UPDATE, so the row cannot change between the check
//    and the write.
let verified = admission.resolve(1).unwrap();
let write = verified
    .update(Dialect::Sqlite, [(PostColumn::Status, Value::Text("published".into()))])
    .unwrap();
assert!(write.query().sql().contains(r#""posts"."author_id" = ?"#));

// 4. Run the statement, then report what it changed. Zero rows means the row
//    stopped matching after the check: an authorization failure, not a no-op.
assert!(matches!(write.applied(0), Err(Denied::RaceLost)));
```

`Verified` has no public constructor and is not `Clone`. A witness that
is dropped unused fails a `debug_assert!` in a debug build. Call
`release()` when the write is not a statement this crate builds; it gives
back the id and the condition that must still guard it.

### Inspect a rule

Every `Pred` variant is public, so a test can ask what a rule means.

```rust
# use brocanth_policy::{Column, Entity, Ident};
# #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
# pub enum PostColumn { Id, AuthorId, Status }
# impl Column for PostColumn {
#     const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status];
#     fn name(&self) -> &'static str {
#         match self { Self::Id => "id", Self::AuthorId => "author_id", Self::Status => "status" }
#     }
# }
use brocanth_policy::{Bound, Pred};

let anonymous: Option<i64> = None;
// `or_allow` grants access when the value is absent. The test says so.
let open = Pred::eq(PostColumn::AuthorId, Bound::or_allow(anonymous));
assert!(open.is_always());
let closed = Pred::eq(PostColumn::AuthorId, Bound::or_deny(anonymous));
assert!(closed.is_never());
```

## Testing

```text
cargo test -p brocanth-policy
```

No database and no environment variable. The agreement tests run the
generated SQL against an in-memory SQLite (`sqlx` with `sqlite-bundled`,
a dev-dependency only), because the claim is that this crate's SQL and its
evaluator agree, and only a real engine can answer that.

An application tests its policies at three levels:

- **The rule:** assert on `condition().sql()` and `binds()`, or on
  `is_always()` / `is_never()` for a subject, as above.
- **`eval()` agrees with `condition()`:** if you call `Pred::eval`, turn on
  `agreement-proptest` in `[dev-dependencies]` and run
  `agreement::policy_agrees` (the `policy!` macro's `eval { … }` clause
  writes that test).
- **The screen:** send requests with `brocanth_test::TestClient` as two
  different users, and assert that each sees only their rows and that a
  forbidden write answers 403 or 404. See `crates/brocanth-test/README.md`.

## More

- `docs/policy.md`: the `policy!` syntax, its errors, and each real call
  site before and after.
- PLAN.md §9: why both a predicate and a witness, and the three
  obligations.
- `docs/module-flags.md`: how `policy.dialect` becomes a flag, a variable
  and a config key.
