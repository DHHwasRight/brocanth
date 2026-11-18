# `policy!`: rules as a declaration

PLAN.md §12 item 10. The macro came last on purpose: three crates used the
predicate algebra by hand first (`brocanth-resource`, `brocanth-realtime`,
`brocanth-admin`), and this syntax is what their call sites have in common.

The macro adds no meaning. It expands to the `Column`, `Entity` and `Policy`
impls that an application writes by hand today. `crates/brocanth-policy-macros/tests/equivalence.rs`
proves it: for every subject and every rule, the macro policy and the
hand-written policy give the same `Pred`, the same SQL and the same binds,
and the same field mask.

## Turn it on

```toml
[dependencies]
brocanth-policy = { version = "0.1", features = ["macro"] }

[dev-dependencies]
# Only if a policy has an `eval { … }` clause.
brocanth-policy = { version = "0.1", features = ["macro", "agreement-proptest"] }
# Only if a policy has a `db { … }` clause.
brocanth-rls = { version = "0.1", features = ["agreement"] }
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
            only [title],
        }
        writable_on_create {
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
  `Vec<u8>`, `uuid::Uuid` (feature `uuid`), `chrono::DateTime<Utc>`
  (feature `chrono`, a `timestamptz`), or an `Option` of one of them.
  `Option` means the column is nullable.
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

Rules: `list`, `view`, `create`, `update`, `delete`, `writable`,
`writable_on_create`. Each is the `Policy` method of the same name. A rule
you omit keeps the trait default: `view` is `list`, `writable_on_create` is
`writable`, every other rule denies. A `#[db]` policy can also have
`update_check` (see [`#[db]`](#db-one-rule-for-the-application-and-the-database)).

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
| `exists T(fk == col, …)` | `Pred::Exists(Membership)`: see [Membership](#membership-exists) |
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

`!` over a comparison with `else deny` or `else allow` does not compile.
`else deny` makes the comparison `never` for an absent value, and `!never` is
`always`: the negation grants access to the visitor that the word refuses.
Write the inverse operator: `author_id != who.user_id else deny`, not
`!(author_id == who.user_id else deny)`.

### Field masks

`all`, `none`, `only [col, …]`, `except [col, …]`, or a `{ … }` block that
returns a `FieldMask`. Prefer `only`: a column added later is not writable
until you say so.

A policy has two masks:

* `writable`: the columns an update may change.
* `writable_on_create`: the columns a create may set. It defaults to
  `writable`.

Keep the owner column (`author_id`) out of `writable`. The reason: `update`
decides on the row as stored, not on the row that the write leaves. For an
author, `author_id == who.user_id` is true before the write, whatever the
write sets. So with `author_id` in `writable`, an author can give a post to
somebody else, and no rule refuses it. `create` decides on the proposed row,
so `create: author_id == who.user_id else deny` lets a create name only the
author's own id. Put `author_id` in `writable_on_create`, and nowhere else.

`brocanth-resource` and the admin check `writable_on_create` on a create and
`writable` on an update, before they read a row. A refused column is `403`.

Decision: two masks, and not a server that fills in the owner. A hook that
sets `author_id` from the subject must exist in each write path
(`brocanth-resource`, the admin, each handler), and a write path that forgets
it lets the client choose the owner again. The create rule already pins the
owner, in the policy, for every write path. The cost: the client sends its
own id on a create. We decided against one mask with a `create`/`update`
argument: each existing policy would break to say the same thing, and the
`policy!` guards would need a name for the argument.

### Membership: `exists`

```rust
pub entity PostTeam in "post_teams" {
    #[key] post_id: i64,
    team_id: Option<Uuid>,
}

pub policy PostPolicy(who: &Visitor) for Post {
    list: status == "published"
        || exists PostTeam(post_id == id, team_id == who.team else deny),
}
```

`exists T(…)` is a row of the link entity `T` that points at this row. `T`
is an entity of the same `policy!` block, so each column is checked. Inside
the parentheses, the left side of each comparison is a column of `T`. The
right side is one of:

* a bare name: a column of the outer entity. Exactly one comparison is
  `<column of T> == <column of this entity>`; it links the two tables, and
  both columns hold the same kind of value.
* a literal, or `is null` / `is not null`.
* a value such as `who.team else deny`, with the same absence words as
  everywhere.

It expands to the `Membership::new(…).filter_by(…)` chain that you write by
hand, so the `Pred` is the same. An absent `else deny` value makes it
`never`. `else allow` drops that filter, as `filter_by` does. `in` is not
allowed inside `exists`. Write a constant with its path (`self::TEAM`): a
bare name is a column.

**`!exists` is one term.** `!exists T(…)` (or `!(exists T(…))`) is
`NOT EXISTS`. When an `else deny` value in its filter is absent, it is
`never`, not `always`. Reason: `else deny` makes the term that holds it
`never`, and `NOT EXISTS` is the term. Without this, an anonymous visitor
passes `!exists Block(blocked_id == author_id, blocker_id == who.user_id
else deny)` and sees every post. The database does the same
(`s(x) IS NOT NULL AND NOT EXISTS (…)`), so the two agree. `!` over a
larger predicate that holds such an `exists`, as in
`!(exists T(…) || status == "x")`, does not compile, for the reason in
[Absence](#absence).

### Escape hatch

A `{ … }` block in place of a predicate or a mask is plain Rust. Use it for
`Pred::Opaque`, or a rule that reuses another: `update: { self.list(who) },`.
Inside a block, the macro checks nothing. A `#[db]` policy refuses a block in
a rule (but not in a mask); see below.

### `#[db]`: one rule for the application and the database

docs/policy-rls-bridge.md. Put `#[db]` on a policy, and the macro also
emits the rules as data: `impl DbPolicy` with `fn rules() -> Rules<Post>`.
`brocanth-rls` reads it to write the database policy (RLS). The `Policy`
methods then resolve the same tree with the subject's attributes:
`rules().list.resolve(&who.attributes())`. So `condition()` and the RLS
expression come from one tree. `tests/db.rs` proves that a policy gives the
same `Pred`, SQL, binds and `eval` with and without `#[db]`.

```rust
#[derive(brocanth_policy::Subject)]
pub struct Visitor {
    #[attr] pub user_id: Option<i64>,
    #[attr] pub admin: bool,
    #[attr] pub banned: bool,
}

brocanth_policy::policy! {
    pub entity Post in "posts" { #[key] id: i64, author_id: Option<i64>, status: String }

    #[db]
    pub policy PostPolicy(who: &Visitor) for Post {
        list {
            who.banned => never,
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        update {
            who.admin => always,
            author_id == who.user_id else deny && status == "draft",
        }
        // The new row of an update: an author may publish a draft.
        update_check {
            who.admin => always,
            author_id == who.user_id else deny && status in ["draft", "published"],
        }
        writable { only [status] }
    }
}
```

The subject derives `Subject`, and a rule reads only its `#[attr]` fields.
Each `#[attr]` field becomes a setting, `brocanth.<field>`. A field without
`#[attr]` is a compile error,
`` no field `x` on type `Visitor_attributes` ``, with the list of attributes.

**`update_check`** is the database's `UPDATE … WITH CHECK`: which rows an
update may leave (bridge decision 8). It defaults to `update`. The
application does not read it: there, the `writable` mask does that job. So
`update_check` needs `#[db]`; without it, the macro refuses the rule.

**`#[app_only("reason")]`** on one rule keeps that rule in Rust. The
`Policy` method runs the Rust code, and `rules()` holds
`Rule::app_only("reason")`: no arms, `never`, and `app_only` set. It
resolves to `never`. `rls make` refuses the command until the schema gives
it a hand-written `SqlExpr` (bridge decision 4). Use it for a rule the
database cannot check, such as one that reads the clock. It needs `#[db]`,
and it does not apply to `update_check` or to a mask.

**Field masks** stay in the application (bridge decision 9): `writable`
and `writable_on_create` expand as without `#[db]`, `{ … }` blocks and Rust
guards included. `DbPolicy::rules()` also holds each mask as data
(`Rules::writable`, `Rules::writable_on_create`): `MaskRule::Fixed(mask)`
for a mask with no guard, `MaskRule::PerSubject` for one with a guard or a
block. `brocanth-rls` grants a fixed mask as columns (`GRANT UPDATE
("title")`), so the database refuses the other columns too. A guarded mask
cannot be a grant, because a grant is per role and the mask is per subject:
`rls make` prints a warning for its table. Write the mask with no guard when
it is the same for everyone.

**What a `#[db]` rule may hold.** The database reads settings and literals.
It cannot run Rust. Each refusal is a compile error on your token, with a
`help:` line:

| You wrote | help |
|---|---|
| a `{ … }` block in a rule or a guard (also `Pred::Opaque`) | write the rule in the policy! syntax, or mark it #[app_only("reason")] |
| a guard other than `who.f`, `!g`, `who.f == lit`, `who.f != lit`, `&&`, `\|\|`, `( … )` | add the value to the subject as a #[attr] field and test that |
| a value that uses `who` and is not exactly `who.<field>` | compute the value when you build the subject, and store it in a field |
| `col is who.x`, `col is not who.x` | use == with else deny; IS NOT DISTINCT FROM matches NULL to a missing setting |
| `col in who.xs` | a setting holds one value; use a membership table and exists |
| `col == who.x else allow` | put the fact in a bool #[attr] field and guard on it |
| a value that is not a literal or a constant (`String::from(…)`, `vec![…]`) | the database policy holds the value written at `rls make`; use a literal or a constant, or store the value in a #[attr] field of the subject |
| `else …` on a literal | `else` says what a missing subject value means; remove it |

A guard reads a `bool` attribute or compares one with a literal. A
`who.f` with no `else` must be a non-`Option` field: the database denies
when its setting is missing.

**An empty text is absent.** A setting cannot tell `''` from "not set",
so `Attributes` stores an empty `String` attribute as absent, in the
application too. A guard over an absent attribute denies the whole rule:
with `role: String::new()`, `who.role == "lead" => …` denies, it does not
take the next arm. Give a role a non-empty value, such as `"guest"`. The types are checked as without `#[db]`:
`status == who.user_id` does not compile.

### `eval { … }`: the agreement test

PLAN.md §9 obligation 1: when you call `eval()`, the two interpretations must
agree. Opt in, and the macro emits the test:

```rust
pub policy PostPolicy(who: &Visitor) for Post {
    list: …,
    eval {
        subjects: [Visitor::anonymous(), Visitor::user(1), Visitor::admin()],
        database: test_db::posts,   // fn(&[MapRow<PostColumn>]) -> impl Oracle<Post>
        dialect: Postgres,          // optional: Postgres is the only one
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
`crates/brocanth-policy-macros/tests/postgres/mod.rs` for one in fifty lines.

### `db { … }`: the database agreement test

docs/policy-rls-bridge.md, decision 10: with `#[db]`, the application's
`condition()` and the database policy must admit the same rows. Opt in,
and the macro emits the test:

```rust
#[db]
pub policy PostPolicy(who: &Visitor) for Post {
    list: …,
    db {
        subjects: [Visitor::anonymous(), Visitor::user(1), Visitor::admin()],
        cases: 64,                          // optional, default 64
        url: crate::tests::database_url(),  // optional: an Option<String>
    }
}
```

The emitted `#[cfg(test)]` test calls `brocanth_rls::agreement::run`. Per
case, it creates a schema, the table from the entity's column types, the
link tables of each `exists`, and the policies `rls make` writes. It
inserts generated rows (with `NULL`s and near-misses) and, for each
subject, compares `SELECT`, `INSERT`, `UPDATE` and `DELETE` as the
application and as the database. On a difference, it prints the subject,
the rows and both SQL texts. docs/rls.md §5 has the details.

`url` is an expression of type `Option<String>`. With `None`, the test
prints `!! SKIPPED` and passes. Without `url`, it reads
`BROCANTH_TEST_DATABASE_URL`. In an application, point it at the
`[db] url` of the settings. The URL's user must be a superuser: the test
creates roles and reads around the policies.

Keep `update`, `delete` and `update_check` inside `list` (or `view`).
Postgres applies the `SELECT` policy to an `UPDATE` or `DELETE` that reads
a column, and to the new row of the `UPDATE`. The test fails, with a
`help:` line, when a rule reaches a row outside it.

`db { … }` needs `#[db]`: without it, the macro refuses the clause. An
`#[app_only]` command is left out of the test.

## Before and after

### `brocanth-resource/tests/common/mod.rs`

Before, about 95 lines:

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
            FieldMask::only([PostColumn::Title])
        }
    }

    fn writable_on_create(&self, who: &Visitor) -> FieldMask<PostColumn> {
        if who.admin {
            FieldMask::except([PostColumn::Id])
        } else {
            FieldMask::only([PostColumn::Title, PostColumn::AuthorId])
        }
    }
}
```

After, about 35 lines. The `create` rule's `match` becomes `else deny`, which gives
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
            only [title],
        }
        writable_on_create {
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
        create {
            who.superuser => always,
        }
        update: author_id == who.id && status == "draft",
        delete: author_id == who.id,
        writable {
            who.superuser => except [id, author_id],
            only [title],
        }
        writable_on_create {
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
| `exists PostTeam(team_id == 3)` | `` `exists PostTeam` does not say which row of `PostTeam` belongs to this row `` |
| `update_check` without `#[db]` | `` `update_check` needs `#[db]` `` |
| a `#[db]` refusal | see [`#[db]`](#db-one-rule-for-the-application-and-the-database) |

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
* **`exists` links one column.** A link table that joins on two columns
  needs a `{ … }` block with `Membership`, and so is not `#[db]`.
* **The `eval` test needs `agreement-proptest`.** Without the feature, the
  test fails to compile with "cannot find function `policy_agrees`".
* **The `db` test needs `brocanth-rls` with `agreement`** in the
  dev-dependencies. Without it, the test fails to compile on
  `brocanth_rls::agreement`.
* **A guard chain loses the column's index** in the database policy: the
  planner cannot read a setting at plan time. A rule without guards keeps
  it (docs/policy-rls-bridge.md, decision 10 notes).
