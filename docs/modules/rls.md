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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-rls` |
| Without it | no row-level security; a query that forgets its WHERE shows every tenant's rows |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/rls.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | [`commands`](commands.md): the migration is written by `rls make`, a command of the project's binary that runs `schema()` |
| After | [`commands`](commands.md), [`model`](model.md) |
| Provides | none |
| Requires | `PgPool` (a Postgres pool): the policies live in the database the `db` module opens, and `Db::transact_as` sets the principal; providers: [`db`](db.md) |
| Uses | none |
| Adapts to | [`model`](model.md): `rls make` writes a SeaORM migration file, not a SQL file; [`admin-model`](admin-model.md): with `policy`, `staff` is the identity the admin runs each request as; without it, AGENTS.md says the admin queries as the login role; [`policy`](policy.md): `schema()` writes the rules of the `#[db]` `PostPolicy` as database policies, and the `RowSecurity` plugin puts the request's `DbIdentity`, built from the policy's `Visitor`, on the context; [`resource`](resource.md): AGENTS.md says how a resource runs each operation as the request's identity |
| Contributes | `AgentsSection` `rls`: how row-level security is declared in Rust and migrated |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

The design, the dependency order, the diff rules and the rejected
alternatives are in [`docs/rls.md`](../rls.md).

## What it writes

Files:

- `src/plugins/rls/mod.rs`: `schema()`, the roles, tables, functions and
  policies, in Rust. The project owns it. It starts empty. With
  [`policy`](policy.md), `schema()` holds `app_user()` (the one role) and
  `.policies(Policy::from_rules::<PostPolicy>(&app_user()))`: the rules of
  the `#[db]` example policy, as database policies on `posts`
  (`docs/policy-rls-bridge.md`). The file also holds the `RowSecurity`
  plugin, `identity(ctx)`, and `agreement_url()`, the database of the
  example's `db { … }` agreement test (`[db] url`; its user must be a
  superuser; `None` when nothing answers there). With `policy`,
  `brocanth-rls` takes its `agreement` feature. With
  [`admin-model`](admin-model.md) too, `staff`.
- `src/commands/rls.rs`: the `rls` command.

Slots:

- src/plugins/mod.rs: `pub mod rls;`; with `policy`, the `RowSecurity`
  registration and its line in the provider list
- src/commands/mod.rs: `pub mod rls;`
- AGENTS.md: `## Row-level security`

## The command

| Command | Does |
|---|---|
| `rls make <name>` | compares `schema()` with `migrations/rls.json`; writes the difference as a migration with its `-- down` part, and the new snapshot. Writes nothing when they are equal |
| `rls sql` | prints the SQL that creates the whole schema |
| `rls diff` | exits non-zero when `schema()` differs from `migrations/rls.json`. Reads no database. [`ci-github`](ci-github.md) runs it |
| `rls check` | compares the database in `[db] url` with `migrations/rls.json`: roles, `ENABLE`/`FORCE`, policy names, `AS`, `FOR`, `TO`, grants, functions. Exits non-zero on a difference |

`rls make` applies nothing. `migrate up` applies the file, in one
transaction, like any other migration.

With [`model`](model.md), the file is a SeaORM migration,
`migrations/m<UTC>_<name>.rs`, that runs the same SQL. Run
`migrate sync` after `rls make` to list it in `migrations/mod.rs`.
Without `model`, it is a SQL file, `migrations/<UTC>_<name>.sql`.

With [`policy`](policy.md), a new project has no migration for the
example policy, so `rls diff` fails until the first `rls make`. The
migration alters `posts`: create that table first, in a migration of its
own, or remove the `.policies(…)` line.

## A request as its identity

```rust
let me = brocanth_db::DbIdentity::new().role("app_member").setting(&crate::plugins::rls::TENANT, tenant_id);
let rows: Vec<(i64, String)> = ctx.db().transact_as(&me, async |tx| tx.all(&q).await).await?;
```

With [`policy`](policy.md), the `RowSecurity` plugin builds the identity
of each request, `brocanth_rls::identity(&app_user(), &visitor)`, and
puts it on the context. A page reads it with
`crate::plugins::rls::identity(ctx)` and passes it to `transact_as`
(`docs/rls.md` §4, "How a request sets its identity").

`transact_as` runs `SET LOCAL ROLE` and `set_config('brocanth.<key>', $1,
true)` at the start of the transaction. Both end with it. A statement on
`ctx.db()` outside `transact_as` runs as the login role, and a policy that
reads a setting sees `NULL`.

With [`model`](model.md), a SeaORM query takes the identity through
`Orm::transact_as(&me, async |txn| …)`, which runs the same statements.

With [`admin-model`](admin-model.md) and [`policy`](policy.md), the
admin's data source runs each request as `crate::plugins::rls::staff`
(`Models::as_identity`): the `Visitor` with `admin` set. A table the admin
shows needs a grant to `app_user` and a policy that admits staff. Without
`policy`, the admin queries the `Orm` with no identity, as the login role,
and on a `FORCE` table it sees no row: call `Models::as_identity` in
`src/plugins/admin/mod.rs` with an identity of your own, or keep the
tables it edits out of row-level security.

## What it does not manage

`USAGE` on a schema other than `public`, sequence privileges other than
`USAGE` on a table's serial sequence (`.sequence(…)`), column grants, default privileges, and login roles. Write those in a SQL
migration. Declare a login with `Role::existing` so a policy or a
membership can name it.
