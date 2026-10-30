# Middleware

What the Gorilla toolkit provides, what brocanth has, and where each one
lands on this framework's seams.

Gorilla's inventory was read from `pkg.go.dev/github.com/gorilla/handlers`
on 2026-10-22, and from the toolkit's other repositories by name. The
brocanth column comes from reading this checkout, not from a crate list.

## 1. The seam

brocanth has two middleware seams, and only one of them is used.

**`Layer`** (`brocanth/src/routing_middleware.rs`) is a pattern-scoped chain
that runs before dispatch. Its own header records that nothing mounts a
layer, and that an `await` inside one does not compile: the output type is a
trait parameter, fixed when the stack is built, so it cannot name the borrow
of the chain that an async layer must hold. `docs/async-handlers.md` §4 has
both compiler errors in full.

**Hooks** (`brocanth/src/plugin_hooks.rs`) is the one that works.
`RequestHook::before_dispatch` may rewrite the request or short-circuit with
`Flow::Respond`; `AsyncRequestHook` is the same seam with an `.await`;
`ResponseHook::on_headers` runs for every response including a stream, and
`on_body` runs for a buffered one; `ErrorHook` turns an error into a
response. Hooks are ordered by declaration, not registration.

Everything below is a hook. Nothing below needs the `Layer` stack, which is
worth saying plainly: the thing that looks like the middleware seam is not
the middleware seam.

**No shipped crate implements `HookSource` today.** Every implementation in
the repository is a test fixture. `brocanth-middleware` is the first real
one, which is why the first tranche is deliberately the dullest six.

## 2. `gorilla/handlers`, mapped

| Gorilla | What it does | brocanth seam | State |
|---|---|---|---|
| `ProxyHeaders` | reads `X-Forwarded-For` / `-Proto` / `-Host` and RFC 7239 `Forwarded`, rewrites the request | `RequestHook` | **built** |
| `HTTPMethodOverrideHandler` | `X-HTTP-Method-Override`, or a `_method` form field on a POST | `RequestHook` | **built** |
| `CanonicalHost` | redirects to one host, keeping the path | `RequestHook` + `Flow::Respond` | **built** |
| `ContentTypeHandler` | 415 unless the request's media type is on a list | `RequestHook` + `Flow::Respond` | **built** |
| `CORS` | preflight answer, and the headers on the real response | `RequestHook` + `ResponseHook` | **built** |
| `CompressHandler` | gzip when `Accept-Encoding` says so | `ResponseHook::on_body`, plus `Streaming::wrap` for a stream | open, needs a deflate implementation |
| `LoggingHandler` | Apache Common Log Format | `RequestHook::after_dispatch` | partly: `logging` writes a line, not CLF |
| `CombinedLoggingHandler` | Apache Combined Log Format | same | open |
| `CustomLoggingHandler` | the same with your own formatter | same | open |
| `RecoveryHandler` | catches a panic, answers 500 | **core**: `catch_unwind` around dispatch | open |

`CompressHandler` is the one with a real cost attached. gzip means
`flate2`, and this workspace spends dependencies carefully, so it will
arrive behind an off-by-default feature. The stream half is the interesting
half: `BodyShape::Streaming` never reaches `on_body`, so a compressing hook
has to wrap the stream rather than rewrite a buffer. `plugin_hooks.rs`
names compression as the case that wrap exists for.

`RecoveryHandler` is not a hook at all. `brocanth-server/src/bridge.rs`
already has a `catch_unwind`, so the edge is covered; what is missing is
recovery *inside* dispatch, where the application's own handler runs, with
a hook that can decide what the 500 says. That is a core change and it is
not in this crate.

The three logging handlers are one question, not three: whether the
`logging` module's line becomes a format choice. CLF and Combined are two
format strings over fields brocanth already has.

## 3. The rest of the toolkit

| Repository | What it is | brocanth | State |
|---|---|---|---|
| `gorilla/mux` → `CORSMethodMiddleware` | fills `Access-Control-Allow-Methods` from the routes actually registered for a path | the route table knows this | open, and better here than in Gorilla: the table is complete at build time |
| `gorilla/csrf` | a token in a session, a hidden field, a check on unsafe verbs | `Csrf`, in this crate | **written**: a double-submit cookie, and §5 says what is still between it and a generated project |
| `gorilla/securecookie` | signed, optionally encrypted cookie values | `brocanth-auth` signs session cookies | present in one place, not offered as a primitive |
| `gorilla/sessions` | a session store behind a cookie | the `sessions` module | present |
| `gorilla/schema` | fills a struct from form values | `brocanth-schema` validates, it does not decode a form | **done**: `brocanth-input`, and a form, a JSON body and a query string all read the same struct. See `docs/input.md` |
| `gorilla/websocket` | the WebSocket protocol | `realtime-websocket` is on the menu and blocked | open; needs an HTTP upgrade, a second exit from `brocanth-server` |

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

## 5. CSRF, and what it is waiting for

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

**What is between it and a generated project.** Two pieces of the
generator, and neither is in this crate:

1. `plugins::Hooks` is written as `NoHooks` whatever registered.
   `plugin_hooking` widens that record, so the registration does not
   compile until the alias is built the way `Installed` already is.
2. `dispatch` in a generated `src/app.rs` goes from the route table to the
   handler. The hook pipeline is `App::run`, which that file never calls,
   so no hook in this crate runs in a generated project -- the five above
   included. `brocanth_server::Bridge` is the piece that runs it.

Both were found by generating a project, serving it and posting to it, not
by reading. The dp-swap gap G2 is closed: `dp::SCRIPT` sends the token in
the `x-csrf-token` header this hook reads, from `CsrfToken::meta_tag`. See
`src/dp.rs`.

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
let headers = config.section::<SecurityHeadersConfig>("security_headers")?;
let builder = builder.plugin_hooking(SecurityHeaders::new(headers));
```

`plugin_with_defaults` cannot register it: that path calls `plugin`,
and a plugin that installs hooks must go through `plugin_hooking`.
Registering it by its default name claims the section all the same.

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
`brocanth_server::Bridge` calls `App::run` and `App::begin` today, never
`begin_async`, so `SharedRateLimit` is served by a project that drives
`begin_async` itself.

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

**What is open.**

- `ProxyHeaders` and the limiter read `PeerAddress`. brocanth-server's
  bridge sets its own `brocanth_server::Peer`. Nothing converts one into
  the other, so behind the bridge neither sees the socket address. The
  limiter still works with `ip_header`.
- `SharedRateLimit` cannot be served by `Bridge` until the bridge drives
  `App::begin_async`.
- The limits are per process with `RateLimit`. A deployment with more
  than one process and a real limit wants `SharedRateLimit`.
