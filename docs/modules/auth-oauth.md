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
