# `middleware-security-headers`

HSTS, nosniff, a referrer policy, no framing, a CSP that runs dp-swap by hash.

The wizard calls it: *Send the headers a browser reads as policy*.

| | |
|---|---|
| Categories | **Middleware** |
| Wizard question | Doors |
| Default | on in a non-interactive `create` |
| Presets | none |
| Part of | [`middleware`](middleware.md), ticked with its parent |
| Needs | none |
| After | [`middleware-csrf`](middleware-csrf.md) |
| Adapts to | [`admin`](admin.md), [`gtag`](gtag.md), [`config`](config.md) |
| Conflicts | [`static`](static.md): this module needs a running server |
| Outside the runtime | no |
| Crates | none |
| Without it | no security headers; a browser sniffs types, frames the pages and runs any inline script |

## What it writes

Slots:

- src/plugins/mod.rs (the builder chain)
- src/plugins/mod.rs (the provider list): `middleware-security-headers brocanth_middleware::SecurityHeaders`
- src/plugins/mod.rs (the hook record): `brocanth_middleware::SecurityHeaders`
- config/<env>.toml: `[security_headers]`
- AGENTS.md: `## Security headers`

The registration reads `[security_headers]` through `plugin_hooking_from_config` with `config`. With `admin`, it uses `plugin_hooking_from_config_with` and adds the admin's inline script hash on top of what the file says. Without `config`, it starts from `SecurityHeadersConfig::default()` and applies the environment and the flags. `frames` and `csp.directives` are file-only: they are not declared, and a flag cannot spell them.

## Settings

Section `[security_headers]`, the plugin's instance name, not the module key. Environment variable: `$<APP>_SECURITY_HEADERS__<KEY>` (a dot in the key is `__`). Flag: `--security_headers.<key>`. Rename a flag in `src/flags.rs`.

| Key | Kind | File value | Flag | Help |
|---|---|---|---|---|
| `behind_tls` | bool | `false` | `--security_headers.behind_tls` | Every request arrives through a TLS terminator, so HSTS always goes out. False in development: HSTS from localhost pins the name in the browser. |
| `nosniff` | bool | `true` | `--security_headers.nosniff` | Send `X-Content-Type-Options: nosniff`. |
| `referrer_policy` | text | `"strict-origin-when-cross-origin"` (commented out) | `--security_headers.referrer_policy` | The `Referrer-Policy`. Empty means none. |
| `permissions_policy` | text | `"camera=(), microphone=(), geolocation=(), payment=(), usb=()"` (commented out) | `--security_headers.permissions_policy` | The `Permissions-Policy`. Empty means none. |
| `hsts.enabled` | bool | `true` | `--security_headers.hsts.enabled` | Send `Strict-Transport-Security`. |
| `hsts.max_age` | integer | `31536000` | `--security_headers.hsts.max_age` | How long the browser remembers HTTPS-only, in seconds. |
| `hsts.include_subdomains` | bool | `false` | `--security_headers.hsts.include_subdomains` | The HSTS rule covers every subdomain too. Only when the person who owns the domain says so. |
| `hsts.preload` | bool | `false` | `--security_headers.hsts.preload` | Ask to be on the browsers' built-in HSTS list. Hard to undo: removal takes months to reach users. |
| `csp.enabled` | bool | `true` | `--security_headers.csp.enabled` | Send a `Content-Security-Policy`. |
| `csp.report_only` | bool | `false` | `--security_headers.csp.report_only` | Send the policy as `Content-Security-Policy-Report-Only`. |
| `csp.report_uri` | text | `"/csp-reports"` (commented out) | `--security_headers.csp.report_uri` | Where a browser reports what the policy blocked. Empty means none. |

## Read more

- [Middleware](../middleware.md)
- Source: [`crates/brocanth-middleware/src/scaffold.rs`](../../crates/brocanth-middleware/src/scaffold.rs)
