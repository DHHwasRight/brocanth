# `cache-redis`

The cache on Redis or Valkey instead of Postgres; one more server to run.

The wizard shows it under the cache, in the Data question.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | [`cache`](cache.md), off until you tick it |
| Conflicts | [`cache-memory`](cache-memory.md): a project has one cache backend, and both write `src/plugins/cache/mod.rs`; [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-cache-redis`; `brocanth-cache` loses `postgres` |
| Without it | the cache is in Postgres; there is no Redis server to run |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/cache_redis.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | [`cache`](cache.md) |
| Provides | `CacheStore` (a cache): the cache, on a Redis or Valkey server |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[cache]`: `url` (secret) |
<!-- end generated: capabilities -->

Use it when the cache is a workload of its own: many writes a second,
reads sent to replicas, or eviction under `maxmemory`. For everything
else, the Postgres cache is one moving part fewer.

`RedisCache` passes the same conformance suite as `PgCache`. The server
expires keys by itself, so the plugin runs no sweep and creates no table.
With `db`, the cache still has its own `url`: it does not live in the
database.

## What it writes

Files:

- `src/plugins/cache/mod.rs`, over `RedisCache`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod cache;`
- src/plugins/mod.rs (the provider list): `cache crate::plugins::cache::Cache`
- config/<env>.toml: `[cache]`

## Settings

Section `[cache]`. Environment variable: `$<APP>_CACHE__<KEY>`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `url` | text | `"redis://localhost:6379"` | none (secret) | The Redis or Valkey URL the cache lives in. |

## Read more

- [Cache](../cache.md)
- [`brocanth-cache-redis`](../../crates/brocanth-cache-redis/README.md)
- Source: [`crates/brocanth-cache-redis/src/scaffold.rs`](../../crates/brocanth-cache-redis/src/scaffold.rs)
