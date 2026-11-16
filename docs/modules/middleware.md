# `middleware`

Proxy headers, method override, canonical host, content types, CORS.

The wizard calls it: *The edge hooks: proxies, CORS, a canonical host*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | on in a non-interactive `create` |
| Presets | none |
| Part of | nothing |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-middleware` (feature `config` when the project has `config`) |
| Without it | no edge hooks; a forwarded address, a preflight and a canonical host are each a handler's own problem |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/middleware.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `middleware`: which edge middleware is registered, and where |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | none |
<!-- end generated: capabilities -->

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- AGENTS.md: `## Edge middleware`

## Settings

None.

## Read more

- [Middleware](../middleware.md)
- Source: [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
