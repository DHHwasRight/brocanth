# `middleware-rate-limit`

A fixed window per address, session or user, per group of routes; 429 with Retry-After.

The wizard calls it: *Limit how often one caller may hit a route*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`middleware`](middleware.md), off until you tick it |
| Needs | none |
| After | [`middleware-security-headers`](middleware-security-headers.md) |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no rate limit; a password guesser gets as many tries as the server answers |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `middleware-rate-limit brocanth_middleware::RateLimit`
- src/plugins/mod.rs (the hook record): `brocanth_middleware::RateLimit`
- AGENTS.md: `## Rate limit`

The hook config is a literal in code, also with `config`. It holds one example rule to replace.

## Settings

None.

## Read more

- [Middleware](../middleware.md)
- Source: [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
