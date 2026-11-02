# brocanth-model

The model layer of brocanth, on SeaORM 2.0.4. It adds what an ORM does not
supply: declarative validations, an eager load that counts parents, hook-safe
bulk writes, N+1 detection, dirty tracking, a test harness that never commits,
change publishing and fixtures. Postgres is the default driver. SQLite is the
smaller target, for one machine with no database server. Use this crate when
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
| `sqlite` | off | the SQLite driver |
| `plugin` | off | `brocanth_model::plugin`, the ORM end of the core's model seam (links `brocanth`) |

A build needs one driver. A build with neither fails with a `compile_error!`.
For SQLite alone:

```toml
brocanth-model = { git = "https://github.com/DHHwasRight/brocanth", branch = "main", default-features = false, features = ["sqlite"] }
```

This crate has no `scaffold` feature. The `model` module lives in the CLI
(`crates/brocanth-cli/src/modules/model.rs`):

```sh
brocanth create my-app --with model,commands   # a new project
brocanth add model                             # an existing project
```

The module writes `src/plugins/model/mod.rs`, `migrations/mod.rs`, and, with
`commands`, the `migrate` command. It turns on both `postgres` and `sqlite`,
because the development database is a SQLite file.

Settings, section `[model]`:

| Key | Kind | Default | Flag | Environment |
|---|---|---|---|---|
| `url` | text | `sqlite://db/development.sqlite?mode=rwc` | none: secret | `$<APP>_MODEL__URL` |

`url` is a secret. A database URL can carry a password, and every user on the
host can read a process's arguments. So no `--model.url` flag exists. Set it
in the environment or, with the `config` module, in `config/<env>.toml`. The
settings report prints `(secret)` for it. See `docs/module-flags.md`.

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
# let harness = Harness::sqlite().await?;
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
# let harness = Harness::sqlite().await?;
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

The suite runs on in-memory SQLite with no server. The crate depends on
itself as a dev-dependency with `sqlite` and `plugin` on, so a test build
always has the SQLite harness.

The Postgres tests skip, and say so on stderr, when no database is set. To run
them:

```sh
BROCANTH_TEST_DATABASE_URL=postgres://localhost/brocanth_test cargo test -p brocanth-model
```

`BROCANTH_MODEL_TEST_POSTGRES` overrides `BROCANTH_TEST_DATABASE_URL` for this
crate only. The worked example runs end to end and prints what it proves:

```sh
cargo run -p brocanth-model --example blog
```

### Testing an application that uses this crate

Use `brocanth_model::testing::Harness`:

- `Harness::sqlite()` or `Harness::migrated::<Migrator>()` gives an empty
  in-memory database per test.
- `Harness::postgres_migrated_from_env::<Migrator>()` reads the same two
  variables, migrates once under an advisory lock, and returns `None` with no
  server. Call `testing::skipped(..)` then, so the skip is visible.
- `harness.begin()` returns a `TestTransaction`. It has no `commit`, so a
  test's rows never leak into the next test.
- `Fixtures` loads a known dataset from a JSON file through the model hooks.

For a screen that reads models, drive the application with `brocanth-test`'s
`TestClient` and assert on the HTML it returns.

## More

- The crate documentation (`cargo doc -p brocanth-model --open`): one section
  per capability, with the reasons.
- `docs/migrations.md`: stable ids and `brocanth migrate`.
- `docs/module-flags.md`: how `[model] url` is set, and why it is a secret.
- `PLAN.md` §10 (publishing) and §11 (migrations).
