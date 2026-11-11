# brocanth-oauth-client

An OAuth 2.0 client for brocanth: call a third-party API on behalf of a
person who connected an account, or on behalf of the application. It is
golang.org/x/oauth2 for brocanth, over the [`oauth2`](https://docs.rs/oauth2)
crate. It is not a sign-in: that is `brocanth-auth` and its `auth-oauth`
module.

| golang.org/x/oauth2 | here |
|---|---|
| `Config`, `Endpoint` | [`Provider`], [`Endpoint`] |
| `golang.org/x/oauth2/github` and friends | [`endpoints`] |
| `Config.AuthCodeURL`, `Config.Exchange` | [`OauthClient::begin`], [`OauthClient::complete`] |
| `TokenSource` | [`OauthClient::token`] |
| `clientcredentials.Config.Token` | [`OauthClient::app_token`] |
| `Config.Client` | [`OauthClient::http`], [`OauthClient::app_http`] |

A generated project gets it from the `oauth` and `oauth-client` modules:
`brocanth add oauth-client`. See `docs/modules/oauth-client.md`.

## A client

One key (32 random bytes), a store, and the providers:

```rust
use std::sync::Arc;
use brocanth_oauth_client::{Key, MemoryTokenStore, OauthClient, Provider};

let client = OauthClient::builder(Key::parse(&Key::generate())?)
    .store(Arc::new(MemoryTokenStore::new()))
    .provider(Provider::github("client-id", "client-secret").scopes(["read:user"]))
    .callback_base("https://example.com/connections")
    .build()?;

assert_eq!(
    client.redirect_uri("github"),
    "https://example.com/connections/github/callback"
);
# Ok::<(), brocanth_oauth_client::Error>(())
```

Register that redirect URI at the provider. A provider URL must be `https`:
the token request carries the client secret. `allow_http(true)` relaxes it
for a fake provider in development.

## Connecting

[`web::ConnectRoutes`] answers the connect pages with no web framework
assumed. Underneath, [`OauthClient::begin`] makes a fresh `state` and a PKCE
pair, and seals them with the account into a cookie value:

```rust
# use std::sync::Arc;
# use brocanth_oauth_client::{Key, MemoryTokenStore, OauthClient, Provider};
# let client = OauthClient::builder(Key::from_bytes([1; 32]))
#     .store(Arc::new(MemoryTokenStore::new()))
#     .provider(Provider::github("client-id", "client-secret"))
#     .build()?;
let started = client.begin("github", "account-42", Some("/settings"))?;
assert!(started.url.starts_with("https://github.com/login/oauth/authorize?"));
assert!(started.url.contains("code_challenge_method=S256"));
// `started.cookie` goes in an HttpOnly cookie. It is sealed: the state and
// the PKCE verifier are not readable in it.
# Ok::<(), brocanth_oauth_client::Error>(())
```

[`OauthClient::complete`] checks the cookie, its age (ten minutes) and the
`state` (in constant time), exchanges the code with the PKCE verifier, and
stores the token for the account that started.

## Calling the API

```rust,no_run
# async fn call(client: brocanth_oauth_client::OauthClient) -> Result<(), brocanth_oauth_client::Error> {
use brocanth_oauth_client::Error;

let github = client.http("account-42", "github");
match github.send(github.get("https://api.github.com/user")).await {
    Ok(response) => println!("{}", response.status()),
    // The grant is gone: only the person can fix it.
    Err(Error::Reconnect { provider, .. }) => println!("connect {provider} again"),
    Err(other) => return Err(other),
}

// As the application itself (client credentials), cached until it runs out.
let app = client.app_http("github");
let _ = app.send(app.get("https://api.example.com/status")).await?;
# Ok(())
# }
```

[`OauthClient::token`] refreshes an expired token before it returns it.
Concurrent callers for one `(account, provider)` make one refresh; the
others wait and read the new row. A rotated refresh token is stored in the
same write. A refused refresh token is [`Error::Reconnect`].

[`Authorized::send`] adds `Authorization: Bearer`, and after a 401 it forces
one refresh and sends the request once more.

## Storage

A [`TokenStore`] keeps [`StoredToken`]s: the access and refresh tokens are
sealed (XChaCha20-Poly1305) before the store sees them, bound to the account,
the provider and the kind. [`MemoryTokenStore`] is for tests.
`PgTokenStore` (feature `postgres`) keeps them in the `db` module's
database; run `PgTokenStore::SCHEMA` or `migrate`. The feature
`conformance` holds any store to the contract:

```rust,ignore
#[tokio::test]
async fn my_store_keeps_the_contract() {
    brocanth_oauth_client::conformance::run(&MyStore::new()).await;
}
```

## Features

| Feature | Default | Brings |
|---|---|---|
| `runtime` | on | the client, the pages, the memory store |
| `postgres` | off | `PgTokenStore`, over `brocanth-db` |
| `admin` | off | `admin::ConnectionsPanel`, a tab in `brocanth-admin` |
| `conformance` | off | `conformance::run` |
| `scaffold` | off | the `oauth`, `oauth-client` and `oauth-client-memory` modules, for the CLI |
