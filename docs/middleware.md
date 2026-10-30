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
