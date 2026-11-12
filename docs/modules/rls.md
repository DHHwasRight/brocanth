# `rls`

Postgres row-level security declared in Rust; `rls make` writes the migration.

The wizard calls it: *Row-level security: Postgres hides the rows of other tenants*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | [`db`](db.md): the policies live in the database the `db` module opens, and `Db::transact_as` sets the principal; [`commands`](commands.md): the migration is written by `rls make`, a command of the project's binary that runs `schema()` |
| After | [`db`](db.md), [`commands`](commands.md), [`model`](model.md) |
| Adapts to | [`model`](model.md), [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-rls` |
| Without it | no row-level security; a query that forgets its WHERE shows every tenant's rows |

The design, the dependency order, the diff rules and the rejected
alternatives are in [`docs/rls.md`](../rls.md).

## What it writes

Files:

- `src/plugins/rls/mod.rs`: `schema()`, the roles, tables, functions and
  policies, in Rust. The project owns it. It starts empty.
- `src/commands/rls.rs`: the `rls` command.

Slots:

- src/plugins/mod.rs: `pub mod rls;`
- src/commands/mod.rs: `pub mod rls;`
- AGENTS.md: `## Row-level security`

## The command

| Command | Does |
|---|---|
| `rls make <name>` | compares `schema()` with `migrations/rls.json`; writes the difference as a migration with its `-- down` part, and the new snapshot. Writes nothing when they are equal |
| `rls sql` | prints the SQL that creates the whole schema |
| `rls check` | compares the database in `[db] url` with `migrations/rls.json`: roles, `ENABLE`/`FORCE`, policy names, `AS`, `FOR`, `TO`, grants, functions. Exits non-zero on a difference |

`rls make` applies nothing. `migrate up` applies the file, in one
transaction, like any other migration.

With [`model`](model.md), the file is a SeaORM migration,
`migrations/m<UTC>_<name>.rs`, that runs the same SQL. Run
`migrate sync` after `rls make` to list it in `migrations/mod.rs`.
Without `model`, it is a SQL file, `migrations/<UTC>_<name>.sql`.

## A request as its identity

```rust
let me = brocanth_db::DbIdentity::new().role("app_member").setting(&crate::plugins::rls::TENANT, tenant_id);
let rows: Vec<(i64, String)> = ctx.db().transact_as(&me, async |tx| tx.all(&q).await).await?;
```

`transact_as` runs `SET LOCAL ROLE` and `set_config('brocanth.<key>', $1,
true)` at the start of the transaction. Both end with it. A statement on
`ctx.db()` outside `transact_as` runs as the login role, and a policy that
reads a setting sees `NULL`.

## What it does not manage

`USAGE` on a schema other than `public`, sequence privileges, column
grants, default privileges, and login roles. Write those in a SQL
migration. Declare a login with `Role::existing` so a policy or a
membership can name it.
