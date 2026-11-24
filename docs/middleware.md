# Middleware

What the Gorilla toolkit provides, what brocanth has, and where each one
lands on this framework's seams.

Gorilla's inventory was read from `pkg.go.dev/github.com/gorilla/handlers`
on 2026-10-22, and from the toolkit's other repositories by name. The
brocanth column comes from reading this checkout, not from a crate list.

## 1. The seam

brocanth has two middleware seams: a pattern-scoped layer chain, and hooks.

**Layers** run inside dispatch, for a route that matched. The sync `Layer`
(`brocanth/src/routing_middleware.rs`) has no user: an `await` inside one
does not compile, because the output type is a trait parameter, fixed when
the stack is built, and it cannot name the borrow of the chain that an async
layer must hold. `docs/async-handlers.md` §4 has both compiler errors in
full. `AsyncMiddlewareStack` (`routing_middleware_async.rs`) is the shape
that works. A plugin offers a layer by name, and the generated `src/app.rs`
mounts it at a pattern the application chooses (PLAN.md §8). `sessions` is
one such layer.

**Hooks** (`brocanth/src/plugin_hooks.rs`) is the one that works.
`RequestHook::before_dispatch` may rewrite the request or short-circuit with
`Flow::Respond`; `AsyncRequestHook` is the same seam with an `.await`;
`ResponseHook::on_headers` runs for every response including a stream, and
`on_body` runs for a buffered one; `ErrorHook` turns an error into a
response. Hooks are ordered by declaration, not registration.

Everything below is a hook. Nothing below needs a layer, which is worth
saying plainly: the thing that looks like the middleware seam is not the
middleware seam. A layer never sees a 404, a 405 or a request a hook
answered; a hook sees all of them.

When this page was first written, no shipped crate implemented
`HookSource`: every implementation was a test fixture. `brocanth-middleware`
was the first real one. Today the edge hooks below, `logging`, the route
cache, `i18n`, `errors`, `audit`, `gtag`, `analytics`, `webhooks-in` and
`storage` all install hooks, in generated projects.

## 2. `gorilla/handlers`, mapped

| Gorilla | What it does | brocanth seam | State |
|---|---|---|---|
| `ProxyHeaders` | reads `X-Forwarded-For` / `-Proto` / `-Host` and RFC 7239 `Forwarded`, rewrites the request | `RequestHook` | **built** |
| `HTTPMethodOverrideHandler` | `X-HTTP-Method-Override`, or a `_method` form field on a POST | `RequestHook` | **built** |
| `CanonicalHost` | redirects to one host, keeping the path | `RequestHook` + `Flow::Respond` | **built** |
| `ContentTypeHandler` | 415 unless the request's media type is on a list | `RequestHook` + `Flow::Respond` | **built** |
| `CORS` | preflight answer, and the headers on the real response | `RequestHook` + `ResponseHook` | **built** |
| `CompressHandler` | gzip when `Accept-Encoding` says so | `ResponseHook::on_body`; a stream is left alone | **built**: `Compression`, gzip and brotli (§8) |
| `LoggingHandler` | Apache Common Log Format | `RequestHook::after_dispatch` | **built**: `logging`, `format = "common"` |
| `CombinedLoggingHandler` | Apache Combined Log Format | same | **built**: `logging`, `format = "combined"` |
| `CustomLoggingHandler` | the same with your own formatter | same | **built**: the plugin file is the project's own; change `Line::after_dispatch` |
| `RecoveryHandler` | catches a panic, answers 500 | the generated `handle`: `catch_unwind` around the hooks and dispatch | **built** (ADR 8) |

`CompressHandler` is the one with a real cost attached: `flate2` and
`brotli`. Both arrive behind the off-by-default `compression` feature.
`plugin_hooks.rs` names compression as the case `Streaming::wrap` exists
for, and the hook does not use it: a stream is never compressed. §8 says
why.

`RecoveryHandler` is not a hook at all. A generated `handle` runs the
request hooks, the chain and the page inside
`brocanth_server::bridge::catch_unwind`. A panic is logged and becomes a
500, and the response hooks and `after_dispatch` still see it, so a
response hook can rewrite what the 500 says and `logging` writes its line.
`Bridge`, for a hand-written `main`, has the same net. PLAN.md ADR 8
records it; the CLI's golden builds test it with a page that panics. One
limit: the `errors` module's `error.rsx` and its reporters do not hear a
panic, because the net answers before `catching` can. See G10 in
`docs/framework-gaps.md`.

The three logging handlers were one question, not three: whether the
`logging` module's line becomes a format choice. It did. `format` is
`line`, `common` or `combined`, and the plugin file is the project's, so a
custom format is an edit. See `docs/modules/logging.md`.

## 3. The rest of the toolkit

| Repository | What it is | brocanth | State |
|---|---|---|---|
| `gorilla/mux` → `CORSMethodMiddleware` | fills `Access-Control-Allow-Methods` from the routes actually registered for a path | `CorsConfig::route_verbs`, a function over the route table (`Router::allow_for`) | **built**, 2026-11-24 (§4) |
| `gorilla/csrf` | a token in a session, a hidden field, a check on unsafe verbs | `Csrf`, in this crate | **written**: a double-submit cookie, live in a generated project with `middleware-csrf` (§5) |
| `gorilla/securecookie` | signed, optionally encrypted cookie values | `brocanth-auth` signs session cookies | present in one place, not offered as a primitive |
| `gorilla/sessions` | a session store behind a cookie | the `sessions` module | present |
| `gorilla/schema` | fills a struct from form values | `brocanth-schema` validates, it does not decode a form | **done**: `brocanth-input`, and a form, a JSON body and a query string all read the same struct. See `docs/input.md` |
| `gorilla/websocket` | the WebSocket protocol | `brocanth_server::websocket` (feature `websocket`), over `brocanth_server::upgrade` | **written**: the handshake, an Origin check and tungstenite's codec; `realtime-websocket` uses it (`docs/realtime.md`) |

## 4. What this leaves

Two findings worth carrying into the plan.

**Form decoding is the real gap, not CSRF.** `gorilla/schema` looks like the
least interesting entry in the table and it is the one blocking the most.
Done since, and the gap was deeper than this paragraph said: a POST handler
did not read `Vec<u8>` and parse it by hand, it had no body to read.
`dispatch` took the verb and the path off the request and dropped the rest.
`docs/input.md` is what closed it.

**`CORSMethodMiddleware` is a case where brocanth can beat the original.**
Gorilla has to ask the router at request time which methods a path has.
Here the route table is complete when the binary is built, so the allowed
methods for a path are known before the first request — and so is the
answer to a preflight for a path that exists with the wrong verb.

Built, 2026-11-24. `Cors` runs before dispatch and cannot name the
project's table, so `CorsConfig::route_verbs` takes a function from a path
to its verbs. The generated registration (commented, like the other four)
passes one over `pages::router()`:
`.route_verbs(move |path| pages.allow_for(path))`. Without the function, a
preflight answers `CorsConfig::methods` for every path, as before.

- **Decision: the allowed verbs are `CorsConfig::methods` intersected with
  the route's verbs.** Reason: the configuration stays the ceiling, so a
  new `DELETE` page is not open to other origins by accident.
- **Decision: a preflight for a verb the route lacks, or for a path with
  no route, gets no `Access-Control-Allow-*` header.** Reason: the browser
  then refuses the real request, as it does for a verb outside the list,
  and says why in its console. The response hook skips every preflight, so
  it cannot put `Access-Control-Allow-Origin` back on one that was refused.

Tests: the unit tests in `crates/brocanth-middleware/src/cors.rs`, and
`crates/brocanth-middleware/tests/cors.rs`, a `TestClient` over a GET-only
page and a GET + DELETE page.

## 5. CSRF

`brocanth_middleware::Csrf` is a `RequestHook` that refuses any verb that
is not GET, HEAD, OPTIONS or TRACE without a token, and a `ResponseHook`
that mints the token and sets the cookie. The comparison is constant time
and folds the two lengths in, so a short token costs what a long one
costs.

**Why double-submit and not a synchroniser token.** The scaffolded
`Session` carries one field, `cookie`, and it is the cookie *name*. There
is no per-session secret to bind a signed token to, so a plugin that
claimed to be a synchroniser token would be claiming a secret it does not
have. This one owns both halves: it mints the value, sets its own cookie,
and compares the field against that cookie. What it does not stop is
written in the module header -- chiefly somebody who can write a cookie on
this site's domain.

**In a generated project.** The hooks run. Two pieces of the generator
were missing, and both are in place now:

1. `plugins::Hooks` is built from the registrations: each hooking plugin
   adds its type to the record (`Slot::PluginsHooks`), as `Installed` is
   built from the provider list.
2. A generated `src/app.rs` runs each request through
   `App::begin_async` and `App::finish`, so the request hooks, the
   handler and the response hooks all run. `begin_async` also runs an
   `AsyncRequestHook`.

`middleware-csrf` registers `Csrf` from its `[csrf]` section, with
`plugin_hooking_from_config` when the project has `config`. The five
hooks of `middleware` are commented out: each one encodes a fact about the
deployment, so you turn it on yourself.

The two gaps above were found by generating a project, serving it and
posting to it, not by reading. The dp-swap gap G2 is closed: `dp::SCRIPT`
sends the token in the `x-csrf-token` header this hook reads, from
`CsrfToken::meta_tag`. See `src/dp.rs`.

## 6. Security headers

`brocanth_middleware::SecurityHeaders` is a `ResponseHook`. It sets six
headers on every response, a stream included. A header the handler set
is kept.

| Header | Default |
|---|---|
| `Strict-Transport-Security` | `max-age=31536000`, on a request known to be HTTPS |
| `X-Content-Type-Options` | `nosniff` |
| `Referrer-Policy` | `strict-origin-when-cross-origin` |
| `X-Frame-Options` | `DENY`, with `frame-ancestors 'none'` to match |
| `Content-Security-Policy` | `'self'` for everything, `object-src 'none'`, dp-swap's scripts by hash |
| `Permissions-Policy` | camera, microphone, geolocation, payment and USB off |

```rust
let builder = builder.plugin_hooking(SecurityHeaders::new(
    SecurityHeadersConfig::default()
        .behind_tls()
        .hsts(Hsts::max_age(ONE_YEAR).include_subdomains())
        .csp(Csp::default().directive("img-src", ["'self'", "https://img.example.com"])),
));
```

**HSTS goes out only on HTTPS.** A browser ignores HSTS over plain HTTP,
and HSTS from a development server pins `localhost` for a year. The hook
sends it when the request URI is `https`, when `ProxyHeaders` read
`proto=https` from a trusted proxy, or when `behind_tls` is set.
brocanth-server does not terminate TLS, so a production deployment
needs one of the last two. `preload` is refused at startup unless
`includeSubDomains` is on and `max-age` is at least a year, which are
the preload list's own rules.

**The CSP uses hashes, not a nonce.** dp-swap's scripts reach a page as
constants a template inlines: `<script>{! brocanth_middleware::dp::SCRIPT }</script>`.
A nonce must be on the header and on each tag, so every template that
inlines a script needs the request's nonce. The admin layout and the
jobs tab inline from a constant and have no request in reach. A nonce
also makes every page unique, which a shared cache must not store. A
hash of a constant is a constant: no template changes, and a cached page
stays valid. So `script-src` is `'self'` and the SHA-256 of `dp::SCRIPT`
and `dp::PREFETCH`. A script the application inlines itself goes in with
`Csp::inline_script(text)`, which hashes the exact text. The admin's own
`SCRIPT` is one such script. The cost: an inline script whose text
depends on the request has no fixed hash. Put that value in a `data-`
attribute or a `<meta>` tag, as `CsrfToken::meta_tag` does.

`style-src` keeps `'unsafe-inline'` by default. The backoffice screens
carry `<style>` blocks and `style=` attributes, and a policy that breaks
them gets turned off rather than tuned. `Csp::strict_styles()` drops it.

**Configuration.** With the `config` feature, every config type
deserialises, and the plugin's instance name is its section:

```toml
[security_headers]
behind_tls = true
frames = "same_origin"           # "deny", "same_origin" or { allow = [...] }

[security_headers.hsts]
include_subdomains = true

[security_headers.csp.directives]
img-src = ["'self'", "https://img.example.com"]   # one directive, the rest stay
```

```rust
use brocanth_config::AppBuilderConfigExt;

let builder = builder
    .with_config(config)
    .plugin_hooking_from_config::<SecurityHeaders>("security_headers")?;
```

`plugin_hooking_from_config` is `plugin_from_config` for a plugin that
installs hooks: the instance takes the section's name, so the section is
claimed, and the hook record is widened. `plugin_with_defaults` cannot
register a hooking plugin: that path calls `plugin`, and `build()`
refuses a hooking plugin added that way.

For a value only code can give, `plugin_hooking_from_config_with` takes a
closure over the configuration. The file still decides every key the
closure leaves alone:

```rust
let builder = builder.plugin_hooking_from_config_with::<SecurityHeaders>(
    "security_headers",
    |headers| headers.inline_script(brocanth_admin::INLINE_SCRIPT),
)?;
```

**In a generated project.** The `middleware-security-headers` module
declares `behind_tls`, `nosniff`, `referrer_policy`,
`permissions_policy`, `hsts.*` and `csp.enabled`, `csp.report_only`,
`csp.report_uri`. Each is a key in `[security_headers]` with `config`,
and a flag (`--security_headers.behind_tls=true`) and an environment
variable either way. `frames` and `csp.directives` are file-only: they
do not parse from one string. See `docs/module-flags.md`.

**`brocanth check --deploy`.** `deploy/hsts` now looks for this hook.
It warns when no hook and no `Strict-Transport-Security` literal is in
`src/`, when the hook is registered with HSTS off, and when nothing tells
the hook a request came over TLS: no `behind_tls` and no `ProxyHeaders`.

## 7. Rate limiting

A fixed window per key, per rule. A rule is a group of routes (path
prefixes by segment, and optional verbs), a limit, a window in seconds,
and a list of keys. A request counts against every rule that matches.
Over the limit, it gets a 429 and the handler does not run:

```text
HTTP/1.1 429 Too Many Requests
retry-after: 42
ratelimit-limit: 5
ratelimit-remaining: 0
ratelimit-reset: 42
ratelimit-policy: 5;w=60
```

A response that went through carries the same `RateLimit-*` headers for
the rule nearest its limit.

```rust
let builder = builder.plugin_hooking(RateLimit::new(
    RateLimitConfig::rules([
        Rule::new("login", 5, 60).paths(["/login"]).methods(["POST"]),
        Rule::new("api", 600, 60).paths(["/api"]).key([Key::User, Key::Ip]),
    ])
    .ip_header("fly-client-ip"),
));
```

**Fixed window, not token bucket.** A fixed window is one row and one
statement. A token bucket needs the last refill time, so a read before
the write or a longer statement, for a smoother curve. The fixed
window's cost is the edge: a caller can spend a limit at the end of one
window and another at the start of the next. For a password guesser or
a scraper that is acceptable.

**The key.** The first key in a rule's list that the request has:

- `ip`: the `ip_header` if set, then `ClientInfo` from `ProxyHeaders`,
  then the socket's `PeerAddress`. Name an `ip_header` only when the
  edge sets it and strips the client's copy.
- `session`: the SHA-256 of the cookie named by `session_cookie`. The
  session id never reaches the store.
- `user`: a `rate_limit::Subject` the application's auth hook puts on
  the request state. Order the limiter after that hook with `after`.

A request with none of a rule's keys is not counted by that rule. One
shared "unknown" bucket would let one caller lock out every other
caller without a key.

**Two plugins.** `RateLimit` counts in a `MemoryStore` with a
synchronous hook. It works with `App::run`, `Bridge` and
`TestClient::send`, and each process counts alone. `SharedRateLimit`
takes any `rate_limit::Store`. A store that waits on the network needs
an `AsyncRequestHook`, and an application with one must serve through
`App::begin_async`. The core makes `App::run` a compile error for it.
A generated `src/app.rs` and `brocanth_server::Bridge` both call
`begin_async`, so both serve `SharedRateLimit`; in a test, use
`TestClient::send_async`. The
`middleware-rate-limit` module registers `RateLimit`; for
`SharedRateLimit` and `PgStore`, replace that registration by hand.

**The Postgres store.** `rate_limit::PgStore`, behind the `postgres`
feature. One `UNLOGGED` table, because a counter is not worth a WAL
entry and a crash that empties it starts every caller on a fresh
window. One statement per count:

```sql
INSERT INTO brocanth_rate_limits AS r (key, window_end, hits)
VALUES ($1, (floor(extract(epoch FROM now()))::bigint / $2 + 1) * $2, 1)
ON CONFLICT (key) DO UPDATE SET
    hits = CASE WHEN r.window_end = EXCLUDED.window_end THEN r.hits + 1 ELSE 1 END,
    window_end = EXCLUDED.window_end
RETURNING hits, window_end - floor(extract(epoch FROM now()))::bigint
```

The window comes from the database's clock, so processes with skewed
clocks agree. The row lock of `ON CONFLICT DO UPDATE` serialises two
counts of one key, and the test sends fifty at once to prove none is
lost. `PgStore::create_table` creates the table, `create_sql` gives the
statement for a migration, and `sweep` deletes ended windows. Run the
sweep from a job.

**When the store fails.** `fail_open` is true by default: the request
goes through and a `StoreFailed` is on the request state. A limiter
that is down should not take the site down. `fail_closed()` answers 503
instead.

**Configuration.** As for the headers, under the `config` feature:

```toml
[rate_limit]
ip_header = "fly-client-ip"

[[rate_limit.rules]]
name = "login"
paths = ["/login"]
methods = ["POST"]
limit = 5
window_secs = 60
key = ["ip"]                    # the default
```

In a generated project, `ip_header`, `session_cookie` and `fail_open` are
also flags (`--rate_limit.ip_header=fly-client-ip`) and environment
variables. The rules are file-only, as `[[rate_limit.rules]]`: a rule is
a record, and a flag has no spelling for a list of records. Without
`config` the example rule is code beside the registration.

CSRF reads `[csrf]` the same way: `cookie`, `field`, `header` and
`secure` are flags too, and `exempt` is file-only.

**What is open.**

- The limits are per process with `RateLimit`. A deployment with more
  than one process and a real limit wants `SharedRateLimit`.

## 8. Conditional GET and compression

Two response hooks for the bytes on the wire. `ConditionalGet` is in the
default `runtime`: it needs only `sha2` and `base64`, which the crate
already takes. `Compression` is behind the `compression` feature.

```rust
let builder = builder
    .plugin_hooking(ConditionalGet::new(ConditionalGetConfig::default()))
    .plugin_hooking(Compression::new(CompressionConfig::default()));
```

**Conditional GET.** For a `GET` or `HEAD` that answers `200` with a
buffered body, the hook sets a weak `ETag` over the body when the page set
none: `W/"…"`, 128 bits of its SHA-256 in URL-safe base64. A matching
`If-None-Match` (weak comparison) is a `304`. Without `If-None-Match`, an
`If-Modified-Since` at or after the page's `Last-Modified` is a `304`.
A `304` keeps the headers and drops the body, `Content-Type`,
`Content-Length`, `Content-Encoding` and `Content-Language`. The page still
renders; the hook saves the transfer. `Cache-Control: no-store` gets no
tag. Only the IMF-fixdate is read; an obsolete date is no date, so the
page goes out in full.

**Why the tag is weak.** A strong tag names exact bytes, and compression
changes them. Rails and nginx make the same choice. The weak tag is over the
identity body, so one tag is good for the brotli, the gzip and the plain
copy, and a `304` is decided before anything is compressed.

**Compression.** The hook reads `Accept-Encoding` with its q-values and
`*`, and picks `br` or `gzip` (brotli on a tie). It compresses a buffered
body of `min_bytes` or more (1024) whose media type is on `types`
(`text/*`, JSON, JavaScript, XML, RSS, Atom, the web manifest, SVG). It
leaves alone a body with a `Content-Encoding`, a status with no body or a
byte range (`1xx`, `204`, `205`, `206`, `304`), `Cache-Control:
no-transform`, `text/event-stream`, and a result that is not smaller. Every
response it could compress gets `Vary: Accept-Encoding`, also for a client
that asked for none. A strong `ETag` on a compressed body becomes weak.

Defaults: gzip level 6, brotli quality 4. Quality 11 takes about fifty times
longer than 4 for a few percent, which suits a file compressed once at build
time and not a page compressed per request.

**Why a stream is never compressed.** An SSE event must reach the browser
when the handler writes it. A compressor holds bytes until its window fills;
a flush per event keeps the latency and loses most of the ratio. A
streamed download is usually a file that is already compressed, and a
`101` has no body. So the hook neither wraps a stream nor announces an
encoding for one.

**Order, from the phases.** Response hooks run `Cache`, `Guard`,
`Identify`, then `Encode` (ADR 10), and the right order on the way out is:

```text
handler -> route_cache.store (Cache) -> flash / gtag / beacon (Identify)
        -> conditional_get.check (Encode) -> compression.encode (Encode, first) -> client
```

The route cache stores the identity body, and a hit is tagged and
compressed again for the client that asks. Both encoding hooks are in
`Phase::Encode`. `Compression` is `Order::first()` there, and response
hooks leave a phase in reverse, so it touches the body last. No hook names
another, so the order holds in any registration order and under any
instance name. An application hook in `Encode` that rewrites the body goes
in `CompressionConfig::runs_after`; one in another phase needs nothing.

**In a generated project.** `middleware-conditional-get` and
`middleware-compression` are children of `middleware`, off until ticked.
Off, because a CDN in front often does both, and compression twice costs
CPU and gains nothing. Each reads its section, `[conditional_get]` and
`[compression]`, with `config`.

**What is open.**

- A stream gets no tag and no `304`. A handler that streams a file and
  knows its version compares `If-None-Match` itself.
- `If-Match` and `If-Unmodified-Since` are not read. They guard a write,
  and the write handler checks them against the row it changes.
- The static files `rsx` serves go through a server wrap, outside the
  hooks, so neither hook sees them. They carry `brocanth-assets`'s strong
  content-hash `ETag` and go out uncompressed. Precompressed copies at
  build time are the next step there, not a hook.
