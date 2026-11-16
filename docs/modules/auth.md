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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth` |
| Without it | no accounts; nothing hashes a password and nothing signs a session cookie |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/auth.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | `SessionCookie` (a session cookie): a signed session cookie, with sign-in and sign-out |
| Requires | none |
| Uses | `PgPool` (a Postgres pool): accounts and sessions are rows on the project's pool, not in memory |
| Adapts to | [`auth-oauth`](auth-oauth.md): the session cookie is SameSite=Lax, so the provider callback sees who is signed in |
| Contributes | none |
| Collects | none |
| Exports | none |
| Hooks | none |
| Options (build time) | none |
| Settings (run time) | `[auth]`: `signing_key_env`, `totp_key_env`, `cookie_name`, `secure`, `idle_timeout`, `absolute_timeout` |
<!-- end generated: capabilities -->

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
