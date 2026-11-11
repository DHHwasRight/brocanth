# `oauth-client-memory`

The connected accounts in this process's memory; for tests and development, gone at restart.

| | |
|---|---|
| Categories | **Enhancer** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`oauth`](oauth.md), off until you tick it |
| Needs | [`oauth-client`](oauth-client.md): it replaces the token store of `oauth-client`, and there is nothing to store without it |
| After | [`oauth-client`](oauth-client.md) |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none of its own; `oauth-client` drops the `postgres` feature of `brocanth-oauth-client` |
| Development only | yes: `brocanth check --deploy` reports `deploy/development-only` |

## What it writes

Files: none. The `oauth-client` module reads this key and writes
`MemoryTokenStore::new()` in `src/plugins/oauth_client/mod.rs`, and drops
the `migrate` setting from `[oauth_client]`.

Slots:

- AGENTS.md: `## Connected accounts in memory`

**Why it is development only:** a restart drops every connection, and two
processes do not share one. A person would have to connect again after each
deploy. The tokens stay sealed in memory too: the store sees only sealed
bytes, whatever the store is.

## Read more

- [`oauth-client`](oauth-client.md)
- Source: [`crates/brocanth-oauth-client/src/scaffold.rs`](../../crates/brocanth-oauth-client/src/scaffold.rs)
