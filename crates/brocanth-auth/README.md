# brocanth-auth

Authentication for brocanth: local accounts with argon2id, server-side
sessions in a signed cookie, OAuth 2.0 and OIDC with mandatory PKCE,
explicit identity linking, second factors (TOTP, recovery codes), one-time
codes, login alerts, account lockout and impersonation. A provider is data, not code: a
struct literal in `providers`. Login, registration, password reset and
one-time codes do not reveal whether an address exists. A sign-in that
skipped the second-factor policy does not compile. Use it when the
application has accounts. Do not use it for authorization (which rows a
user may see or change): that is `brocanth-policy`.

## Getting started

```toml
[dependencies]
brocanth-auth = { git = "https://github.com/DHHwasRight/brocanth.git" }
# Features, all off by default:
#   reqwest-client  - `ReqwestClient`, the shipped `HttpClient`, for OAuth providers
#   postgres        - `PgStore`: every storage trait over Postgres
#   admin           - the Accounts tab for brocanth-admin
#   conformance     - the storage contract as tests, for an adapter author
#   scaffold        - the `brocanth create` module; only the CLI turns it on
```

Inside this workspace: `brocanth-auth.workspace = true`.

With no feature, the crate links no HTTP client, no TLS stack and no SQL
driver. **Reason:** the common case is local accounts with no provider, and
that application must not link a TLS stack to hash a password.

### As a module

```text
brocanth create my-app --with auth
brocanth add auth
```

The module writes `src/plugins/auth/mod.rs`, a plugin that publishes the
services on the context (`cx.extract::<crate::plugins::auth::Services>()`).
It starts on `MemoryStore`. With `jobs` or `cache` in the project, it uses
`PgStore` and turns on the `postgres` feature. It writes no provider, no
second factor and no sign-in page.

Two modules from this crate go beside it, off until you tick them:

```text
brocanth add auth-screens   # sign-up, sign-in, reset, verify and TOTP pages
brocanth add auth-lockout   # an account locks for minutes after wrong passwords
```

`auth-screens` needs `rsx`, `input`, `middleware-csrf`, `mail` and
`flash`. See `docs/modules/auth-screens.md` and
`docs/modules/auth-lockout.md`.

Settings (section `auth`; see `docs/module-flags.md`):

| Key | Default | Flag | Variable | Meaning |
|---|---|---|---|---|
| `url` | `postgres://localhost/development` | none (secret) | `$<APP>_AUTH__URL` | The Postgres database. Only with `jobs` or `cache`. |
| `signing_key_env` | `<APP>_SESSION_KEY` | `--auth.signing_key_env` | `$<APP>_AUTH__SIGNING_KEY_ENV` | The **name** of the variable that holds the key, not the key. |
| `cookie_name` | `session` | `--auth.cookie_name` | `$<APP>_AUTH__COOKIE_NAME` | The session cookie's name. `__Host-session` in production. |
| `secure` | `false` | `--auth.secure` | `$<APP>_AUTH__SECURE` | Send the cookie over HTTPS only. `true` in production. |
| `idle_timeout` | `28800` | `--auth.idle_timeout` | `$<APP>_AUTH__IDLE_TIMEOUT` | Seconds of inactivity before a session is dead. |
| `absolute_timeout` | `2592000` | `--auth.absolute_timeout` | `$<APP>_AUTH__ABSOLUTE_TIMEOUT` | Seconds from creation before a session is dead. |

With the `config` module, the same keys are in `[auth]` in
`config/<env>.toml`. The signing key is 32 bytes, base64url, and the
application stops without it. `brocanth` does not invent one: a key made
at startup logs every session out on each restart. Make one:

```text
export MY_APP_SESSION_KEY=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')
```

## Examples

### Hash and verify a password

```rust
use brocanth_auth::{PasswordCost, Passwords};

// `Passwords::recommended()` in production. The test cost keeps this fast.
let passwords = Passwords::new(PasswordCost::insecure_for_tests()).unwrap();
let phc = passwords.hash("correct horse battery staple").unwrap();
assert!(phc.starts_with("$argon2id$"));
assert!(passwords.verify(&phc, "correct horse battery staple"));
assert!(!passwords.verify(&phc, "wrong"));
```

### Register, sign in, and open a session

A login gives a `Verified` proof. The `FactorPolicy` turns the proof into
an `Admitted` (or asks for a second factor). Only an `Admitted` opens a
session, and opening the session consumes it.

```rust
use std::sync::Arc;
use brocanth_auth::{
    Accounts, Admission, FactorPolicy, MemoryStore, Mfa, PasswordCost, Passwords, SessionConfig,
    Sessions, SigningKey, SystemClock,
};

# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), brocanth_auth::AuthError> {
let store = Arc::new(MemoryStore::new());
let clock = Arc::new(SystemClock);
let passwords = Arc::new(Passwords::new(PasswordCost::insecure_for_tests())?);

let accounts = Accounts::new(store.clone(), clock.clone(), passwords)?;
let mfa = Mfa::new(store.clone(), clock.clone())?;
let sessions = Sessions::new(store.clone(), clock.clone(), SigningKey::generate()?, SessionConfig::default());

// Registration always succeeds, so the response does not reveal a taken address.
accounts.register("ada@example.com", "a long passphrase").await?;

// A wrong password and an unknown address give the same error.
assert!(accounts.login("ada@example.com", "nope").await.is_err());
let proof = accounts.login("ada@example.com", "a long passphrase").await?.into_proof();

// Ask the policy. The account has no second factor, so the proof is enough.
let enrolled = mfa.second_factors(&proof).await?;
let Admission::Admit(admitted) = FactorPolicy::recommended().admit(&enrolled, proof) else {
    panic!("ask for a second factor here");
};

// `replacing` is the pre-login session id, which is deleted: no session fixation.
let started = sessions.begin_with_factors(admitted, None).await?;
assert!(started.cookie.as_str().contains("HttpOnly"));

// On the next request, the cookie value gives the session back.
let value = sessions.seal(&started.session.id);
let loaded = sessions.load(&value).await?;
assert_eq!(loaded.account, started.session.account);
# Ok(())
# }
```

### Register the plugin, so handlers get `Auth` on the context

`AuthPlugin` takes values that the application built. There is no default
store and no default signing key.

```rust
use std::sync::Arc;
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_auth::http_client::StubHttp;
use brocanth_auth::{AuthConfig, AuthPlugin, MemoryStore, Passwords, SessionConfig, SigningKey, SystemClock};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let app = AppBuilder::new()
    .plugin(AuthPlugin::new(AuthConfig {
        store: Arc::new(MemoryStore::new()),
        // No provider, so no request leaves. With a provider, use
        // `ReqwestClient` (feature `reqwest-client`) or your own `HttpClient`.
        http: Arc::new(StubHttp::new()),
        clock: Arc::new(SystemClock),
        // In production: SigningKey::parse(&std::env::var("MY_APP_SESSION_KEY")?)?
        signing_key: SigningKey::generate()?,
        session: SessionConfig::default(),
        passwords: Arc::new(Passwords::recommended()?),
        providers: Vec::new(),
    }))
    .build()?;
let _ = app;
# Ok(())
# }
```

A handler then reads it with `let auth: &Auth<_, _, _> = cx.extract();`
and calls `auth.accounts()`, `auth.sessions()`, `auth.linking()` or
`auth.oauth()`. Second factors, one-time codes, login alerts and the
lockout each need their own store trait. `auth.mfa()` and
`auth.lockout(policy)` build `Mfa` and `Lockout` when the store has the
trait. An application builds `Otp` or `LoginAlerts` from the same `Arc<S>`.

## Testing

```text
cargo test -p brocanth-auth
```

This runs the unit tests, the integration tests over `MemoryStore`, the
Accounts tab tests (`admin`) and the storage conformance suite. The
`compile_fail` tests (trybuild) check that a sign-in without the policy
does not compile. Their expected output is rustc's own text, pinned to the
toolchain in `mise.toml`. After a toolchain change, regenerate them with
`TRYBUILD=overwrite`.

The Postgres store needs a feature and a database:

```text
BROCANTH_AUTH_TEST_POSTGRES=postgres://localhost/brocanth_auth_test \
  cargo test -p brocanth-auth --features postgres
```

`BROCANTH_TEST_DATABASE_URL` is the fallback. Without either, the
Postgres tests skip. `BROCANTH_AUTH_REQUIRE_POSTGRES=1` makes a missing
database a failure. Each case creates its own schema and drops it.

An application tests code that uses this crate with `MemoryStore`,
`TestClock` (move time with `advance`) and
`PasswordCost::insecure_for_tests()`. To test a screen behind a sign-in,
send the session cookie with `brocanth_test::TestClient`
(`crates/brocanth-test/README.md`). An adapter for another database turns
on `conformance` in its test binary and runs the same suite as the two
shipped stores.

## More

- `docs/auth-store.md`: the storage traits, `PgStore`, what each row
  holds, and the sweep.
- `docs/module-flags.md`: how the `auth` settings become flags,
  variables and config keys.
- The crate documentation below: each security decision and its reason.
