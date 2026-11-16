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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no CSRF check; any page on any site can post a form here and the browser sends the cookies with it |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/middleware_csrf.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | none |
| Adapts to | none |
| Contributes | `AgentsSection` `middleware-csrf`: every form carries the CSRF token |
| Collects | none |
| Exports | none |
| Hooks | `check` (Guard, required): refuse an unsafe request that does not send the token back; `mint` (Guard, required): set the token cookie on a response |
| Options (build time) | none |
| Settings (run time) | `[csrf]`: `cookie`, `field`, `header`, `exempt`, `secure` |
<!-- end generated: capabilities -->

Hooks: `csrf.check` and `csrf.mint`. Both are required, so
`src/modules/middleware_csrf.rs` has no `hooks` field. Its header names them.

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `middleware-csrf brocanth_middleware::Csrf`
- src/plugins/mod.rs (the hook record): `brocanth_middleware::Csrf`
- config/<env>.toml: `[csrf]`
- AGENTS.md: `## CSRF`

The registration reads `[csrf]` through `plugin_hooking_from_config` with `config`. Without `config`, it starts from `CsrfConfig::default()` and applies the environment and the flags. A list has no flag, so `exempt` needs `config`.

## Settings

Section `[csrf]`, the plugin's instance name, not the module key. Environment variable: `$<APP>_CSRF__<KEY>` (a dot in the key is `__`). Flag: `--csrf.<key>`. Rename or hide a flag in `src/modules/middleware_csrf.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `cookie` | text | `"__Host-csrf_token"` | `--csrf.cookie` | The cookie this hook owns. Keep the `__Host-` prefix: a browser stores it only from this host, over HTTPS, so a sibling subdomain cannot plant the cookie. With `secure` off the prefix is dropped on the wire. |
| `field` | text | `"_csrf_token"` | `--csrf.field` | The form field the token comes back in. |
| `header` | text | `"x-csrf-token"` (commented out) | `--csrf.header` | The header a fetch caller may send instead of the field. Empty means none. While it is set, the cookie is not HttpOnly: a script reads it. |
| `exempt` | list | `[]` | none (a list) | Paths never checked, each with everything under it. For a caller that is not a browser: a webhook, a device. |
| `secure` | bool | `true` | `--csrf.secure` | Send the cookie over HTTPS only. |

## Read more

- [Middleware](../middleware.md)
- Source: [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
