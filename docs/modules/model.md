# `model`

SeaORM, validations, migrations, over the `db` pool.

The wizard calls it: *An ORM with validations and migrations*.

| | |
|---|---|
| Categories | **Enhancer**, Middleware, Renderer |
| Wizard question | Data |
| Default | off |
| Presets | HTML application, JSON API, Something that does work |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-model` (features `db`, `plugin`), `brocanth-migrate` (the `migrate` tool) |
| Without it | no ORM, no sea-orm anywhere in the lock file |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/model.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | `PgPool` (a Postgres pool): the ORM runs over the pool the `db` plugin opens, and without `db` there is none; providers: [`db`](db.md) |
| Uses | none |
| Adapts to | [`commands`](commands.md): a `migrate` subcommand is written |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Files:

- `migrations/mod.rs`
- `src/plugins/model/mod.rs`
- `src/bin/migrate.rs`: the `migrate` tool, a second binary: `ids`, `make`
  and `sync`. It parses `src/models/`, so the server binary does not link
  it. `cargo run --bin migrate -- make add_posts`
- `src/commands/migrate.rs`, with `commands`: `up`, `down [N]` and
  `status`, over the pool `[db]` configures, and `ids`, `make` and `sync`,
  which run the tool

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod model;`
- src/plugins/mod.rs (the provider list): `model crate::plugins::model::Model`
- src/pages/mod.rs: `impl Ctx { pub fn orm(&self) -> &brocanth_model::sea_orm::DatabaseConnection }`

## What the plugin does

It opens no pool. ADR 2: the ORM is registered after the pool, because it
needs the pool.

- `before_start` reads the pool the `db` plugin opened,
  `lc.require_service::<brocanth_db::Db>()`, and wraps it in
  `brocanth_model::Orm`. It opens no connection.
- It inserts the `Orm` into the application's services. A plugin
  registered after it reads it with `lc.require_service::<Orm>()`.
- `provide` puts the `Orm` on every request's context. A page reads the
  SeaORM connection with `ctx.orm()`. A `Ctx` built by hand has none, and
  `ctx.orm()` panics with a `help:` line.

An entity query through `ctx.orm()` and a sea-query statement through
`ctx.db()` use the same connections and the same session settings.

## Settings

None. The URL and the pool are `[db]`'s. A leftover `[model] url` is an
unknown key, and the application refuses to start.

## Read more

- [Migrations](../migrations.md)
- [brocanth-model](../../crates/brocanth-model/README.md)
- Source: [`crates/brocanth-model/src/scaffold.rs`](../../crates/brocanth-model/src/scaffold.rs)
