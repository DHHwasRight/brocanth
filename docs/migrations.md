# Migrations from the models

How `brocanth migrate` writes a migration, and why a rename stays a rename.

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
- `brocanth migrate ids` writes them. It also writes `#[brocanth_model::ids]`
  when the model has neither it nor `#[brocanth_model::validates]`. Both
  attributes remove the ids before SeaORM sees them, and refuse an id with
  the wrong shape.
- Do not write an id by hand. Do not copy an id: an id that occurs two times
  is refused.
- To rename a field, change the name and keep the id.
- Relations (`has_many`, `has_one`, `belongs_to`) and `#[sea_orm(ignore)]`
  fields are not columns. They get no id.

## 3. The commands

| Command | Reads | Writes |
|---|---|---|
| `brocanth migrate ids` | `src/models/**/*.rs` | the same files, one new line per missing id |
| `brocanth migrate make <name>` | `src/models/**/*.rs`, `migrations/schema.json` | `migrations/m<UTC>_<name>.rs`, `migrations/schema.json` |
| `brocanth migrate make <name> --snapshot-only` | the same | `migrations/schema.json` only |

`make` applies nothing. The generator proposes and the developer commits.
Use `--snapshot-only` one time in a project whose tables a hand-written
migration already made.

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
- It does not add the migration to a `Migrator`. Add `m<..>::Migration` to
  yours.
- SQLite cannot alter a column. An alter step runs on Postgres only.
- Two fields that swap names in one step make two renames that collide. Make
  that change in two migrations.

## 6. Where the code is

- `crates/brocanth-migrate`: ids, source scan, snapshot, diff, rendering, the
  id stamper. No ORM and no driver. Its tests compile the generated golden
  migrations and run them up and down on SQLite.
- `crates/brocanth-model-derive`: `#[ids]`, and the id removal in
  `#[validates]`.
- `crates/brocanth-cli/src/commands/migrate.rs`: the command.
