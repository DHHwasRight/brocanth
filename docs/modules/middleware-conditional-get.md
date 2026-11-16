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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no ETag and no 304; a browser downloads an unchanged page again in full |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/middleware_conditional_get.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `middleware-conditional-get`: how a page sets an ETag or Last-Modified |
| Collects | none |
| Exports | none |
| Hooks | `check` (Encode, required): tag each 200 to a GET with a weak ETag, and answer 304 when it matches |
| Options (build time) | none |
| Settings (run time) | `[conditional_get]`: `etag`, `last_modified` |
<!-- end generated: capabilities -->

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

**Order.** The hook is in `Phase::Encode`: it runs after the route cache stores a page (`Cache`) and before compression, which is first in `Encode`. It names no other hook, so this holds in any registration order. A cache hit gets a tag and a `304` like a fresh page.

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
