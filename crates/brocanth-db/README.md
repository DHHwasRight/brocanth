# brocanth-db

The database layer: one Postgres pool, the session every connection starts
with, and [sea-query](https://docs.rs/sea-query) statements run through
[sqlx](https://docs.rs/sqlx). No entities, no models. For models with hooks,
use `brocanth-model` (SeaORM) on the same pool.

## Getting started

```toml
[dependencies]
brocanth-db = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
```

| Feature | Default | Adds |
|---|---|---|
| `runtime` | on | the pool and the statements: everything but `scaffold`. `serde` and `plugin` turn it on |
| `serde` | off | `DbConfig: Deserialize`, for a `[db]` config section |
| `plugin` | off | `Db: ContextItem`, so a plugin puts the pool on the request context |
| `scaffold` | off | the `db` module of `brocanth create`; for the CLI only |

`brocanth_db::sea_query` and `brocanth_db::sqlx` are re-exports. You do not
need a second dependency line for either.

### In a generated project

`brocanth create --modules db` or `brocanth add db` writes
`src/plugins/db/mod.rs` and a `[db]` section. The plugin opens the pool in
`before_start`, registers `Db::ping` as the readiness check, inserts the
`Db` into the application's services, puts it on every request's context,
and closes it on shutdown. A page reads it with `ctx.db()`:

```rust,ignore
use brocanth_db::sea_query::Query;
use brocanth_db::Exec;

use crate::pages::{Ctx, Error, IntoResponse};

pub async fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let q = Query::select().columns(["id", "title"]).from("posts").to_owned();
    let posts: Vec<(i64, String)> = ctx.db().all(&q).await?;
    Ok(format!("{} posts", posts.len()))
}
```

A `DbError` goes through `?` as a 500: the message goes to the log, not to
the client. Another plugin, registered after `db`, reads the same pool with
`lc.require_service::<brocanth_db::Db>()`. The settings are in
[docs/modules/db.md](../../docs/modules/db.md).

## Example

```rust,no_run
use std::time::Duration;

use brocanth_db::sea_query::{Expr, ExprTrait, Order, Query};
use brocanth_db::{Db, DbConfig, DbError, Exec};

# async fn example() -> Result<(), DbError> {
let mut cfg = DbConfig::new("postgres://app:secret@localhost/app");
cfg.search_path = vec!["app".into()];
cfg.statement_timeout = Some(Duration::from_secs(5));
let db = Db::connect(&cfg).await?;

// A connection for this statement only, then back to the pool.
let q = Query::select()
    .columns(["id", "title"])
    .from("posts")
    .and_where(Expr::col("published").eq(true))
    .order_by("id", Order::Desc)
    .limit(10)
    .to_owned();
let rows: Vec<(i64, String)> = (&db).all(&q).await?;
let as_json = (&db).json(&q).await?; // [{"id": 3, "title": "…"}, …]

// Commit on Ok, roll back on Err.
db.transact(async |tx| {
    let insert = Query::insert()
        .into_table("posts")
        .columns(["title"])
        .values_panic(["Hello".into()])
        .to_owned();
    tx.execute(&insert).await?;
    Ok(())
})
.await?;

db.ping().await?; // for a readiness probe
# let _ = (rows, as_json);
# Ok(())
# }
```

## Settings

| Field | Default | Effect |
|---|---|---|
| `url` | required | `postgres://`, `postgresql://`, or SQLAlchemy `postgresql+driver://` |
| `max_connections` | 10 | the pool size |
| `min_connections` | 0 | connections kept open when idle |
| `acquire_timeout` | 30 s | the wait for a connection, opening included |
| `idle_timeout` | 10 min | an idle connection closes after this |
| `max_lifetime` | 30 min | a connection is replaced after this |
| `statement_timeout` | none | the server cancels a longer statement (`57014`) |
| `search_path` | server default | schemas for unqualified names |
| `read_only` | false | every transaction is read-only (`25006` on a write) |
| `application_name` | none | the name in `pg_stat_activity` |
| `ca_file` | none | trust this PEM bundle, with `verify-full` |

With the `serde` feature, a duration is a number of seconds (`30`, `0.5`) or
a string with a unit (`"250ms"`, `"30s"`, `"5m"`, `"1h"`). For an optional
duration, `0` turns it off.

`Debug` of a `DbConfig` hides the password. No `DbError` contains the URL.

## Design decisions

**sea-query and sqlx 0.9, not a second ORM.** SeaORM is the model layer
(PLAN.md §7). Some code needs SQL without a model: a report, an analytics
query, a table that belongs to another service. sea-query is SeaORM's own
query builder. So the two layers share one SQL vocabulary, one value type
and one driver. sqlx 0.9 and sea-query 1.0 are the versions `sea-orm` 2.0.4
resolves to, so this crate adds no second copy of either to the lock file.

**A connection per statement, never per request.** A `Db` statement takes a
connection and gives it back at once. A handler that awaits something slow
between two queries holds nothing while it waits. So the pool size bounds
concurrent statements, and a slow upstream cannot drain the pool. A
transaction holds one connection, and it says so: `transact` or `begin`.

**Session settings on the connection, not on the query.** `search_path`,
`statement_timeout`, `default_transaction_read_only` and `application_name`
go into the startup packet. No query pays a `SET`, and no failed transaction
leaves a setting behind for the next caller.

**`Exec` takes `&mut self`.** A transaction is one connection, and sqlx
needs `&mut` for it. A pool ignores the `mut`. So one helper,
`async fn f(db: &mut impl Exec)`, runs on a `Db` or inside a `Tx`.

**`transact` takes an async closure.** `db.transact(async |tx| { … })`: no
`Box::pin`, no lifetime annotation. Inside a `Tx`, `transact` and `begin`
use a `SAVEPOINT`, so an inner failure leaves the outer transaction usable.

**`json` lets Postgres encode.** The statement runs as
`WITH q AS (…) SELECT row_to_json(q)::text FROM q`. Postgres writes each
type as it knows it (`numeric` exact, `timestamptz` ISO 8601, `jsonb`
nested), and `INSERT … RETURNING` works. `raw_json` does the same for SQL
text that was built elsewhere.

**No `connect_timeout`.** sqlx 0.9 has none for Postgres and ignores one in
the URL. `acquire_timeout` bounds the wait, and the wait includes the
connect.

**SeaORM shares the pool.** `brocanth-model` wraps the same pool:

```rust,ignore
let conn = SqlxPostgresConnector::from_sqlx_postgres_pool(db.pool().clone());
```

One set of connections, one set of session settings, one place to size.

## Tests

The Postgres tests read `BROCANTH_DB_TEST_POSTGRES`, then
`BROCANTH_TEST_DATABASE_URL`. Each test creates its own schema and drops it.
Without a URL, each test prints `!! SKIPPED` and passes.

```sh
BROCANTH_TEST_DATABASE_URL=postgres://postgres:pw@localhost:55432/postgres \
  cargo test -p brocanth-db
```
