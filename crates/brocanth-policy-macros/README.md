# brocanth-policy-macros

The `policy!` macro: an entity with typed columns and the rules over it,
in one block. It expands to the `Column`, `Entity` and `Policy` impls that
you otherwise write by hand for `brocanth-policy`, and nothing else. A
test (`tests/equivalence.rs`) proves that the macro policy and the
hand-written policy give the same `Pred`, the same SQL and the same
binds. An unknown column is a compile error on the typo, with
the nearest column name. An `Option` value without `else deny`,
`else allow` or `else null` does not compile. Do not depend on this crate
directly: `brocanth-policy` re-exports the macro behind its `macro`
feature, and the macro emits `::brocanth_policy::…` paths. Write the
impls by hand when a rule needs a shape the syntax does not have. A
`{ … }` block is the escape hatch.

## Getting started

```toml
[dependencies]
brocanth-policy = { git = "https://github.com/DHHwasRight/brocanth.git", features = ["macro"] }

[dev-dependencies]
# Only if a policy has an `eval { … }` clause.
brocanth-policy = { git = "https://github.com/DHHwasRight/brocanth.git", features = ["macro", "agreement-proptest"] }
```

Inside this workspace: `brocanth-policy = { workspace = true, features = ["macro"] }`.

There is no module for this crate. `brocanth create --with policy` adds
`brocanth-policy` without the `macro` feature. Turn the feature on by hand.

## Examples

### An entity and its policy

```rust
pub struct Visitor {
    pub user_id: Option<i64>,
    pub admin: bool,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,   // Option: the column is nullable
        status: String,
        title: String,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list {
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        update {
            who.admin => always,
            author_id == who.user_id else deny,
        }
        writable {
            who.admin => except [id],
            only [title],
        }
        // `create` and `delete` are omitted, so they deny.
    }
}

use brocanth_policy::{Column, Dialect, Policy};

// The macro wrote `PostColumn`, one variant per column.
assert_eq!(PostColumn::AuthorId.name(), "author_id");

let ada = Visitor { user_id: Some(7), admin: false };
let condition = PostPolicy.list(&ada).condition::<Post>(Dialect::Postgres).unwrap();
assert_eq!(
    condition.sql(),
    r#"("posts"."status" = $1) OR ("posts"."author_id" = $2)"#
);

// An anonymous visitor edits nothing, and an omitted rule denies.
let anonymous = Visitor { user_id: None, admin: false };
assert!(PostPolicy.update(&anonymous).is_never());
assert!(PostPolicy.delete(&ada).is_never());

// The field mask.
assert!(PostPolicy.writable(&ada).permits(PostColumn::Title));
assert!(!PostPolicy.writable(&ada).permits(PostColumn::Status));
```

Use the generated policy as you use a hand-written one: `condition()` in
the list query, and `admission()`, then the `Verified` it gives, for each
write. See `crates/brocanth-policy/README.md`.

### What a typo gives you

```rust,compile_fail
pub struct Visitor { pub user_id: Option<i64> }

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        // error: `authr_id` is not a column of entity `Post`
        //        help: did you mean `author_id`?
        list: authr_id == who.user_id else deny,
    }
}
```

```rust,compile_fail
pub struct Visitor { pub user_id: Option<i64> }

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        // error: this value can be absent. The rule does not say what absence means
        //        help: write `else deny`, `else allow` or `else null`
        list: author_id == who.user_id,
    }
}
```

The exact compiler output for each error is in `tests/ui/*.stderr`.

### Prove that `eval()` agrees with `condition()`

An `eval { … }` clause makes the macro emit a `#[cfg(test)]` test. For
generated rows and each subject, it asserts that `eval(row)` equals
"`row` is in the result of the `condition()` query". `database` is your
function: it creates the table, inserts the rows and answers the query.
`tests/postgres/mod.rs` has one: a schema per call, dropped when the
oracle drops. The generated test has no skip of its own. To skip it when
no database is set, give `subjects` an expression that yields no subjects
then, as `postgres::or_skip` does.

```text
pub policy PostPolicy(who: &Visitor) for Post {
    list: status == "published" || author_id == who.user_id else deny,
    eval {
        subjects: [Visitor::anonymous(), Visitor::user(1)],
        database: test_db::posts,   // fn(&[MapRow<PostColumn>]) -> impl Oracle<Post>
        dialect: Postgres,
        cases: 64,                  // optional, default 64
    }
}
```

This needs `agreement-proptest` in `[dev-dependencies]`.

## Testing

```text
cargo test -p brocanth-policy-macros
```

- `tests/equivalence.rs`: the macro and the hand-written policy give the
  same values, SQL and masks.
- `tests/agreement.rs`: an `eval { … }` clause against PostgreSQL.
- `tests/trybuild.rs`: `tests/ui/pass_*.rs` compile, and each
  `tests/ui/fail_*.rs` fails with the message in its `.stderr` file. The
  messages are rustc's own text, pinned to the toolchain in `mise.toml`.
  After a toolchain change, regenerate them with `TRYBUILD=overwrite`.

`tests/agreement.rs` needs `BROCANTH_TEST_DATABASE_URL`. Without it, its
tests print `!! SKIPPED` and pass. The other tests need no database.

## More

- `docs/policy.md`: the full syntax (predicates, absence, masks, escape
  hatch, `eval`), the error table, and each real call site before and
  after.
- `crates/brocanth-policy/README.md`: the algebra the macro expands to.
