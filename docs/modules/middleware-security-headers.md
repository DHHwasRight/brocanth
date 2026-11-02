# `middleware-security-headers`

HSTS, nosniff, a referrer policy, no framing, a CSP that runs dp-swap by hash.

The wizard calls it: *Send the headers a browser reads as policy*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | on in a non-interactive `create` |
| Presets | none |
| Part of | [`middleware`](middleware.md), ticked with its parent |
| Needs | none |
| After | [`middleware-csrf`](middleware-csrf.md) |
| Adapts to | [`admin`](admin.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no security headers; a browser sniffs types, frames the pages and runs any inline script |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `middleware-security-headers brocanth_middleware::SecurityHeaders`
- src/plugins/mod.rs (the hook record): `brocanth_middleware::SecurityHeaders`
- AGENTS.md: `## Security headers`

The hook config is a literal in code, also with `config`. See docs/module-flags.md, open question 1.

## Settings

None.

## Read more

- [Middleware](../middleware.md)
- Source: [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
