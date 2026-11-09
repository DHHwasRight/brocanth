# `auth`

Argon2id accounts and signed sessions; no provider, so no HTTP client.

The wizard calls it: *Accounts and passwords. No OAuth provider*.

| | |
|---|---|
| Categories | **Middleware**, Enhancer |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | nothing |
| Needs | none |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md), [`jobs`](jobs.md), [`cache`](cache.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth` |
| Without it | no accounts; nothing hashes a password and nothing signs a session cookie |

## What it writes

Files:

- `src/plugins/auth/mod.rs`

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the context): `let auth: &crate::plugins::auth::Services = cx.extract();`
- src/plugins/mod.rs: `pub mod auth;`
- src/plugins/mod.rs (the provider list): `auth crate::plugins::auth::Auth`
- config/<env>.toml: `[auth]`

`auth.url` exists only when the project also has `jobs` or `cache`: then accounts live in Postgres (`PgStore`). Otherwise they live in memory (`MemoryStore`).

## Settings

Section `[auth]`. Environment variable: `$<APP>_AUTH__<KEY>`. Flag: `--auth.<key>`. Rename or hide a flag in `src/modules/auth.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `signing_key_env` | text | `"<APP>_SESSION_KEY"` | `--auth.signing_key_env` | The name of the variable that holds the session signing key. |
| `cookie_name` | text | `"session"` | `--auth.cookie_name` | The session cookie's name. |
| `secure` | bool | `false` | `--auth.secure` | Send the cookie over HTTPS only. |
| `idle_timeout` | integer | `28800` | `--auth.idle_timeout` | Seconds of inactivity before a session is dead. |
| `absolute_timeout` | integer | `2592000` | `--auth.absolute_timeout` | Seconds from creation before a session is dead, however much it was used. |

## Read more

- [Where accounts live](../auth-store.md)
- [brocanth-auth](../../crates/brocanth-auth/README.md)
- Source: [`crates/brocanth-auth/src/scaffold.rs`](../../crates/brocanth-auth/src/scaffold.rs)
