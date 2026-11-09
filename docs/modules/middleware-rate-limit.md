# `middleware-rate-limit`

A fixed window per address, session or user, per group of routes; 429 with Retry-After.

The wizard calls it: *Limit how often one caller may hit a route*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | off |
| Presets | none |
| Part of | [`middleware`](middleware.md), off until you tick it |
| Needs | none |
| After | [`middleware-security-headers`](middleware-security-headers.md) |
| Adapts to | [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no rate limit; a password guesser gets as many tries as the server answers |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `middleware-rate-limit brocanth_middleware::RateLimit`
- src/plugins/mod.rs (the hook record): `brocanth_middleware::RateLimit`
- config/<env>.toml: `[rate_limit]`
- AGENTS.md: `## Rate limit`

With `config`, the registration reads `[rate_limit]` through `plugin_hooking_from_config`, and the file also holds one example rule as `[[rate_limit.rules]]` (`login`, 5 per 60 s). Replace the numbers. Without `config`, the example rule is code beside the registration (`.rule(login)`), on the settings the environment and the flags built. A rule is a record, and a flag has no spelling for a list of records, so the rules have no flag.

## Settings

Section `[rate_limit]`, the plugin's instance name, not the module key. Environment variable: `$<APP>_RATE_LIMIT__<KEY>` (a dot in the key is `__`). Flag: `--rate_limit.<key>`. Rename or hide a flag in `src/modules/middleware_rate_limit.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `ip_header` | text | `"fly-client-ip"` (commented out) | `--rate_limit.ip_header` | A header that holds the client address, set by your edge. Empty means none. Only a header the edge overwrites. Never `X-Forwarded-For` as the client sent it. |
| `session_cookie` | text | `"_<app>_session"` (commented out) | `--rate_limit.session_cookie` | The session cookie, for a rule keyed by session. Empty means none. |
| `fail_open` | bool | `true` | `--rate_limit.fail_open` | Let a request through when the counter store fails. |

## Read more

- [Middleware](../middleware.md)
- Source: [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
