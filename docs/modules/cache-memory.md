# `cache-memory`

The cache in this process's memory, for tests and development; never in production.

The wizard shows it under the cache, in the Data question.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Data |
| Default | off |
| Presets | none |
| Part of | [`cache`](cache.md), off until you tick it |
| Needs | none |
| After | [`cache`](cache.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`cache-redis`](cache-redis.md): a project has one cache backend, and both write `src/plugins/cache/mod.rs`; [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Development only | yes: `brocanth check --deploy` reports `deploy/development-only` |
| Crates | `brocanth-cache` (`MemoryCache`; the parent's dependency, without `postgres`) |
| Without it | the cache is in Postgres, or on Redis with `cache-redis` |

`MemoryCache` passes the same conformance suite as `PgCache` and
`RedisCache` (`brocanth-cache-conformance`). So a test that runs on it
runs the same code a deploy runs on Postgres.

It is for tests and development only. Each process has its own map, an
invalidation reaches only the process that made it, and a restart empties
it. `brocanth check --deploy` reports it as an error, with
`help: run brocanth remove cache-memory`. A deploy that is not production
passes `--env development`, `--env test`, or
`--allow deploy/development-only`.

## What it writes

Files:

- `src/plugins/cache/mod.rs`, over `MemoryCache`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod cache;`
- src/plugins/mod.rs (the provider list): `cache crate::plugins::cache::Cache`
- config/<env>.toml: `[cache]`

## Settings

Section `[cache]`. Environment variable: `$<APP>_CACHE__<KEY>`. Flag: `--cache.<key>`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `sweep_every` | integer | `300` | `--cache.sweep_every` | Seconds between two sweeps of expired entries; 0 turns the sweep off. |

## Read more

- [Cache](../cache.md)
- Source: [`crates/brocanth-cache/src/scaffold.rs`](../../crates/brocanth-cache/src/scaffold.rs), [`crates/brocanth-cache/src/memory.rs`](../../crates/brocanth-cache/src/memory.rs)
