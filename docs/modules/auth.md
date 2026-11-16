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
| Uses | `PgPool`: accounts and sessions are rows on the project's pool, not in memory |
| After | [`config`](config.md) |
| Adapts to | [`config`](config.md), [`auth-oauth`](auth-oauth.md) |
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

With the project's pool (`PgPool`, from [`db`](db.md)), accounts and sessions live in Postgres (`PgStore`) on that pool: auth opens no pool and `[auth]` has no `url`. Without it they live in memory (`MemoryStore`). A queue or a cache in Postgres requires the pool, so with `jobs` or `cache` the accounts are in Postgres too; `jobs-memory` and `cache-memory` bring no pool.

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
