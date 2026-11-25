# `storage-db`

The object store is two tables in the project's Postgres database.

The wizard shows it under `storage`, in the Data question.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | [`storage`](storage.md), off until you tick it |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-storage` (feature `postgres`) |
| Without it | another child of `storage` holds the files |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/storage_db.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | `ObjectStore` (an object store): objects in `bytea` columns, on the `db` pool |
| Requires | `PgPool` (a Postgres pool): the objects live in the `db` database, on its pool, and not on one machine's disk; providers: [`db`](db.md) |
| Uses | none |
| Adapts to | none |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it does

It writes no file. `storage` writes `PgStorage` on the `db` pool into
`src/plugins/storage/mod.rs`, behind a `Deferred` store that the plugin
fills at `before_start`, once the pool is open. The objects live in
`brocanth_objects` and `brocanth_object_variants`, as `bytea`, with a
cascade from an object to its variants. The default limit is 32 MiB per
object. Every instance sees every file, and a database backup holds them.

It requires `PgPool`, so `brocanth create <name> --with storage,storage-db` adds
`db`. A project made before the `ObjectStore` Kind with `storage` and `db`
reads as this module, so its files stay in Postgres.

## Settings

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `migrate` | bool | `true` | `--storage.migrate` | Create the two storage tables at startup if they are absent. Development only: put the statements of `brocanth_storage::postgres::sql` in a migration instead. |

## Read more

- [`storage`](storage.md), and the design: [`docs/object-storage.md`](../object-storage.md)
- [brocanth-storage](../../crates/brocanth-storage/README.md)
- Source: [`crates/brocanth-storage/src/scaffold.rs`](../../crates/brocanth-storage/src/scaffold.rs)
