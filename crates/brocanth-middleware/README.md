# brocanth-middleware

The edge hooks that every application otherwise writes by hand: proxy
headers, method override, canonical host, content types, CORS, CSRF,
security headers, rate limiting, and the server half of dp-swap. Each hook
is its own plugin. You register only the hooks you need. Use this crate for
a fact about the edge of the application: who the client is, which origin
may call, which form post is real. Do not use it for compression, Common
Log Format or panic recovery. This crate does not have them
(`docs/middleware.md` §2 says why).

## The hooks

| Plugin | Instance | Hooks (`instance.hook`) | Answers |
|---|---|---|---|
| `ProxyHeaders` | `proxy-headers` | `read` (request) | Who the client is, behind a trusted proxy. Puts a `ClientInfo` on the request state. |
| `MethodOverride` | `method-override` | `rewrite` (request) | A `DELETE`, `PUT` or `PATCH` sent from an HTML form, through `_method` or `X-HTTP-Method-Override`. Only a POST is overridden. |
| `CanonicalHost` | `canonical-host` | `redirect` (request) | One host. Any other host gets a redirect that keeps the path. |
| `ContentTypes` | `content-types` | `check` (request) | 415 before a handler parses a body of the wrong type. |
| `Cors` | `cors` | `preflight` (request), `mark` (response) | A browser on another origin. A wildcard with credentials stops the build. |
| `Csrf` | `csrf` | `check` (request), `mint` (response) | A form post this application handed out. A double-submit cookie. |
| `DpSwap` | `dp-swap` | `read` (request), `vary` (response) | dp-swap's `DP-*` request headers, `Vary`, typed response directives and out-of-band fragments (module `dp`). |
| `SecurityHeaders` | `security_headers` | `set` (response) | HSTS (over HTTPS only), `nosniff`, `Referrer-Policy`, framing, a CSP that allows dp-swap's scripts by hash, `Permissions-Policy`. |
| `RateLimit` | `rate_limit` | `count` (request), `headers` (response) | A fixed window per address, session or user, per group of routes. 429 with `Retry-After`. Counts in this process. |
| `SharedRateLimit` | `rate_limit` | `count` (async request), `headers` (response) | The same, over any `rate_limit::Store`, for example `rate_limit::PgStore`. |

## How the hooks run in the pipeline

A plugin that installs hooks is registered with `plugin_hooking`, not
`plugin`. `build()` refuses `plugin` for such a plugin and names the line
to write. Each `plugin_hooking` call widens the application's hook record
by the plugin's `HookSource::Families`, so the application's type names
every hook it runs.

For one request, the core runs:

```text
before_dispatch   request hooks, declared order           a hook may answer (Flow::Respond)
    dispatch      the handler
on_headers        response hooks, reverse declared order  every response, a stream too
on_body           response hooks, reverse declared order  a buffered body only
after_dispatch    request hooks, reverse declared order   read-only
```

- **The order is declared, not discovered.** A hook declares `after` and
  `before` constraints (`brocanth::plugin_order::Order`). Registration
  order breaks a tie only. In this crate:
  - `csrf.check` runs after `method-override.rewrite`, if that hook is
    present.
  - `rate_limit.count` runs after `proxy-headers.read`, if present, so
    `ClientInfo` is there to read. `RateLimitConfig::after("auth.session")`
    adds a constraint, for a `user` key that another hook puts on the
    request state.
  - The other hooks declare nothing and keep their registration order.
- **A request hook may answer.** `CanonicalHost` (a redirect),
  `ContentTypes` (415), `Cors` (a preflight), `Csrf` (403) and the rate
  limiter (429) return `Flow::Respond`. The handler does not run. The
  response hooks still run on that answer, so a 403 also gets the security
  headers.
- **A response hook keeps the handler's header.** `SecurityHeaders` does not
  replace a header the handler set.
- **An async hook changes the entry point.** `SharedRateLimit` installs an
  `AsyncRequestHook`. An application with one must call `App::begin_async`,
  then `App::finish`. `App::run` and `App::begin` are a compile error for
  it. A generated `src/app.rs` already calls `begin_async` and `finish`.
  `brocanth_server::Bridge` calls `App::run` and `App::begin` only.

## Getting started

```toml
[dependencies]
brocanth-middleware = { git = "https://github.com/DHHwasRight/brocanth.git" }
# Optional features:
#   config    - CsrfConfig, SecurityHeadersConfig and RateLimitConfig deserialise from a brocanth-config section
#   postgres  - rate_limit::PgStore, the shared counter store (one UNLOGGED table)
#   scaffold  - the `brocanth create` modules; only the CLI turns it on
```

Inside this workspace: `brocanth-middleware.workspace = true`.

### As a module

```text
brocanth create my-app --with middleware
brocanth add middleware
```

| Key | Default with `middleware` | Writes |
|---|---|---|
| `middleware` | | `brocanth-middleware` in `Cargo.toml`, and five commented registrations (`ProxyHeaders`, `MethodOverride`, `CanonicalHost`, `ContentTypes`, `Cors`). Each one encodes a fact about your deployment, so you turn it on yourself. |
| `middleware-csrf` | on | A live `Csrf` registration, built from its `[csrf]` settings. |
| `middleware-security-headers` | on | A live `SecurityHeaders` registration, built from its `[security_headers]` settings. With `admin`, the CSP also allows the admin's inline script by hash. |
| `middleware-rate-limit` | off | A live `RateLimit` registration, built from its `[rate_limit]` settings, with one example rule (`login`, 5 per 60 s). Replace the numbers. |

To turn on one of the five commented hooks, copy its line to `build()` in
`src/app.rs`, after `plugins::register!`, and widen `type Core` by the
plugin's hook families. The generated comment shows the type.

### Settings

The three live modules declare their settings. Each setting is a key in
the plugin's section of `config/<env>.toml` when the project has
`config`, and a flag and an environment variable either way. The section
is the plugin's instance name: `[csrf]`, `[security_headers]`,
`[rate_limit]`.

| Section | Keys | File only |
|---|---|---|
| `[csrf]` | `cookie`, `field`, `header`, `secure` | `exempt` |
| `[security_headers]` | `behind_tls`, `nosniff`, `referrer_policy`, `permissions_policy`, `hsts.enabled`, `hsts.max_age`, `hsts.include_subdomains`, `hsts.preload`, `csp.enabled`, `csp.report_only`, `csp.report_uri` | `frames`, `csp.directives` |
| `[rate_limit]` | `ip_header`, `session_cookie`, `fail_open` | `[[rate_limit.rules]]` |

```text
cargo run -- --security_headers.behind_tls=true
MY_APP_RATE_LIMIT__IP_HEADER=fly-client-ip cargo run
```

A list has no flag, so `exempt` and the rate-limit rules live in the file.
Without `config`, the example rule is code beside the registration.

The generated registration calls
`plugin_hooking_from_config::<P>("<section>")` from `brocanth-config`.
With `admin`, security headers use `plugin_hooking_from_config_with`,
which adds the admin's script hash on top of what the file says. The
module turns on this crate's `config` feature only when the project has
`config`. See `docs/module-flags.md`, "Hooking plugins from a library
crate".

The five commented hooks of `middleware` declare no settings. You change
their configuration in the Rust literal you copy into `src/app.rs`.

An application without the generator can read a section itself. With the
`config` feature, `CsrfConfig`, `SecurityHeadersConfig` and
`RateLimitConfig` deserialise. See the last example below.
`ProxyHeadersConfig`, `MethodOverrideConfig`, `CanonicalHostConfig`,
`ContentTypesConfig` and `CorsConfig` do not deserialise.

## Examples

### CSRF: refuse a post that did not send the token back

`Csrf` mints a token on a safe request and sets it as its own cookie. A
request with any verb other than GET, HEAD, OPTIONS or TRACE must send the
same value back in the `_csrf_token` field or the `X-CSRF-Token` header.
A page reads the token from the request state as `CsrfToken` and writes
`token.hidden_field()` in each form.

```rust
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_middleware::{Csrf, CsrfConfig, CsrfToken};
use brocanth_test::{TestClient, TestRequest};

fn form() -> Handler<Base> {
    Box::new(|cx: &mut RouteCx<'_, Base>| {
        let field = cx.state().get::<CsrfToken>().map(CsrfToken::hidden_field);
        let html = format!(r#"<form method="post">{}</form>"#, field.unwrap_or_default());
        Ok(http::Response::new(Body::from(html.into_bytes())))
    })
}

fn saved() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| Ok(http::Response::new(Body::from(b"saved".to_vec()))))
}

let app = AppBuilder::new()
    // `insecure()` drops `Secure` from the cookie. Development only.
    .plugin_hooking(Csrf::new(CsrfConfig::default().insecure()))
    .build()
    .expect("an application");
let mut table = Router::builder();
table.route("/items", Verb::Get, form()).expect("a route");
table.route("/items", Verb::Post, saved()).expect("a route");
let client = TestClient::serving(app, table.build().expect("a table"));

// A first visit gets the cookie, and the page carries the same value.
let page = client.get("/items");
let cookie = page.header("set-cookie").expect("a cookie");
let token = cookie
    .split(';')
    .next()
    .and_then(|pair| pair.split_once('='))
    .map(|(_, value)| value.to_string())
    .expect("name=value");
assert!(page.text().contains(&token));

// No token: refused before the handler runs.
client
    .send(TestRequest::post("/items").form(&[("name", "lamp")]))
    .assert_status(403);

// The cookie and the field agree: accepted.
client
    .send(
        TestRequest::post("/items")
            .header("cookie", &format!("csrf_token={token}"))
            .form(&[("name", "lamp"), ("_csrf_token", &token)]),
    )
    .assert_ok();
```

A webhook or another caller that is not a browser goes in
`CsrfConfig::default().exempt(["/webhooks"])`. Do not remove the
registration.

### Security headers, with a CSP that allows an inline script by hash

```rust
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_middleware::{script_hash, Csp, Hsts, SecurityHeaders, SecurityHeadersConfig, ONE_YEAR};
use brocanth_test::TestClient;

const MY_SCRIPT: &str = "document.body.dataset.ready = 'yes';";

fn page() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| {
        let html = format!("<!DOCTYPE html><body><script>{MY_SCRIPT}</script></body>");
        Ok(http::Response::new(Body::from(html.into_bytes())))
    })
}

let config = SecurityHeadersConfig::default()
    // A TLS terminator is in front, so every request counts as HTTPS.
    .behind_tls()
    .hsts(Hsts::max_age(ONE_YEAR).include_subdomains())
    .csp(Csp::default().inline_script(MY_SCRIPT));

let app = AppBuilder::new()
    .plugin_hooking(SecurityHeaders::new(config))
    .build()
    .expect("an application");
let mut table = Router::builder();
table.route("/", Verb::Get, page()).expect("a route");
let client = TestClient::serving(app, table.build().expect("a table"));

let response = client.get("/");
response
    .assert_header("x-content-type-options", "nosniff")
    .assert_header("x-frame-options", "DENY")
    .assert_header("strict-transport-security", "max-age=31536000; includeSubDomains");
let csp = response.header("content-security-policy").expect("a policy");
assert!(csp.contains(&script_hash(MY_SCRIPT)));
```

Without `behind_tls()` and without a trusted `ProxyHeaders` that read
`proto=https`, a plain HTTP request gets no HSTS. That keeps a development
server from pinning `localhost` for a year.

### Rate limit a login form

```rust
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_middleware::{RateLimit, RateLimitConfig, Rule};
use brocanth_test::{TestClient, TestRequest};

fn ok() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| Ok(http::Response::new(Body::from(b"ok".to_vec()))))
}

// Two tries per minute per address, on POST /login only.
let config = RateLimitConfig::rules([
    Rule::new("login", 2, 60).paths(["/login"]).methods(["POST"]),
])
// Name a header only when your edge sets it and strips the client's copy.
.ip_header("fly-client-ip");

let app = AppBuilder::new()
    .plugin_hooking(RateLimit::new(config))
    .build()
    .expect("an application");
let mut table = Router::builder();
table.route("/login", Verb::Post, ok()).expect("a route");
let client = TestClient::serving(app, table.build().expect("a table"));

let attempt = || TestRequest::post("/login").header("fly-client-ip", "203.0.113.7");
client.send(attempt()).assert_ok();
client.send(attempt()).assert_ok();
let refused = client.send(attempt());
refused.assert_status(429);
assert!(refused.header("retry-after").is_some());
```

A request that has none of a rule's keys is not counted by that rule.
With more than one process, use `SharedRateLimit` with
`rate_limit::PgStore` (feature `postgres`).

### Name the hook record, as a generated project does

`plugin_hooking` changes the application's type. A project that keeps its
`App` in a struct field spells that type. One `Both` per hooking plugin,
innermost first:

```rust
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::{App, AppBuilder};
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{Both, HookSource, NoHooks};
use brocanth_middleware::{Csrf, CsrfConfig, ProxyHeaders, ProxyHeadersConfig};

type Hooks = Both<
    <ProxyHeaders as HookSource>::Families,
    Both<<Csrf as HookSource>::Families, NoHooks>,
>;

let app: App<Base, Hooks> = AppBuilder::new()
    .plugin_hooking(Csrf::new(CsrfConfig::default()))
    .plugin_hooking(ProxyHeaders::new(ProxyHeadersConfig::trusting([
        "10.0.0.1".parse().expect("an address"),
    ])))
    .build()
    .expect("an application");
assert!(app.plugin_named::<Csrf>("csrf").is_some());
```

### Read `[security_headers]` and `[rate_limit]` from a config file

With the `config` feature:

```rust
use brocanth_config::Config;
use brocanth_middleware::{RateLimitConfig, SecurityHeadersConfig};

let config = Config::from_toml(
    "shop",
    "production",
    r#"
[security_headers]
behind_tls = true
frames = "same_origin"

[rate_limit]
ip_header = "fly-client-ip"

[[rate_limit.rules]]
name = "login"
paths = ["/login"]
methods = ["POST"]
limit = 5
window_secs = 60
"#,
)
.expect("TOML");

let headers: SecurityHeadersConfig = config.section("security_headers").expect("a section");
assert!(headers.behind_tls);
let limits: RateLimitConfig = config.section("rate_limit").expect("a section");
assert!(limits.check().is_ok());
```

Then register them with `plugin_hooking(SecurityHeaders::new(headers))`
and `plugin_hooking(RateLimit::new(limits))`. With `brocanth-config`'s
`AppBuilderConfigExt`, `plugin_hooking_from_config::<SecurityHeaders>("security_headers")`
reads the section and registers the plugin in one call.

## Testing

```text
cargo test -p brocanth-middleware
```

The crate's dev-dependencies turn on `config` and `postgres`, so this
runs the configuration tests and compiles the Postgres tests.
`tests/rate_limit_postgres.rs` runs only when `BROCANTH_TEST_DATABASE_URL`
is set. It skips otherwise. Each case uses its own table.

To test an application that uses these hooks, build the `App` with the
same `plugin_hooking` calls and send requests through
`brocanth_test::TestClient`, as the examples above do. The client runs the
request hooks, the handler and the response hooks, so a 403, a 429 or a
missing header shows up as it does in production. For `SharedRateLimit`,
use `TestClient::send_async`. See `crates/brocanth-test/README.md`.

## More

- `docs/middleware.md`: the Gorilla mapping, CSRF, security headers and
  rate limiting in depth, and what is still open.
- `docs/module-flags.md`: module settings, and how the hooking modules
  declare theirs.
- `docs/module-map.md`: where each module writes.
- `brocanth/src/plugin_hooks.rs` and `brocanth/src/plugin_order.rs`: the
  hook phases and the order rules.
