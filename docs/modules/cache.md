# `cache`

A cache on an unlogged Postgres table, not Redis; without it no cache at all.

The wizard calls it: *A cache, in the database you already have*.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Data |
| Default | off |
| Presets | Something that does work |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-cache` (no default features; `postgres` without a backend child, `admin` with `admin`) |
| Without it | no cache; every read does the work a hit would have skipped |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/cache.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | `CacheStore` (a cache): the cache, on an unlogged Postgres table; a child backend replaces the table |
| Requires | `PgPool` (a Postgres pool): the cache is an unlogged table on the project's pool; without a child backend there is nowhere else to keep it; providers: [`db`](db.md); not with [`cache-memory`](cache-memory.md), [`cache-redis`](cache-redis.md) |
| Uses | none |
| Adapts to | [`cache-memory`](cache-memory.md): `cache-memory` writes the plugin file, over this process's memory; [`cache-redis`](cache-redis.md): `cache-redis` writes the plugin file, over a Redis or Valkey server |
| Contributes | `ConfigCheck` `cache`: `deploy/migrate` for `[cache]`, while the cache is on Postgres; `AdminPanel` `cache`, on: a `cache` tab in the admin: purge expired entries, clear a namespace you name |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[cache]`: `migrate`, `sweep_every` |
<!-- end generated: capabilities -->

## Backends

The cache is in Postgres by default. A child module picks another backend
and writes the plugin file instead:

- [`cache-memory`](cache-memory.md): this process's memory, for tests and
  development. `brocanth check --deploy` reports it as an error.
- [`cache-redis`](cache-redis.md): a Redis or Valkey server.

The two children conflict: a project has one cache backend. A child is a
module and not a setting, because a backend changes `Cargo.toml`: with a
child, `brocanth-cache` loses its `postgres` feature.

The Postgres cache requires the project's pool (`PgPool`), so
`brocanth create --with cache` adds [`db`](db.md) and says so. It reuses
the `db` pool and has no `url` of its own: one set of connections to size,
not two against one database. A backend child lifts the requirement.

With [`admin`](admin.md), a tab at `/admin/cache`, in the generated
`plugins::admin_panels`: purge expired entries, and clear each namespace
you name with `.namespace(..)`. `integrate.admin` in `src/modules/cache.rs`
turns it off. The tab reads the cache the plugin opens, whatever the
backend.

## What it writes

Files:

- `src/plugins/cache/mod.rs`, without a backend child (the child writes it)

Slots:

- src/plugins/mod.rs (admin_panels), with `admin`
- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod cache;`
- src/plugins/mod.rs (the provider list): `cache crate::plugins::cache::Cache`
- config/<env>.toml: `[cache]`

## Settings

Section `[cache]`. Environment variable: `$<APP>_CACHE__<KEY>`. Flag: `--cache.<key>`. Rename or hide a flag in `src/modules/cache.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `migrate` | bool | `true` | `--cache.migrate` | Create the unlogged cache table at startup if it is absent. |
| `sweep_every` | integer | `300` | `--cache.sweep_every` | Seconds between two sweeps of expired entries; 0 turns the sweep off. |

The plugin deletes expired rows itself, from `start` to `shutdown`, with a
random quarter of jitter on each wait. See [the sweep](../cache.md#5-the-sweep).

## Read more

- [Cache](../cache.md)
- Source: [`crates/brocanth-cache/src/scaffold.rs`](../../crates/brocanth-cache/src/scaffold.rs)
