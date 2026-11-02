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
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-cache` |
| Without it | no cache; every read does the work a hit would have skipped |

## What it writes

Files:

- `src/plugins/cache/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod cache;`
- src/plugins/mod.rs (the provider list): `cache crate::plugins::cache::Cache`
- config/<env>.toml: `[cache]`

## Settings

Section `[cache]`. Environment variable: `$<APP>_CACHE__<KEY>`. Flag: `--cache.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `url` | text | `"postgres://localhost/development"` | none (secret) | The Postgres URL the cache lives in. |

## Read more

- [Cache](../cache.md)
- Source: [`crates/brocanth-cache/src/scaffold.rs`](../../crates/brocanth-cache/src/scaffold.rs)
