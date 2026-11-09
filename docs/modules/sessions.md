# `sessions`

A cookie, and a `Session` the context hands a handler.

The wizard calls it: *A signed cookie, and a session on the context*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | HTML application |
| Part of | nothing |
| Needs | none |
| After | none |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |

## What it writes

Files:

- `src/plugins/sessions/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/app.rs (setup): `setup.router().mount("/users/{*rest}", "session");`
- src/plugins/mod.rs (the context): `let session: &crate::plugins::sessions::Session = cx.extract();`
- src/plugins/mod.rs: `pub mod sessions;`
- src/plugins/mod.rs (the provider list): `sessions crate::plugins::sessions::Sessions`
- src/plugins/mod.rs (layer): `"session" => Some(sessions::layer),`
- config/<env>.toml: `[sessions]`

## Settings

Section `[sessions]`. Environment variable: `$<APP>_SESSIONS__<KEY>`. Flag: `--sessions.<key>`. Rename or hide a flag in `src/modules/sessions.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `cookie` | text | `"_<app>_session"` | `--sessions.cookie` | The session cookie's name. |
| `secure` | bool | `false` | `--sessions.secure` | Send the cookie over HTTPS only. |

## Read more

- [Middleware](../middleware.md)
- [Module settings and flags](../module-flags.md)
- Source: [`crates/brocanth-cli/src/modules/sessions.rs`](../../crates/brocanth-cli/src/modules/sessions.rs)
