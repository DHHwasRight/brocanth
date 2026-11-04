# `db`

One Postgres pool and sea-query statements, read as `ctx.db()`; without it no SQL driver.

The wizard calls it: *A Postgres pool, and SQL built with sea-query*.

| | |
|---|---|
| Categories | **Enhancer**, Middleware, Renderer |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-db` (feature `plugin`) |
| Without it | no database pool; a page that needs SQL has nowhere to send it |

## What it writes

Files:

- `src/plugins/db/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod db;`
- src/plugins/mod.rs (the provider list): `db crate::plugins::db::Database`
- src/plugins/mod.rs (the context): `let db: &brocanth_db::Db = cx.extract();`
- src/pages/mod.rs: `impl Ctx { pub fn db(&self) -> &brocanth_db::Db }`
- config/<env>.toml: `[db]`
- AGENTS.md: `## The database`

## What the plugin does

- `before_start` opens the pool with `Db::connect`, so a wrong URL or
  password stops the start. It registers `Db::ping` as the readiness check.
- It inserts the `Db` into the application's services. A plugin registered
  after it reads the same pool with
  `lc.require_service::<brocanth_db::Db>()`.
- `provide` puts the `Db` on every request's context. A page reads it with
  `ctx.db()`. A `Ctx` built by hand has none, and `ctx.db()` panics with a
  `help:` line.
- `shutdown` closes the pool inside what is left of the grace period.

A `brocanth_db::DbError` goes through `?` in a page: the generated `Error`
takes any `std::error::Error` as a 500. The message goes to the log, and the
client gets `Internal Server Error`.

## Settings

Section `[db]`. Environment variable: `$<APP>_DB__<KEY>`. Flag: `--db.<key>`. Rename a flag in `src/flags.rs`.

A duration is a whole number of seconds. For `idle_timeout` and
`statement_timeout`, 0 turns it off.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `url` | text | `"postgres://localhost/development"` | none (secret) | The Postgres URL: postgres://user:password@host:5432/database. |
| `max_connections` | integer | `10` | `--db.max_connections` | The most connections the pool opens. |
| `min_connections` | integer | `0` | `--db.min_connections` | The connections the pool keeps open when idle. |
| `acquire_timeout` | integer | `30` | `--db.acquire_timeout` | Seconds a statement waits for a connection, opening one included. |
| `idle_timeout` | integer | `600` | `--db.idle_timeout` | Seconds before an idle connection closes. 0 keeps it open. |
| `statement_timeout` | integer | `0` | `--db.statement_timeout` | Seconds before the server cancels a statement. 0 is no limit. |
| `search_path` | list | `[]` | none (list) | The schemas for a table name with no schema. Empty is the server's default. |
| `read_only` | bool | `false` | `--db.read_only` | Every transaction is read-only: for a replica, or a reporting process. |
| `application_name` | text | the project name | `--db.application_name` | The name pg_stat_activity shows. Empty sends none. |
| `ca_file` | text, optional | commented out | `--db.ca_file` | A PEM bundle to trust, with verify-full. Empty means none. |

## Read more

- [brocanth-db](../../crates/brocanth-db/README.md)
- Source: [`crates/brocanth-db/src/scaffold.rs`](../../crates/brocanth-db/src/scaffold.rs)
