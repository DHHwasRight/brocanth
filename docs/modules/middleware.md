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
| Needs | none |
| After | none |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-middleware` (feature `config` when the project has `config`) |
| Without it | no edge hooks; a forwarded address, a preflight and a canonical host are each a handler's own problem |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- AGENTS.md: `## Edge middleware`

## Settings

None.

## Read more

- [Middleware](../middleware.md)
- Source: [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
