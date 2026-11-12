# `auth-oauth`

Sign-in with GitHub, Google, Microsoft or GitLab at /oauth; links an HTTP
client.

| | |
|---|---|
| Categories | **Routes**, Enhancer |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`auth`](auth.md), off until you tick it |
| Needs | none |
| After | [`auth`](auth.md) |
| Adapts to | [`config`](config.md), [`middleware-csrf`](middleware-csrf.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | `brocanth-auth`, feature `reqwest-client` |
| Without it | no provider sign-in; an account has an address and a password |

## What it writes

Files:

- `src/plugins/auth_oauth/mod.rs`: the settings, a plugin that checks them
  at startup, and `mount`, which puts the flow into the route table.

Slots:

- `RouterMounts`: `crate::plugins::auth_oauth::mount(routes, <mount_at>)`.
- `PagesRoutes`: `/oauth/{*rest}`.
- `AgentsSections`: "Provider sign-in".

The routes are `brocanth_auth::oauth::web::OauthRoutes`:

| Route | Does |
|---|---|
| `GET /oauth/<provider>` | a page with one button |
| `POST /oauth/<provider>` | a signed flow cookie (`state`, `nonce`, PKCE verifier), then 303 to the provider |
| `GET /oauth/<provider>/callback` | a session, or a page that asks |
| `GET`, `POST /oauth/<provider>/link` | connects the provider to the signed-in account |

With `middleware-csrf`, the button and link forms carry the token.

## Decisions

- **The flow starts with a POST.** A link that starts a sign-in can be
  fired by any page on the web. The GET page exists so a plain link still
  works without script.
- **The flow state is a signed cookie, not a row.** A row per started
  sign-in would give a stranger a write on the database. The cookie lasts
  ten minutes and is `SameSite=Lax`, because the callback is a cross-site
  navigation.
- **A matching address never links by itself.** A provider joins an
  existing account only by a POST from a session signed in as that
  account. Otherwise a provider that reports somebody else's address takes
  the account over.
- **An account with a second factor is not signed in by a provider alone.**
  The callback asks `FactorPolicy::recommended`.
- **The callback answers a page, not a redirect.** A `SameSite=Strict`
  session cookie set on a cross-site navigation is not sent on the
  redirect that follows. The page moves on by itself, from this site.
- **The auth plugin does not change.** The routes hold their own HTTP
  client. `Auth` keeps `StubHttp`: linking and sessions call no provider.
- **Four providers.** Each costs two settings. Discord, Slack and Facebook
  are one line each in the generated file. Apple is not offered: its client
  secret is a JWT signed with a private key, and its callback is a
  cross-site POST that the CSRF hook refuses.
- **The mount point is an option.** `mount_at` in
  `src/modules/auth_oauth.rs`. The redirect URI is a constant at the
  provider's console, so it is not a setting.

## Protocol

The flow does not write OAuth or OIDC by hand. Two crates do it:

- `oauth2` 5 builds the authorization URL, makes the `state` and the PKCE
  pair (`S256`, always), and sends the code exchange and the refresh.
  `brocanth-oauth-client` uses the same crate.
- `openidconnect` 4.0 verifies the id_token: the signature against the
  provider's key set, the issuer, the audience, the expiry and the nonce.

Both send their requests through `brocanth_auth::http_client::HttpClient`,
so an application that implements that trait keeps it. `ReqwestClient`
sends through `brocanth_oauth_client::http::Http`: one reqwest, one TLS
setup for the sign-in and the connected accounts. The `state` check, the
signed flow cookie, the profile read and the provider quirks stay in
`brocanth-auth`.

The providers are static: each one names its endpoints and its key-set URL.
The flow does no OIDC discovery.

### Behaviour changes, and why

- **`ReqwestClient` follows no redirect and reads at most 1 MiB.** Before,
  it followed ten redirects. A token endpoint that answers 3xx would then
  send the code, the verifier and the client secret to another host. An
  answer without a size limit lets a provider fill the memory.
- **An id_token with a second audience is refused.** Before, a token
  passed when its `aud` list held this client and anything else. This
  client trusts no other audience, and OIDC Core §3.1.3.7 says to reject a
  token with an untrusted audience.
- **An id_token must carry `iat`.** OIDC Core requires it, and
  `openidconnect` reads it. Every provider in the catalogue sends it.
- **`exp` is checked against the injected `Clock`,** with the same minute
  of leeway as before. Before, it was checked against the system clock, so
  a test clock did not reach it. With `SystemClock` nothing changes.
- **With `ClientSecretBasic`, the client id goes only in the
  `Authorization` header.** Before, it also went in the form body. RFC 6749
  §2.3.1 says to use one method, not both. No provider in the catalogue
  uses Basic.
- **`TokenResponse::token_type` is lower case** (`bearer`), as `oauth2`
  reads it. Compare it without case.
- **A redirect URI that does not parse fails at `begin`,** with
  `AuthError::Misconfigured`. Before, it went to the provider, which
  refused it.

## Settings

Section `[auth_oauth]`.

| Key | File value | Help |
|---|---|---|
| `providers` | `["github"]` | The providers to offer, by id. No flag (a list). |
| `base_url` | `"http://localhost:3000"` | The scheme and host the providers send the browser back to. |
| `after_sign_in` | `"/"` | Where a person goes after a sign-in or a link. |
| `sign_in_page` | none | Your password sign-in page, for a person who must sign in before a link. |
| `<id>_client_id` | none | The client id at the provider, for `github`, `google`, `microsoft`, `gitlab`. |
| `<id>_client_secret` | none | The client secret. No flag: set it in `$<APP>_AUTH_OAUTH__<ID>_CLIENT_SECRET`. |

The server refuses to start when a provider in `providers` has no id or no
secret.

## Read more

- Source: [`crates/brocanth-auth/src/scaffold/oauth.rs`](../../crates/brocanth-auth/src/scaffold/oauth.rs)
- The flow: [`crates/brocanth-auth/src/oauth/web.rs`](../../crates/brocanth-auth/src/oauth/web.rs)
