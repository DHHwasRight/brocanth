# brocanth-model

The model layer of brocanth, on SeaORM 2.0.4. It adds what an ORM does not
supply: declarative validations, an eager load that counts parents, hook-safe
bulk writes, N+1 detection, dirty tracking, a test harness that never commits,
change publishing and fixtures. Postgres is the only database (2026-11-11):
RLS and one database keep the framework simple. Use this crate when
your application stores rows. Do not use it for an application that stores
nothing: without it, no SQL driver is in the lock file. To write migrations
from the models, see `brocanth-migrate`.

## Getting started

```toml
[dependencies]
brocanth-model = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
```

Features:

| Feature | Default | Adds |
|---|---|---|
| `postgres` | on | the Postgres driver |
| `plugin` | off | `brocanth_model::plugin`, the ORM end of the core's model seam (links `brocanth`) |
| `db` | off | `brocanth_model::Orm`: SeaORM over the pool a `brocanth_db::Db` opened (turns on `postgres`) |

A build without `postgres` fails with a `compile_error!`.

This crate has no `scaffold` feature. The `model` module lives in the CLI
(`crates/brocanth-cli/src/modules/model.rs`). It needs the `db` module:

```sh
brocanth create my-app --with db,model,commands   # a new project
brocanth add model                                # an existing project with db
```

The module writes `src/plugins/model/mod.rs`, `migrations/mod.rs`, and, with
`commands`, the `migrate` command. It turns on `db` and `plugin`.

The plugin opens no pool (PLAN.md ADR 2: the ORM is registered after the
pool, because it needs the pool). In `before_start` it takes the
`brocanth_db::Db` the `db` plugin put into the services, wraps it in an
`Orm`, inserts the `Orm` into the services and puts it on every request's
context. A page reads the connection with `ctx.orm()`. It has no settings:
the URL and the pool are `[db]`'s.

### `Orm`

```rust,ignore
use brocanth_model::Orm;

let orm = Orm::from_db(&db); // shares db's pool; opens nothing
let posts = post::Entity::find().all(&*orm).await?;
let tx = orm.begin().await?; // `Orm` derefs to `DatabaseConnection`
```

A newtype and not an alias, because the services and the request context are
keyed by type, and `ContextItem` cannot be implemented for SeaORM's own
type. `Orm::connection` and `From<Orm> for DatabaseConnection` hand the
connection to an API that names the SeaORM type.

## Examples

### A model with validations

`#[validates(..)]` goes above `#[sea_orm::model]`. It writes the `Validate`
impl and the `before_save` hook that enforces it. An invalid record never
reaches SQL, and `Error::errors` gives the messages per field.

```rust
# mod user {
use brocanth_model::prelude::*;

#[brocanth_model::validates(
    name(presence, length(min = 2, max = 80)),
    email(presence, format(r"^[^@\s]+@[^@\s]+$"), uniqueness),
)]
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "user")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: String,
    #[sea_orm(unique)]
    pub email: String,
}
# }
# use brocanth_model::prelude::*;
# use brocanth_model::testing::Harness;
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let Some(harness) = Harness::isolated().await? else { return Ok(()) }; // no server: nothing to show
# let schema = sea_orm::Schema::new(harness.conn().get_database_backend());
# harness.conn().execute(&schema.create_table_from_entity(user::Entity)).await?;
# let db = harness.begin().await?;

let result = user::ActiveModel {
    name: Set("A".to_owned()),
    email: Set("not an email".to_owned()),
    ..Default::default()
}
.insert_checked(&*db)
.await;

let error = result.expect_err("two rules fail");
assert!(error.is_invalid());
let errors = error.errors().expect("field errors");
assert!(errors.has("name"));
assert!(errors.has("email"));
println!("{}", errors.full_messages().join("; "));
# Ok(())
# }
```

### An eager load that respects `limit`

SeaORM's `.limit(n).find_with_related(..)` limits joined rows, so it can
return fewer than `n` parents. `.eager()` switches to two queries when the
parent set is limited.

```rust
# mod user {
#     use brocanth_model::prelude::*;
#     #[sea_orm::model]
#     #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#     #[sea_orm(table_name = "user")]
#     pub struct Model {
#         #[sea_orm(primary_key)]
#         pub id: i32,
#         pub name: String,
#         #[sea_orm(has_many)]
#         pub posts: HasMany<super::post::Entity>,
#     }
#     impl ActiveModelBehavior for ActiveModel {}
# }
# mod post {
#     use brocanth_model::prelude::*;
#     #[sea_orm::model]
#     #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#     #[sea_orm(table_name = "post")]
#     pub struct Model {
#         #[sea_orm(primary_key)]
#         pub id: i32,
#         pub user_id: i32,
#         pub title: String,
#         #[sea_orm(belongs_to, from = "user_id", to = "id")]
#         pub author: BelongsTo<super::user::Entity>,
#     }
#     impl ActiveModelBehavior for ActiveModel {}
# }
use brocanth_model::prelude::*;
use brocanth_model::EagerStrategy;
# use brocanth_model::testing::Harness;
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let Some(harness) = Harness::isolated().await? else { return Ok(()) }; // no server: nothing to show
# let schema = sea_orm::Schema::new(harness.conn().get_database_backend());
# harness.conn().execute(&schema.create_table_from_entity(user::Entity)).await?;
# harness.conn().execute(&schema.create_table_from_entity(post::Entity)).await?;
# let db = harness.begin().await?;
# for u in 1..=3 {
#     user::ActiveModel { id: Set(u), name: Set(format!("user {u}")), ..Default::default() }.insert(&*db).await?;
#     for p in 0..2 {
#         post::ActiveModel { user_id: Set(u), title: Set(format!("post {p}")), ..Default::default() }.insert(&*db).await?;
#     }
# }

let query = user::Entity::find().eager().limit(2);
assert_eq!(query.strategy(), EagerStrategy::TwoQuery);

let users = query.with_related::<post::Entity, _>(&*db).await?;
assert_eq!(users.len(), 2); // two parents, whatever the number of posts
assert_eq!(users[0].related().len(), 2);
# Ok(())
# }
```

### Dirty tracking

`Tracked` keeps the loaded row beside the `ActiveModel`. It derefs to the
`ActiveModel`, so assignment reads as in SeaORM.

```rust
# mod user {
#     use brocanth_model::prelude::*;
#     #[sea_orm::model]
#     #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#     #[sea_orm(table_name = "user")]
#     pub struct Model {
#         #[sea_orm(primary_key)]
#         pub id: i32,
#         pub name: String,
#         pub email: String,
#     }
#     impl ActiveModelBehavior for ActiveModel {}
# }
use brocanth_model::prelude::*;

# fn main() {
let row = user::Model {
    id: 1,
    name: "Grace".to_owned(),
    email: "grace@example.test".to_owned(),
};
let mut tracked = Tracked::<user::ActiveModel>::load(row);
assert!(!tracked.is_changed());

tracked.name = Set("Ada".to_owned());
tracked.email = Set("grace@example.test".to_owned()); // the same value

assert_eq!(tracked.changed(), ["name"]);
assert_eq!(tracked.was("name"), Some("Grace".into()));
# }
```

`tracked.save(db)` writes the record through the checked path, so the
validations run.

### A bulk write that runs the hooks

`update_many` skips `before_save`, so it skips validation. `bulk::update_where`
loads each row and writes it back through the hook, in one transaction. The
escape hatch, `bulk::unchecked`, takes a `HooksBypassed` with a written reason.

```rust,no_run
# mod post {
#     use brocanth_model::prelude::*;
#     #[brocanth_model::validates(status(inclusion("draft", "published")))]
#     #[sea_orm::model]
#     #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#     #[sea_orm(table_name = "post")]
#     pub struct Model {
#         #[sea_orm(primary_key)]
#         pub id: i32,
#         pub status: String,
#     }
# }
use brocanth_model::prelude::*;
use sea_orm::sea_query::IntoCondition;

# async fn archive(db: &sea_orm::DatabaseConnection) -> Result<(), brocanth_model::Error> {
let refused = bulk::update_where::<post::Entity, _, _>(
    post::Column::Status.eq("published").into_condition(),
    |p| p.status = Set("archived".to_owned()),
    db,
)
.await;
assert!(refused.is_err()); // "archived" fails the inclusion rule
# Ok(())
# }
# fn main() {}
```

`no_run`: the block needs a database connection. The same flow runs in
`examples/blog.rs` and `tests/blog.rs`.

## Testing

```sh
cargo test -p brocanth-model
```

With no server, the tests that need no database run, and the database tests
skip and say so on stderr. To run them all:

```sh
BROCANTH_TEST_DATABASE_URL=postgres://localhost/brocanth_test cargo test -p brocanth-model
```

`BROCANTH_MODEL_TEST_POSTGRES` overrides `BROCANTH_TEST_DATABASE_URL` for this
crate only. Most tests get a schema of their own; `tests/postgres.rs` shares
one migrated schema and keeps the tests apart with a transaction.

The worked example runs end to end and prints what it proves. It reads the
same two variables, then `DATABASE_URL`, and works in a schema that it drops
at the end. With no URL it prints a line and exits:

```sh
DATABASE_URL=postgres://localhost/brocanth_example cargo run -p brocanth-model --example blog
```

### Testing an application that uses this crate

Use `brocanth_model::testing::Harness`:

- `Harness::isolated()` or `Harness::isolated_migrated::<Migrator>()` gives
  each test an empty schema of its own. The schema is dropped when the
  harness drops.
- `Harness::postgres_migrated_from_env::<Migrator>()` migrates one shared
  schema once, under an advisory lock.
- Both read the same two variables and return `None` with no server. Call
  `testing::skipped(..)` then, so the skip is visible.
- `harness.begin()` returns a `TestTransaction`. It has no `commit`, so a
  test's rows never leak into the next test.
- `Fixtures` loads a known dataset from a JSON file through the model hooks.

For a screen that reads models, drive the application with `brocanth-test`'s
`TestClient` and assert on the HTML it returns.

## More

- The crate documentation (`cargo doc -p brocanth-model --open`): one section
  per capability, with the reasons.
- `docs/migrations.md`: stable ids and `brocanth migrate`.
- `docs/module-flags.md`: how `[db] url` is set, and why it is a secret.
- `PLAN.md` §10 (publishing) and §11 (migrations).
