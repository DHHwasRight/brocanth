# brocanth-migrate

The engine behind `brocanth migrate`. It reads model source as text, gives
each table and column a stable id, records a snapshot
(`migrations/schema.json`), compares two snapshots by id and writes a SeaORM
migration file. A diff of two structs cannot tell a rename from a drop plus an
add. A diff by id can: a known id under a new name is a rename. The crate
links no ORM and no SQL driver, because the CLI links it to write a file. Use
it through the `brocanth migrate` commands. Depend on it directly only to
build your own tooling over model source. It does not apply a migration: the
project's own binary does that, through `brocanth-model`.

## Getting started

Most projects never name this crate. The CLI links it, and the `model` module
writes the project side:

```sh
brocanth create my-app --with model,commands
brocanth migrate ids            # write the missing ids into src/models/**/*.rs
brocanth migrate make add_posts # write migrations/m<UTC>_add_posts.rs and schema.json
brocanth migrate up             # apply the pending migrations (needs `commands`)
brocanth migrate status
brocanth migrate down 1
```

`make` applies nothing. It proposes a file, and you read it and commit it.
`up`, `down` and `status` run the `migrate` command in the project's binary.
They read the database URL from `[model] url`, a secret: set it with
`$<APP>_MODEL__URL` or in `config/<env>.toml`, never with a flag. See the
`brocanth-model` README for that setting.

To use the library directly:

```toml
[dependencies]
brocanth-migrate = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
```

The crate has no features, no `scaffold` feature, and no module key of its
own. It is not reachable as `brocanth add <key>`.

## Examples

### Scan a model and take a snapshot

`scan_source` reads the source of a model file. `schema_of` turns the models
into a `Schema`, the content of `migrations/schema.json`.

```rust
use brocanth_migrate::scan::{scan_source, schema_of};

let source = r#"
pub mod post {
    use brocanth_model::prelude::*;

    #[brocanth_model::ids]
    #[sea_orm::model]
    #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
    #[sea_orm(table_name = "post")]
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ20")]
    pub struct Model {
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ21")]
        #[sea_orm(primary_key)]
        pub id: i32,
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ23")]
        pub title: String,
    }
}
"#;

let models = scan_source("src/models/post.rs", source).expect("scan");
let schema = schema_of(&models).expect("every table and column has an id");
assert_eq!(schema.tables[0].name, "post");
assert_eq!(schema.tables[0].fields[1].name, "title");
assert_eq!(schema.tables[0].fields[1].kind, "string");

// The snapshot is JSON, and reads back the same.
let json = schema.to_json();
assert_eq!(brocanth_migrate::Schema::from_json(&json).unwrap(), schema);
```

### A rename stays a rename

```rust
use brocanth_migrate::scan::{scan_source, schema_of};
use brocanth_migrate::{diff, Change, Schema};

fn snapshot(title: &str) -> Schema {
    let source = format!(
        r#"
        #[brocanth_model::ids]
        #[sea_orm::model]
        #[sea_orm(table_name = "post")]
        #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ20")]
        pub struct Model {{
            #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ21")]
            #[sea_orm(primary_key)]
            pub id: i32,
            #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ23")]
            pub {title}: String,
        }}
        "#
    );
    schema_of(&scan_source("post.rs", &source).unwrap()).unwrap()
}

let changes = diff(&snapshot("title"), &snapshot("headline"));
let said: Vec<String> = changes.iter().map(Change::describe).collect();
assert_eq!(said, ["rename column `post.title` to `headline`"]);
assert!(!changes[0].destructive());
```

### Write the migration file

`render` writes the file `make` would write. It names tables and columns with
`Alias::new("..")`, so the file still compiles after the model changes again.
`down` is the inverse of each `up` step, in reverse order.

```rust
use brocanth_migrate::render::render;
use brocanth_migrate::schema::{Field, Table};
use brocanth_migrate::{diff, Schema};

let post = Table {
    id: "01JBQ7R8V4K2M9X3T6N5P0WZ20".into(),
    name: "post".into(),
    fields: vec![Field {
        id: "01JBQ7R8V4K2M9X3T6N5P0WZ21".into(),
        name: "id".into(),
        kind: "integer".into(),
        nullable: false,
        primary_key: true,
        auto_increment: true,
        unique: false,
    }],
};
let changes = diff(&Schema::default(), &Schema::new(vec![post]));
let file = render("m20261030_120000_initial", &changes);
assert!(file.contains("// - create table `post`"));
assert!(file.contains("Alias::new(\"post\")"));
```

### Stamp the missing ids

`stamp` is `brocanth migrate ids`. It adds one line per missing id and leaves
every other byte of the file as it was. A field that has an id keeps it.

```rust
use brocanth_migrate::stamp::stamp;

let source = "\
#[sea_orm::model]
#[sea_orm(table_name = \"tag\")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
}
";
let mut n = 0;
let stamped = stamp("tag.rs", source, || {
    n += 1;
    format!("01JBQ7R8V4K2M9X3T6N5P0WZ{n:02}")
})
.expect("stamp");
assert_eq!(stamped.written, ["tag", "tag.id"]);
assert!(stamped.text.starts_with("#[brocanth_model::ids]\n"));
```

## Testing

```sh
cargo test -p brocanth-migrate
```

No database and no environment variable is needed. `tests/migrations.rs`
diffs `tests/fixtures/v1.rs` against `v2.rs`, checks the text against
`tests/golden/`, then compiles the golden files and runs them up and down on
in-memory SQLite through `brocanth-model`. After a deliberate change to the
generator, rewrite the golden files and read the diff before you commit it:

```sh
BROCANTH_BLESS=1 cargo test -p brocanth-migrate
```

An application tests its migrations through its own binary
(`<binary> migrate up` against a test database), or with
`brocanth_model::testing::Harness::migrated::<Migrator>()` in a test.

## More

- `docs/migrations.md`: the ids, the commands, the diff table, and what the
  generator does not do (foreign keys, indexes, defaults, SQLite alters).
- `PLAN.md` §11: stable field identity.
- `crates/brocanth-cli/src/commands/migrate.rs`: the commands and the
  `migrations/mod.rs` list.
