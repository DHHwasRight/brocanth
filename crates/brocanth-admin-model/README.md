# brocanth-admin-model

The data source that lets `brocanth-admin` read and write SeaORM entities.
One macro, `admin_source!`, takes one line per entity and writes a type that
implements `brocanth_admin::AdminData`. Every write loads or builds an
`ActiveModel` and goes through SeaORM, so the hooks run, and with them the
validations of `brocanth-model`. Use it when the application stores its rows
with `brocanth-model`. Do not use it when the rows live somewhere else:
implement `AdminData` yourself. The crate exists apart from
`brocanth-admin` so that an admin without SeaORM does not link the ORM.

## Getting started

```toml
[dependencies]
brocanth-admin = { git = "https://github.com/DHHwasRight/brocanth.git" }
brocanth-admin-model = { git = "https://github.com/DHHwasRight/brocanth.git" }
serde = { version = "1", features = ["derive"] }
```

The crate has no features. It enables SeaORM's `with-json` itself.

Each entity's `Model` must derive `Serialize` and `Deserialize`.
`ActiveModel::set_from_json` and `Select::into_json` are the bridge between
the dynamic admin and the static ORM, and both need serde.

### In a generated project

`brocanth create` lists the module `admin-model` and blocks it: the
generated `model` plugin opens no connection pool, so there is nothing to
read. Write the source by hand over your own `DatabaseConnection` and pass
it to `Admin::new` in `src/plugins/admin/mod.rs`, in place of the
`Fixture`. The module has no settings.

## Examples

### One line per entity

```rust
use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::{Admin, AdminConfig, Discovery, Principal, Submission};
use brocanth_admin_model::admin_source;
use brocanth_model::sea_orm::ConnectionTrait;
use brocanth_model::testing::Harness;

mod blog {
    pub mod user {
        use brocanth_model::prelude::*;
        use serde::{Deserialize, Serialize};

        #[sea_orm::model]
        #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
        #[sea_orm(table_name = "user")]
        pub struct Model {
            #[sea_orm(primary_key)]
            pub id: i32,
            pub name: String,
        }

        // `#[brocanth_model::validates(..)]` writes this impl with the
        // validations in it. An entity without rules writes it empty.
        impl ActiveModelBehavior for ActiveModel {}
    }
}

admin_source! {
    /// Every model this admin shows.
    pub struct Models;
    "blog::user" => blog::user::Entity,
}

# #[tokio::main(flavor = "multi_thread")]
# async fn main() {
// An in-memory SQLite database. An application passes its own connection.
let harness = Harness::sqlite().await.unwrap();
harness
    .conn()
    .execute_unprepared(
        "CREATE TABLE user (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)",
    )
    .await
    .unwrap();
harness
    .conn()
    .execute_unprepared("INSERT INTO user (name) VALUES ('Ada Lovelace')")
    .await
    .unwrap();

let admin = Admin::new(
    Arc::new(Models::new(harness.conn().clone())),
    AdminConfig::new()
        .discovering(Discovery::new().under("blog"))
        // A real application registers a row policy per model with
        // `.resource(..)` and a screen policy with `.with_policy(..)`.
        .unscoped_rows_unsafe_for_development()
        .with_policy(AllowAllUnsafeForDevelopment),
);

struct Anyone;
let response = admin
    .dispatch(Principal::new(&Anyone), "GET", "/admin/user", "", &Submission::new())
    .await
    .unwrap();
assert_eq!(response.status(), 200);
assert!(response.html().unwrap().contains("Ada Lovelace"));
# }
```

The string is the entity's module path, as SeaORM's registry reports it.
`Discovery` filters on it, and the index screen shows it. An entity that is
not listed is not in the admin. That is the right default for a screen with
a delete button.

Inside a Tokio runtime, await `Admin::dispatch`. Do not call
`brocanth_admin::rt::block_on` there: it starts its own executor and blocks
the thread the runtime needs.

### Row policies reach the SQL

With `AdminConfig::resource(..)`, the admin passes a `Scope` to `count`,
`list`, `fetch`, `update` and `delete`. This source splices the condition
into the statement. A read adds it to the `WHERE` clause. A write reads the
row with the condition and `FOR UPDATE` in a transaction, then writes it
through the `ActiveModel`. Zero rows changed is a failure, not a success.
`tests/policy.rs` is the worked example, through the router.

## What it does not do

- **No joins.** Declare a foreign key with `ModelAdmin::foreign_key`. The
  admin resolves it with a second read.
- **No composite-key writes.** A composite key reads. `update` and `delete`
  refuse it by name, because `find_by_id` with a tuple has no dynamic form.

## Testing

```sh
cargo test -p brocanth-admin-model
```

The tests need no environment variable. They run against an in-memory
SQLite database through `brocanth_model::testing::Harness::sqlite`.

- `tests/seaorm.rs`: the six operations, and a write that goes through the
  `ActiveModel`, so a `#[validates]` rule refuses a bad row.
- `tests/policy.rs`: row policies on a real database. It drives the admin
  through the router with `brocanth_test::TestClient`, so the plugin and
  the principal hook are on the path.

An application tests its admin the same way: build the app with
`AdminPlugin`, send requests with `TestClient`, and assert on the status
and on what the page shows.

## Links

- `brocanth-admin`: the screens, the policies, and how to tune them.
- [`docs/backoffice.md`](../../docs/backoffice.md) §2a: the row policies
  on each screen.
- [`docs/admin-customization.md`](../../docs/admin-customization.md): how
  to change what the admin looks like.
