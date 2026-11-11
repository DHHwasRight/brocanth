# `oauth-client`

Connect pages at /connections; tokens sealed in Postgres, refreshed once, sent as a bearer.

The wizard calls it: *Connect accounts and call their APIs*.

| | |
|---|---|
| Categories | **Routes**, Enhancer |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`oauth`](oauth.md), ticked with its parent |
| Needs | [`auth`](auth.md): a connection belongs to a signed-in account, and `auth` holds the sessions; [`db`](db.md): the tokens are rows in the `db` module's database, and a restart must not drop them |
| After | [`auth`](auth.md), [`db`](db.md), [`config`](config.md) |
| Adapts to | [`config`](config.md), [`middleware-csrf`](middleware-csrf.md), [`oauth-client-memory`](oauth-client-memory.md), [`admin`](admin.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-oauth-client` (feature `postgres` unless `oauth-client-memory`; `admin` with that module) |
| Without it | no connected accounts; nothing calls a provider's API for a person |

## What it does

It is golang.org/x/oauth2 for a brocanth application, over the `oauth2`
crate (5.x). A signed-in person connects a provider account. The
application then calls that provider's API as the person, or as itself.

| golang.org/x/oauth2 | brocanth-oauth-client |
|---|---|
| `Config`, `Endpoint` | `Provider`, `Endpoint` |
| `golang.org/x/oauth2/github` and friends | `endpoints::{github, google, microsoft, gitlab}` |
| `Config.AuthCodeURL`, `Config.Exchange` | `OauthClient::begin`, `OauthClient::complete`, behind the connect pages |
| `TokenSource` | `OauthClient::token(account, provider)` |
| `clientcredentials.Config.Token` | `OauthClient::app_token(provider)` |
| `Config.Client` | `OauthClient::http(account, provider)`, `OauthClient::app_http(provider)` |

```rust,ignore
let oauth = ctx.provided::<brocanth_oauth_client::OauthClient, _>().expect("dispatched");
let github = oauth.http(&account, "github");
let response = github.send(github.get("https://api.github.com/user")).await?;
```

The routes, under `mount_at` (`/connections`):

| Route | Does |
|---|---|
| `GET /connections` | the signed-in person's connections, a button each |
| `GET /connections/<provider>` | one Connect button, for a plain link |
| `POST /connections/<provider>` | a sealed flow cookie, then 303 to the provider |
| `GET /connections/<provider>/callback` | the provider's answer: the token is stored |
| `POST /connections/<provider>/disconnect` | revokes at the provider, deletes the row |

## Decisions

- **`oauth2` with its features off, over the workspace's reqwest.** The
  `oauth2` crate's default features bring reqwest 0.12. The workspace is on
  reqwest 0.13. So the features are off, and `brocanth_oauth_client::http::Http`
  implements `oauth2::AsyncHttpClient` over 0.13. `cargo tree -d` shows no
  second reqwest.
- **The revocation request is the crate's, not `oauth2`'s.** `oauth2`
  refuses an `http` revocation URL with no way to relax it. The https rule
  is this crate's, and development may relax it (`allow_http`). One code
  path: the tests run the request that production runs.
- **The flow starts with a POST, and the state is a sealed cookie.** A GET
  that starts a connection can be fired by any page on the web. A row per
  started flow is a write that a stranger can cause. The cookie is
  XChaCha20-Poly1305, `HttpOnly`, `SameSite=Lax`, ten minutes. The callback
  compares `state` in constant time.
- **The account rides in the cookie.** The `auth` session cookie is
  `SameSite=Strict`, so it is not sent on the cross-site callback. The
  account is read from the session at the POST, and the seal keeps anybody
  from changing it. A callback that does carry a session must carry the same
  account.
- **The callback answers a page, not a redirect.** After a cross-site
  navigation, a redirect also arrives without the `Strict` cookie, and the
  person lands signed out. The page moves on by itself, from this site.
- **No open redirect.** `return_to` is kept only when it is a path on this
  site: it starts with one `/`, has no `//` or `/\` start, no backslash and
  no control character.
- **An impersonated session connects nothing.** An operator acting as a
  person could otherwise attach the operator's provider account to the
  person's account.
- **The store sees sealed bytes only.** The client seals before `put` and
  opens after `get`, with the account, the provider and the kind (access or
  refresh) as associated data. A store author cannot forget the encryption,
  and a row copied to another account does not open.
- **A key of its own, not the session key.** Rotating the session key signs
  everybody out. That must not also lose every stored connection.
- **One refresh per key, per process.** An async lock per
  `(account, provider)`. The first caller refreshes, the others wait and read
  the new row. Two instances can still race; an `invalid_grant` is checked
  once more against the row, and a refresh token another instance stored is
  used instead of `Error::Reconnect`.
- **The rotated refresh token is stored in the same write.** When a refresh
  answers with no refresh token, the old one stays (RFC 6749 §6).
- **`Error::Reconnect` is a type, not a string.** Only the person can fix a
  revoked grant. The application matches the variant and sends them to
  `/connections`.
- **The application token is cached in memory, not stored.** It costs one
  request to get again, and a row would be a second copy of a credential.
- **One retry after a 401, never a loop.** The API said the token is bad
  although it looked valid. One forced refresh, one retry; the second 401
  goes back to the caller.
- **Scopes are code, not settings.** The code that calls the API needs them;
  a deployment cannot change them without that code changing. They are in
  `scopes()` in the generated file.
- **https for every provider URL, except in development.** The token and
  revocation requests carry the client secret and the tokens. An `http` URL
  in production is most often an internal address, which is how a wrong
  provider setting becomes a request into your own network. `allow_http`
  is off by default.
- **Four built-in providers.** Each costs two settings. Another provider is
  one `Provider::new` in the generated file. GitHub and Microsoft have no
  RFC 7009 endpoint, so a disconnect there only deletes the row.

## What it writes

Files:

- `src/plugins/oauth_client/mod.rs`: the settings, the scopes, a plugin
  that builds the client over the token store, and `mount`.
- `src/modules/oauth_client.rs`.

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs: `pub mod oauth_client;`
- config/<env>.toml: `[oauth_client]`
- the route table: `crate::plugins::oauth_client::mount(routes, <mount_at>)`
- the route list: `/connections`, `/connections/{*rest}`
- the admin panels, with `admin`: `brocanth_oauth_client::admin::ConnectionsPanel`
- AGENTS.md: `## Connected accounts`

With `middleware-csrf`, the Connect and Disconnect forms carry the token.

## Choices

| Choice | Default | What it does |
|---|---|---|
| option `mount_at` | `/connections` | Where the connect pages answer. The redirect URI at each provider is `<base_url><mount_at>/<provider>/callback`. |
| integration `admin` | on | A tab at `/admin/connections`: every connection without its tokens, and a Revoke button. Without `admin` it does nothing. |

## Settings

Section `[oauth_client]`. Environment variable: `$<APP>_OAUTH_CLIENT__<KEY>`. Flag: `--oauth_client.<key>`. Rename or hide a flag in `src/modules/oauth_client.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `providers` | list | `["github"]` | none | The providers to offer, by id: github, google, microsoft, gitlab. |
| `base_url` | text | `"http://localhost:3000"` | `--oauth_client.base_url` | The scheme and host the providers send the browser back to. |
| `key` | text | none | none (a secret) | The key that seals the tokens: 32 random bytes, base64url. |
| `allow_http` | bool | `false` | `--oauth_client.allow_http` | Accept `http://` provider URLs. Development only. |
| `migrate` | bool | `true` | `--oauth_client.migrate` | Create the token table on the first readiness check. Not with `oauth-client-memory`. |
| `<id>_client_id` | text | none | `--oauth_client.<id>_client_id` | The client id at the provider, for `github`, `google`, `microsoft`, `gitlab`. |
| `<id>_client_secret` | text | none | none (a secret) | The client secret. |

The server refuses to start without `key`, and when a provider in
`providers` has no id or no secret. The project's own tests use a fixed key
and leave out a provider with no secret.

## Read more

- [`oauth`](oauth.md), [`oauth-client-memory`](oauth-client-memory.md), [`auth-oauth`](auth-oauth.md)
- [brocanth-oauth-client](../../crates/brocanth-oauth-client/README.md)
- Source: [`crates/brocanth-oauth-client/src/scaffold.rs`](../../crates/brocanth-oauth-client/src/scaffold.rs), [`crates/brocanth-oauth-client/src/client.rs`](../../crates/brocanth-oauth-client/src/client.rs)
