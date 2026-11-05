# Migrations from the models

How the `migrate` command of a project with `model` writes a migration, and
why a rename stays a rename. Section 7 is the other path: plain SQL files,
for a project with `db` and no `model`.

Written 30 October 2026, with `brocanth-migrate`. PLAN.md §11, gap G7.

## 1. The problem

A diff of two structs cannot tell a rename from a drop plus an add. A
generator that guesses "drop plus add" deletes a column of data. A generator
that guesses "rename" renames the wrong column when the developer meant to
replace it.

PocketBase solves this with a stored id on each field. The diff matches by id,
so a new name on a known id is a rename. Our editor is a text editor, so the id
lives in the source text.

## 2. The ids

```rust
#[brocanth_model::ids]
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "post")]
#[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1A")]
pub struct Model {
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1B")]
    #[sea_orm(primary_key)]
    pub id: i32,
    #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1C")]
    pub title: String,
}
```

- Each model and each column field has one id: a ULID, 26 characters.
- `migrate ids` writes them. It also writes `#[brocanth_model::ids]`
  when the model has neither it nor `#[brocanth_model::validates]`. Both
  attributes remove the ids before SeaORM sees them, and refuse an id with
  the wrong shape.
- Do not write an id by hand. Do not copy an id: an id that occurs two times
  is refused.
- To rename a field, change the name and keep the id.
- Relations (`has_many`, `has_one`, `belongs_to`) and `#[sea_orm(ignore)]`
  fields are not columns. They get no id.

## 3. The commands

The `model` module writes them into the project. `brocanth` itself has no
`migrate` command: only a project with an ORM has migrations, and `ids` and
`make` parse Rust source, so the parser stays out of the global CLI.

| Command | Reads | Writes |
|---|---|---|
| `migrate ids` | `src/models/**/*.rs` | the same files, one new line per missing id |
| `migrate make <name>` | `src/models/**/*.rs`, `migrations/schema.json` | `migrations/m<UTC>_<name>.rs`, `migrations/schema.json`, `migrations/mod.rs` |
| `migrate make <name> --snapshot-only` | the same | `migrations/schema.json` only |
| `migrate sync` | `migrations/` | `migrations/mod.rs` |
| `migrate up` | `migrations/`, `[db] url` | the database: each pending migration, oldest first |
| `migrate down [N]` | the same | the database: the newest N applied migrations are rolled back (default 1) |
| `migrate status` | the same | nothing; prints `applied` or `pending` per migration |

How to run them:

- With `commands`: `cargo run -- migrate make add_posts`, or
  `brocanth migrate make add_posts`, which hands over to the project's
  binary like every project command.
- Without `commands`, `ids`, `make` and `sync` still run:
  `cargo run --bin migrate -- make add_posts`. `up`, `down` and `status`
  need `commands` (section 5).

### Two binaries, and why

`ids`, `make` and `sync` run in the `migrate` tool, `src/bin/migrate.rs`,
a second binary of the project whose `main` is `brocanth_migrate::tool::main`.
The application's `migrate ids|make|sync` runs it with
`cargo run --bin migrate`. They parse Rust source with `syn`, and the server
binary has no reason to link a parser. The tool is for development: it
writes source files and never touches a database.

`up`, `down` and `status` run in the application's binary, because the
migrations are compiled into it.

`brocanth-migrate` is a dependency of the whole package, because cargo has
no per-binary dependencies. Only the tool names it, so the server binary
links none of it, and `syn` is already in the build for the derive macros.

`make` applies nothing. The generator proposes and the developer commits.
Use `--snapshot-only` one time in a project whose tables a hand-written
migration already made.

### The list and the apply

`migrations/mod.rs` holds the `Migrator`: one `mod` line and one
`Box::new(<name>::Migration)` per file in `migrations/`, sorted by name. A
name starts with its UTC time, so that order is the order of writing.
`make` and `sync` rewrite the list from the directory, so a migration
written by hand is listed by the next `sync`. The file carries a digest,
like the files `brocanth add` owns. When you edit it, the tool does not
rewrite it: it names the files it does not list.

`up`, `down` and `status` refuse when a file in `migrations/` is not in the
list the binary was built with: the binary would skip it without a word.
The `help:` line says to run `migrate sync`. Before the tool moved, the CLI
rewrote the list before each `up`; a binary cannot, because it is already
compiled when it runs.

`up`, `down` and `status` run `migrate` in the project's own binary, the way
every project command runs: the migrations are compiled into that binary.
The scaffold writes that command as `src/commands/migrate.rs` when the
project has `model` and `commands`. It reads `[db] url` through the
application's own config loader, so `$<APP>_ENV` and `$<APP>_DB__URL`
work. Where there is no `brocanth` (a container, a release job), run the
binary: `<binary> migrate up`.

The command opens the pool from `[db]` the way the `db` plugin does
(`Settings::config`), and wraps it in `brocanth_model::Orm` the way the
`model` plugin does. So `search_path` and the other session settings apply
to the migrations too. The database is Postgres.

`make` refuses, and writes nothing, when:

- a model or a field has no id;
- an id occurs two times;
- a field type has no column type (add `#[sea_orm(column_type = "..")]`);
- the name is not snake case.

## 4. The snapshot and the diff

`migrations/schema.json` records each table and column by id: name, column
type, nullability, primary key, auto increment, uniqueness. Commit it with the
migration. The diff compares it with the models:

| Models against snapshot | Change |
|---|---|
| new table id | create table |
| table id is gone | drop table |
| table id, new name | rename table |
| new field id | add column |
| field id is gone | drop column |
| field id, new name | rename column |
| field id, new type, nullability, key or uniqueness | alter column |

The migration file uses `Alias::new("..")`, not the entity's `Column` enum, so
it still compiles after the model changes again. `down` is the inverse of each
`up` step, in reverse order. A drop remembers the old definition, so its
inverse can add the column again (but not the data).

The file marks each step that needs a check with `// Check:`. Examples: a
`NOT NULL` column with no default on a table with rows, a drop, a type change.

## 5. What it does not do

- It does not diff foreign keys, indexes or default values. Write those by
  hand in the generated file.
- `up` without `commands` cannot run: the application has no command line.
  `brocanth add commands` gives one.
- A project with `db` and no `model` (sea-query, no ORM) has no models to
  diff, so no `make`. It gets plain SQL migrations instead (section 7).
- SQLite cannot alter a column. An alter step runs on Postgres only.
- Two fields that swap names in one step make two renames that collide. Make
  that change in two migrations.

## 6. Where the code is

- `crates/brocanth-migrate`: ids, source scan, snapshot, diff, rendering, the
  id stamper. No ORM and no driver. Its tests compile the generated golden
  migrations and run them up and down on SQLite.
- `crates/brocanth-model-derive`: `#[ids]`, and the id removal in
  `#[validates]`.
- `crates/brocanth-migrate/src/tool.rs`: the `migrate` tool (`ids`, `make`,
  `sync`). `src/migrator.rs`: the `migrations/mod.rs` list and its digest.
- `crates/brocanth-cli/src/modules/model.rs`: the project's
  `src/commands/migrate.rs`, `src/bin/migrate.rs` and the first, empty
  list. `crates/brocanth-migrate/tests/golden/migrator-empty.rs` holds the
  CLI and the tool to the same bytes.

## 7. SQL migrations, for `db` without `model`

Written 5 November 2026. A project with `db` and `commands` and no `model`
gets a `migrate` command over plain SQL files. The code is
`crates/brocanth-db/src/migrate.rs`; the command is written by
`crates/brocanth-db/src/scaffold.rs`.

### The file

`migrations/<YYYYMMDD_HHMMSS>_<name>.sql`, one file per migration. The
statements before a line `-- down` apply it. The statements after that line
undo it.

```sql
CREATE TABLE posts (id bigserial PRIMARY KEY, title text NOT NULL);

-- down
DROP TABLE posts;
```

A file with no statement after `-- down` (comments do not count) cannot be
rolled back. The stamp has the format the model path writes, so a reader
sorts both kinds the same way.

### The commands

| Command | Does |
|---|---|
| `migrate new <name>` | writes an empty file with the UTC stamp; refuses a name that is not snake case |
| `migrate up` | applies each pending file, oldest first |
| `migrate down [N]` | runs the `-- down` part of the newest N applied files (default 1) |
| `migrate status` | prints `applied`, `pending` or `no file` per migration |

They read `[db]` the way the server does, as the model's command does. Run
them as `brocanth migrate up`, `cargo run -- migrate up`, or
`<binary> migrate up` in a container.

### How it applies

- Each file runs in one transaction, with its row in `brocanth_migrations`
  (`name`, `applied_at`). A failed statement keeps nothing of that file,
  and the run stops. The files before it stay applied.
- `up` and `down` hold a Postgres advisory lock, so two runs cannot apply
  the same file.
- `down` checks every step before it undoes one. A missing file or a
  missing `-- down` part stops it before any change, with a `help:` line.
- A statement that Postgres refuses in a transaction, such as
  `CREATE INDEX CONCURRENTLY`, cannot run here. Run it by hand.
- The table is created in the first schema of `search_path`.

### Decisions

1. **Plain SQL, not sea-query statements.** A migration is a schema change,
   and SQL is the language of a schema change. sea-query would need a Rust
   file per migration, a list of them, and a build, which is the model
   path's cost without the model path's diff.
2. **Read at run time, not built into the binary.** The files need no list
   to keep in step and no rebuild. A dropped-in file is applied. The cost:
   the files must be next to the binary. The Dockerfile copies
   `migrations/` into the image, as it copies `config/`, and makes it empty
   when there is none.
3. **One file with a `-- down` line, not two files.** The two halves are
   read and reviewed together. A forgotten down is visible in one place.
4. **Its own table, `brocanth_migrations`, not SeaORM's
   `seaql_migrations`.** SeaORM refuses an applied row that has no
   migration in its list. A shared table would break the day the project
   adds `model`.
5. **One `migrate` command per project.** With `model`, the db module
   writes no `migrate` command: the model's command is the one. The model's
   `up`, `down` and `status` refuse while `migrations/` holds a `.sql`
   file. The `help:` line says what to do: record the tables the SQL files
   made with `migrate make <name> --snapshot-only`, then move the `.sql`
   files out of `migrations/`.
6. **No checksum.** An applied file that changed is not detected. Do not
   edit an applied file: write a new one. A checksum is a later step if
   this bites.
7. **No command without `commands`.** As for the model path: a project
   with no command line has nowhere to put one. `brocanth add commands`
   gives one.
