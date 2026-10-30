# Tooling: fixtures, the database shell, deploy checks, prefetch

Four small tools from gap G9 (`docs/framework-gaps.md`). Each one says what it
does, and what it does not do.

## 1. Fixtures (Django D27)

A fixture is a JSON file of records under `fixtures/`. A record names a model
by its table name and sets columns by their database names:

```json
[
  { "model": "user", "fields": { "id": 1, "name": "Ada", "email": "ada@example.com" } },
  { "model": "post", "fields": { "id": 1, "user_id": 1, "title": "Hello",
                                 "slug": "hello", "status": "draft", "views": 0 } }
]
```

`brocanth_model::Fixtures` loads it. You register the entities a fixture may
write:

```rust
use brocanth_model::Fixtures;

let fixtures = Fixtures::new()
    .dir("fixtures")
    .model::<user::Entity>()
    .model::<post::Entity>();
fixtures.load("blog", &db).await?;
```

What it does:

- **It checks the whole file before it writes a row.** An unknown model, an
  unknown column, or a value that does not fit the field's Rust type is a
  problem with a file, a line, a column and a `help:` line. It reports every
  problem in the file, not only the first:

  ```text
  fixtures/blog.json:3:58: record 2 (`user`) has no column `emial`
    = help: did you mean `email`? The columns of `user` are: id, name, email
  ```

- **It writes through the model layer.** Each record becomes the entity's
  `ActiveModel` and goes through `insert`, so `before_save` and every
  `#[brocanth_model::validates]` rule run. Django and Rails skip the hooks for
  a fixture. brocanth does not: a seed that fails a validation is a seed that
  no form can edit.
- **One transaction per file.** If record 7 fails, records 1 to 6 do not stay.
  Records go in file order, so put a row after the rows it refers to.
- **On Postgres, it moves the sequence** of each table where a record set an
  integer key, so the next `INSERT` without a key does not collide.

`Fixtures::check(file, text)` runs the first half with no database. A CI step
can check every fixture file with it.

### In a test

`Harness::begin` gives a transaction that cannot commit. Load into it. The
fixture's transaction becomes a `SAVEPOINT`, and the rows go away at the end
of the test:

```rust
let harness = Harness::migrated::<Migrator>().await?;
let txn = harness.begin().await?;
fixtures.load("blog", &*txn).await?;
```

### From the command line

`brocanth fixtures load blog` gives `fixtures load blog` to the project's own
binary, as `brocanth` does for every command it does not define. The global
`brocanth` cannot know the entities of a project, so the command is in the
project. Put this in `src/commands/fixtures.rs` and add `pub mod fixtures;`
to `src/commands/mod.rs`:

```rust
use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Load a fixture from fixtures/<NAME>.json into the database
#[cmd]
#[derive(clap::Args)]
pub struct Fixtures {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Write every record in fixtures/<NAME>.json, in one transaction
    Load { name: String },
    /// Check fixtures/<NAME>.json against the models, with no database
    Check { name: String },
}

impl Command for Fixtures {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        let set = brocanth_model::Fixtures::new()
            .dir(ctx.root().join("fixtures"))
            .model::<crate::models::user::Entity>();
        let config = brocanth_config::Loader::new(crate::app::APP).load()?;
        let url = config.section::<crate::plugins::model::Settings>("model")?.url;
        tokio::runtime::Runtime::new()?.block_on(async {
            match self.action {
                Action::Load { name } => {
                    let db = brocanth_model::sea_orm::Database::connect(url).await?;
                    let loaded = set.load(&name, &db).await?;
                    println!("{} rows from {name}", loaded.rows());
                }
                Action::Check { name } => {
                    let path = set.path_of(&name);
                    let text = std::fs::read_to_string(&path)?;
                    let records = set.check(&path.display().to_string(), &text)?;
                    println!("{records} records, all of them fit the models");
                }
            }
            Ok(())
        })
    }
}
```

The scaffold does not write this file yet. The model list is the project's,
and the scaffold has no entities to put in it.

Not done: `dumpdata`, and a JSON, array, `Decimal` or `Uuid` column. Those
types are behind SeaORM features that `brocanth-model` does not turn on, and
the error says so.

## 2. `brocanth dbshell` (Django D26)

```text
brocanth dbshell                      # the development database
brocanth dbshell --env production     # [model] url of config/production.toml
brocanth dbshell -- -c 'select 1'     # arguments after -- go to the client
brocanth dbshell --print              # print the command line, run nothing
```

It reads `[model] url` through the same `brocanth_config::Loader` the
application uses, so the files, `$<APP>_ENV` and `$<APP>_MODEL__URL` agree
with what the application sees. Then it replaces itself with the database's
own client:

| URL | Client | Notes |
|---|---|---|
| `postgres://` or `postgresql://` | `psql <url>` | The password goes in `PGPASSWORD`, not on the command line, where `ps` shows it |
| `sqlite://<path>` | `sqlite3 <path>` | A relative path is read from the project root. A file that does not exist is refused, because `sqlite3` would create an empty one without a word |
| `sqlite::memory:` | none | Refused: no other process can open an in-memory database |

The alias is `dbconsole`, the Rails name.

### Why there is no `brocanth shell`

`rails console` and `manage.py shell` evaluate the application's own language
against its loaded classes. Rust has no interpreter, and the types of a
project exist only inside its compiled binary. A REPL such as `evcxr`
compiles each line as a new crate. It cannot see the private modules of a
project, it cannot hold the state of a running application, and it builds
the dependency tree again on the first line. A command named `shell` would
promise a thing it cannot do.

A console has two uses, and each one has an honest tool:

- **Look at the data**: `brocanth dbshell`.
- **Run application code one time**: a project command in
  `src/commands/<name>.rs`, run as `brocanth <name>`. The compiler checks it
  against the project, and it is a file, so you can review it and run it
  again.
