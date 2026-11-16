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
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no rate limit; a password guesser gets as many tries as the server answers |

<!-- generated: capabilities -->
## Capabilities

Generated from the manifest; do not edit. `docs_check` compares it. After a manifest change, run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`.
An option is set at build time in `src/modules/middleware_rate_limit.rs`. A setting is read at run time from these layers, lowest first: the default, the module file, the config file (with a `ConfigSource`), the environment, the flags.

| | |
|---|---|
| Needs | none |
| After | none |
| Provides | none |
| Requires | none |
| Uses | `ConfigSource` (settings read from a file): the rules are records: in the file when there is one, else code beside the registration |
| Adapts to | [`auth-screens`](auth-screens.md): the example rule covers the paths `auth-screens` exports as `sign_in_posts`; without it the rule is written commented out |
| Contributes | `AgentsSection` `middleware-rate-limit`: the example rule, and that a limit is asked for, not invented |
| Collects | none |
| Exports | none |
| Hooks | `count` (Guard, required): count each request against its rule, and answer 429 past the limit; `headers` (Encode, on by default): send the RateLimit headers on each response a rule covers |
| Options (build time) | none |
| Settings (run time) | `[rate_limit]`: `ip_header`, `session_cookie`, `fail_open` |
<!-- end generated: capabilities -->

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
