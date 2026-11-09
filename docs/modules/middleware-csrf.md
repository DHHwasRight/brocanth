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
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no CSRF check; any page on any site can post a form here and the browser sends the cookies with it |

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
| `cookie` | text | `"csrf_token"` | `--csrf.cookie` | The cookie this hook owns. |
| `field` | text | `"_csrf_token"` | `--csrf.field` | The form field the token comes back in. |
| `header` | text | `"x-csrf-token"` (commented out) | `--csrf.header` | The header a fetch caller may send instead of the field. Empty means none. While it is set, the cookie is not HttpOnly: a script reads it. |
| `exempt` | list | `[]` | none (a list) | Paths never checked, each with everything under it. For a caller that is not a browser: a webhook, a device. |
| `secure` | bool | `true` | `--csrf.secure` | Send the cookie over HTTPS only. |

## Read more

- [Middleware](../middleware.md)
- Source: [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
