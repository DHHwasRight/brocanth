# `middleware-csrf`

A double-submit cookie, checked on every verb that is not a read.

The wizard calls it: *Refuse a form post this application did not hand out*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`middleware`](middleware.md), ticked with its parent |
| Needs | none |
| After | [`middleware`](middleware.md) |
| Adapts to | none |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no CSRF check; any page on any site can post a form here and the browser sends the cookies with it |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `middleware-csrf brocanth_middleware::Csrf`
- src/plugins/mod.rs (the hook record): `brocanth_middleware::Csrf`
- AGENTS.md: `## CSRF`

The hook config is a literal in code, also with `config`. See docs/module-flags.md, open question 1.

## Settings

None.

## Read more

- [Middleware](../middleware.md)
- Source: [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
