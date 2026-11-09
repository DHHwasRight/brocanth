# `middleware-conditional-get`

A weak ETag over every page, and 304 for If-None-Match or If-Modified-Since.

The wizard calls it: *Answer 304 when the browser already has the page*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`middleware`](middleware.md), off until you tick it |
| Needs | none |
| After | [`middleware-rate-limit`](middleware-rate-limit.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no ETag and no 304; a browser downloads an unchanged page again in full |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `middleware-conditional-get brocanth_middleware::ConditionalGet`
- src/plugins/mod.rs (the hook record): `brocanth_middleware::ConditionalGet`
- config/<env>.toml: `[conditional_get]`
- AGENTS.md: `## Conditional GET`

With `config`, the registration reads `[conditional_get]` through `plugin_hooking_from_config`. Without `config`, it starts from `ConditionalGetConfig::default()` and applies the environment and the flags.

## What the hook does

For a `GET` or `HEAD` that answers `200` with a buffered body:

1. With no `ETag` from the page, it sets `W/"<22 characters>"`: 128 bits of the body's SHA-256.
2. `If-None-Match` that names the tag (weak comparison) gives a `304` with no body.
3. With no `If-None-Match`, `If-Modified-Since` at or after the page's `Last-Modified` gives a `304`.

It leaves alone a stream, any other verb or status, and a response with `Cache-Control: no-store`. The page still renders: the hook saves the transfer, not the work.

**Why a weak tag.** The tag is over the identity body, before compression runs. A weak tag stays true for the gzip and the brotli copy of the page, so one tag serves every encoding.

**Order.** The hook runs after the route cache stores a page and before compression. Its `Order` names `route_cache` and `compression`, so this holds in any registration order. A cache hit gets a tag and a `304` like a fresh page.

## The module file

`src/modules/middleware_conditional_get.rs` has no hook switch. The module is one hook, `check`, and it is required: `etag` and `last_modified` turn off each half, and `brocanth remove middleware-conditional-get` turns off the rest.

## Settings

Section `[conditional_get]`, the plugin's instance name, not the module key. Environment variable: `$<APP>_CONDITIONAL_GET__<KEY>`. Flag: `--conditional_get.<key>`. Rename or hide a flag in `src/modules/middleware_conditional_get.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `etag` | bool | `true` | `--conditional_get.etag` | Set a weak ETag over the body when the page set none. |
| `last_modified` | bool | `true` | `--conditional_get.last_modified` | Answer If-Modified-Since from a Last-Modified the page set. |

## Read more

- [Middleware](../middleware.md) §8
- Source: [`crates/brocanth-middleware/src/conditional_get.rs`](../../crates/brocanth-middleware/src/conditional_get.rs), [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
