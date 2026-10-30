# `policy!`: rules as a declaration

PLAN.md §12 item 10. The macro came last on purpose: three crates used the
predicate algebra by hand first (`brocanth-resource`, `brocanth-realtime`,
`brocanth-admin`), and this syntax is what their call sites have in common.

The macro adds no meaning. It expands to the `Column`, `Entity` and `Policy`
impls that an application writes by hand today. `crates/brocanth-policy-macros/tests/equivalence.rs`
proves it: for every subject and every rule, the macro policy and the
hand-written policy give the same `Pred`, the same SQL and the same binds in
both dialects, and the same field mask.

## Turn it on

```toml
[dependencies]
brocanth-policy = { version = "0.1", features = ["macro"] }

[dev-dependencies]
# Only if a policy has an `eval { … }` clause.
brocanth-policy = { version = "0.1", features = ["macro", "agreement-proptest"] }
```

The macro emits `::brocanth_policy::…` paths, so the application depends on
`brocanth-policy` directly, even when it also uses
`brocanth_admin::row_policy`.

## What the call sites had in common

Before writing any syntax, we read every policy in the tree. Four patterns
repeat in all of them:

1. **The entity is boilerplate, written again in each file.** An enum, a
   `Column` impl with a `match` of name strings, an `Entity` impl. Every test
   suite has its own copy. The name strings must equal the admin's column names and the SeaORM
   field names. Nothing checks that.
2. **Each rule is a short list of guards, then one predicate.**
   `if who.admin { return Pred::Always; }`, then
   `Pred::any([Pred::eq(…), Pred::eq(…)])`.
3. **Absence is `Bound::or_deny(who.user_id)`**, or a `match` on the
   `Option` that does the same thing by hand.
4. **The field mask is one guard and one list**: `except([Id])` for an
   administrator, `only([Title])` for everyone else.

The syntax keeps these four shapes and removes the rest.

## The syntax

```rust
brocanth_policy::policy! {
    /// Blog posts.
    #[model(blog::post::Model)]          // optional: check against a struct
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,          // nullable column
        status: String,
        title: String,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list {
            who.banned => never,
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        create: author_id == who.user_id else deny,
        update {
            who.admin => always,
            author_id == who.user_id else deny,
        }
        delete {
            who.admin => always,
        }
        writable {
            who.admin => except [id],
            only [title, author_id],
        }
    }
}
```

### Entity

`entity Name in "table" { column: Type, … }` emits `struct Name`, `enum
NameColumn` (one variant per column, `author_id` becomes `AuthorId`), and the
`Column` and `Entity` impls. `Column::name()` is the field name.

* `#[key]` marks the primary key. Exactly one column has it.
* A column type is `i8`–`i64`, `u8`–`u32`, `f32`, `f64`, `bool`, `String`,
  `Vec<u8>`, or an `Option` of one of them. `Option` means the column is
  nullable.
* `#[model(path::Model)]` checks that each column is a field of that struct,
  with the same type. Use it with the SeaORM `Model`. Then the names the admin
  matches at runtime come from the same struct, and a typo or a changed type
  is a compile error.

### Policy

`policy Name(who: &Subject) for Entity { … }` emits `struct Name` and
`impl Policy for Name`. The subject can be any type; the macro does not look
inside it. Write `_` for a policy that does not read it. The entity must be
declared in the same `policy!` block, because the macro reads the column
types from it.

Rules: `list`, `view`, `create`, `update`, `delete`, `writable`. Each is the
`Policy` method of the same name. A rule you omit keeps the trait default:
`view` is `list`, every other rule denies.

A rule has two forms:

* `name: <predicate>,`
* `name { <guard> => <result>, …, <result> }`. A guard is any Rust `bool`
  expression. `cond => result,` is `if cond { return result; }`. The line
  without `=>` is the fallback and comes last. With no fallback, the rule
  denies (`never`, or `none` for `writable`).

Doc comments on a rule go on the generated method.

### Predicates

| You write | It expands to |
|---|---|
| `always`, `never` | `Pred::Always`, `Pred::Never` |
| `col == v`, `!=`, `<`, `<=`, `>`, `>=` | `Pred::eq(Col::X, v)`, `ne`, `lt`, `le`, `gt`, `ge` |
| `col is v`, `col is not v` | `Pred::is`, `Pred::is_not` (null-safe) |
| `col is null`, `col is not null` | `Pred::is_null`, `Pred::is_not_null` |
| `col in xs`, `col not in xs` | `Pred::one_of`, `Pred::none_of` |
| `v else deny`, `else allow`, `else null` | `Bound::or_deny(v)`, `or_allow`, `or_null` |
| `a && b && c` | `Pred::all([a, b, c])` |
| `a \|\| b` | `Pred::any([a, b])` |
| `!a` | `Pred::negate(a)` |
| `( … )` | grouping |
| `{ rust }` | the block, unchanged: any expression that returns the `Pred` |

A value is any Rust expression, up to the next `&&`, `||`, `else` or `,`.
Wrap it in parentheses if it contains one of these.

`!` applies to the next predicate, not to the column: `!status == "draft"` is
`!(status == "draft")`.

Each value goes through `brocanth_policy::typed::bind`, which checks its type
and returns the same `Bound` as `value.into()`. So `status == 3` and
`author_id == "x"` do not compile.

### Absence

A value of type `Option<T>` does not compile on its own. The error says the
rule does not say what absence means. Write one of:

* `else deny`: absence matches no row. The usual choice for an identity.
* `else allow`: absence matches every row. Add a comment with the reason.
* `else null`: compare with `NULL`, three-valued.

This is PLAN.md §9's vulnerability class: in PocketBase, a typo in a negated
rule becomes `NULL`, and `NULL` becomes access.

### Field masks

`all`, `none`, `only [col, …]`, `except [col, …]`, or a `{ … }` block that
returns a `FieldMask`. Prefer `only`: a column added later is not writable
until you say so.

### Escape hatch

A `{ … }` block in place of a predicate or a mask is plain Rust. Use it for
`Pred::Exists` (team membership), `Pred::Opaque`, or a rule that reuses
another: `update: { self.list(who) },`. Inside a block, the macro checks
nothing.

### `eval { … }`: the agreement test

PLAN.md §9 obligation 1: when you call `eval()`, the two interpretations must
agree. Opt in, and the macro emits the test:

```rust
pub policy PostPolicy(who: &Visitor) for Post {
    list: …,
    eval {
        subjects: [Visitor::anonymous(), Visitor::user(1), Visitor::admin()],
        database: test_db::posts,   // fn(&[MapRow<PostColumn>]) -> impl Oracle<Post>
        dialect: Sqlite,
        cases: 64,                  // optional, default 64
    }
}
```

The emitted `#[cfg(test)]` test calls `agreement::policy_agrees`. For each
case it generates rows, asks `database` for a fresh database that holds
exactly those rows, and asserts `eval(row) == (row ∈ query(condition))` for
every rule and every subject. The cell values come from the column types and
from the values the rules compare against, so near-misses are common.

`database` is the application's function: create the table, insert the rows,
answer the query. The macro does not know your driver. See
`crates/brocanth-policy-macros/tests/sqlite/mod.rs` for one in fifty lines.

## Before and after

### `brocanth-resource/tests/common/mod.rs`

Before, about 85 lines:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostColumn { Id, AuthorId, Status, Title }

impl Column for PostColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status, Self::Title];
    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::AuthorId => "author_id",
            Self::Status => "status",
            Self::Title => "title",
        }
    }
}

pub struct Post;

impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

pub struct PostPolicy;

impl Policy for PostPolicy {
    type Entity = Post;
    type Subject = Visitor;

    fn list(&self, who: &Visitor) -> EntityPred<Post> {
        if who.banned {
            return Pred::Never;
        }
        if who.admin {
            return Pred::Always;
        }
        Pred::any([
            Pred::eq(PostColumn::Status, "published"),
            Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id)),
        ])
    }

    fn create(&self, who: &Visitor) -> EntityPred<Post> {
        match who.user_id {
            Some(id) => Pred::eq(PostColumn::AuthorId, id),
            None => Pred::Never,
        }
    }

    fn update(&self, who: &Visitor) -> EntityPred<Post> {
        if who.admin {
            return Pred::Always;
        }
        Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id))
    }

    fn delete(&self, who: &Visitor) -> EntityPred<Post> {
        if who.admin { Pred::Always } else { Pred::Never }
    }

    fn writable(&self, who: &Visitor) -> FieldMask<PostColumn> {
        if who.admin {
            FieldMask::except([PostColumn::Id])
        } else {
            FieldMask::only([PostColumn::Title, PostColumn::AuthorId])
        }
    }
}
```

After, about 30 lines. The `create` rule's `match` becomes `else deny`, which gives
the same `Pred` for both arms:

```rust
brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        status: String,
        title: String,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list {
            who.banned => never,
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        create: author_id == who.user_id else deny,
        update {
            who.admin => always,
            author_id == who.user_id else deny,
        }
        delete {
            who.admin => always,
        }
        writable {
            who.admin => except [id],
            only [title, author_id],
        }
    }
}
```

### `brocanth-realtime/tests/common/mod.rs`

The audience inversion of PLAN.md §10 reads the shape of a `Pred`. The macro
emits the same shape, so the realtime tests pass unchanged on the macro
policy:

```rust
brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        team_id: Option<i64>,
        status: String,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        /// Published posts, or your own.
        list {
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        /// Only your own, whatever its status.
        view {
            who.admin => always,
            author_id == who.user_id else deny,
        }
    }
}
```

### `brocanth-admin-model/tests/policy.rs`

The hand-written entity repeated the SeaORM model by hand. Now
`#[model(…)]` checks it:

```rust
brocanth_policy::policy! {
    #[model(blog::post::Model)]
    entity Post in "post" {
        #[key] id: i32,
        author_id: i32,
        title: String,
        status: String,
    }

    policy PostRules(who: &Staff) for Post {
        list {
            who.superuser => always,
            author_id == who.id,
        }
        update: author_id == who.id && status == "draft",
        delete: author_id == who.id,
        writable {
            who.superuser => except [id],
            only [title],
        }
    }
}
```

`AdminResource` and `brocanth_resource::Resource` stay hand-written. Each is
five lines that name the entity, the policy and the dialect, and the
application owns that choice.

## Errors

Each error points at the token you wrote.

| You wrote | The compiler says |
|---|---|
| `authr_id == who.id` | `` `authr_id` is not a column of entity `Post` `` / `help: did you mean `author_id`?` |
| `only [titel]` | the same, on the mask |
| `status == who.id` | `` `i64` does not compare with this column `` / the column holds `Text` values |
| `author_id != who.user_id` (an `Option`) | `this value can be absent. The rule does not say what absence means` / write `else deny`, `else allow` or `else null` |
| `created_at: SystemTime` | `` `SystemTime` is not a column type that `policy!` can bind `` |
| `#[model(M)]` and a column `M` lacks | `` no field `titel` on type `&M` ``, with a suggestion |
| `#[model(M)]` and a different type | `mismatched types`, on the column |
| `read: …` | `` `read` is not a rule `` / `help: the rule for one row is `view`` |
| no `#[key]` | `` entity `Post` has no key column `` |

`crates/brocanth-policy-macros/tests/ui/` holds each case with the
compiler's output.

## Why a proc-macro

`macro_rules!` cannot do three things the syntax needs:

* turn `author_id` into the variant `AuthorId`;
* report an unknown column with the entity's name, the nearest column and the
  span of the typo, instead of "no rules expected this token";
* parse `a == x || b in xs && !(c is null)` with precedence, readably.

## Not done yet

* **A policy for an entity in another block.** The macro reads column types
  from the entity in the same block. One entity with several policies works;
  a policy in another module for a macro entity does not.
* **The table name is not checked against the model.** SeaORM's
  `table_name()` is not `const`, so `#[model(…)]` checks the fields and their
  types, and the `in "table"` string stays the author's.
* **Completeness against the model.** `#[model(…)]` checks that each column
  is a field. It does not check that each field is a column, because a SeaORM
  model can hold relation fields that are not columns. A field the entity
  omits is not writable (`FieldMask::check` refuses an unknown name), which is
  the safe direction.
* **No sugar for `EXISTS`.** Team membership is a `{ … }` block with
  `Membership::new(…)`.
* **The `eval` test needs `agreement-proptest`.** Without the feature, the
  test fails to compile with "cannot find function `policy_agrees`".
