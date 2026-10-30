//! `middleware` -- what `brocanth create` writes when somebody asks for the
//! edge hooks.
//!
//! The module is here rather than in the CLI because everything it writes
//! is a statement about this crate: the names of five plugins, the fact
//! that each takes its own configuration struct, and the fact that they are
//! registered with `plugin_hooking` rather than `plugin`. Those move when
//! this crate moves, and a generator in another repository would learn
//! about it from a bug report.
//!
//! Compiled only under the `scaffold` feature, which is off.
//!
//! # One module, five plugins, none of them on by default
//!
//! Picking `middleware` writes the file and registers nothing. Each of the
//! five is a line the developer uncomments, because every one of them is a
//! decision about somebody else's deployment: which proxy to trust, which
//! host is canonical, which origins may call. A generator that guessed any
//! of those would be writing a security policy on behalf of a person who
//! has not described their network yet.
//!
//! That is the opposite of what `cache` does, and the difference is the
//! point: a cache has one obvious configuration and these have none.
//!
//! # And one that is
//!
//! [`Csrf`] is a second choice, drawn inside `middleware` and ticked with
//! it. It is separate because it is the one hook in this crate that is
//! not a decision about somebody's network: a page on another site can
//! post a form at this application and the browser sends the cookies with
//! it, and that is true wherever the application runs. So it arrives
//! ticked, configured, and with its own paragraph in `AGENTS.md`.
//!
//! Its registration is real code, and so are the security headers' and
//! the rate limit's. Each goes through [`snippet::hooking`], which also
//! writes the plugin into the generated `plugins::Hooks` record, and the
//! generated `src/app.rs` runs every request through `App::begin_async`
//! and `App::finish`. So a hook this module registers runs.
//!
//! [`snippet::hooking`]: brocanth_scaffold::snippet::hooking

use brocanth_scaffold::{snippet, Dep, Manifest, Module, Project, Scaffold, Slot};

/// The edge hooks, as a choice on the `brocanth create` menu.
pub struct Middleware;

impl Module for Middleware {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "middleware",
            "proxy headers, method override, canonical host, content types, CORS",
        )
        .asked_in("doors", "The edge hooks: proxies, CORS, a canonical host")
        .krate(Dep::brocanth("brocanth-middleware"))
        .absent(
            "brocanth-middleware",
            "no edge hooks; a forwarded address, a preflight and a canonical \
             host are each a handler's own problem",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // No file and no registration: five commented lines where they
        // would be uncommented, which is inside `build()` beside every
        // other plugin. A `src/plugins/middleware/mod.rs` holding the same
        // comments would be a second place to look.
        s.at(Slot::AppPlugins).block(REGISTRATIONS.to_string());
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The five, commented, with the decision each one encodes.
///
/// Registered with `plugin_hooking` and not `plugin`: a plugin that
/// installs hooks has to be added by that call, because that call is what
/// widens the application's hook-family record. `build()` refuses the
/// other one and names this line.
const REGISTRATIONS: &str = r#"
        // -- brocanth-middleware ------------------------------------------
        //
        // Five edge hooks, none of them on. Each encodes a fact about this
        // deployment, so each is turned on by somebody who knows it.
        //
        // This file is generated, so do not uncomment them here. Copy the
        // line to `build()` in `src/app.rs`, after `plugins::register!`,
        // and widen `type Core` there by the plugin's hook families:
        //
        //     brocanth::plugin_hooks::Both<
        //         <brocanth_middleware::Cors as brocanth::plugin_hooks::HookSource>::Families,
        //         plugins::Hooks,
        //     >

        // Who the client is, behind a proxy. The list is the addresses of
        // YOUR load balancers: `X-Forwarded-For` is a claim anybody who
        // reaches the socket may make, so there is no "trust everything".
        // let builder = builder.plugin_hooking(
        //     <brocanth_middleware::ProxyHeaders as brocanth::plugin_api::Plugin>::new(
        //         brocanth_middleware::ProxyHeadersConfig::trusting([
        //             "10.0.0.1".parse().expect("an address"),
        //         ]),
        //     ),
        // );

        // A DELETE sent from an HTML form, through `_method` or
        // `X-HTTP-Method-Override`. Only a POST is overridden, and only
        // into PUT, PATCH or DELETE.
        // let builder = builder.plugin_hooking(
        //     <brocanth_middleware::MethodOverride as brocanth::plugin_api::Plugin>::new(
        //         brocanth_middleware::MethodOverrideConfig::default(),
        //     ),
        // );

        // One host, so cookies and caches agree. 308 keeps the verb; 301
        // is what crawlers prefer and may turn a POST into a GET.
        // let builder = builder.plugin_hooking(
        //     <brocanth_middleware::CanonicalHost as brocanth::plugin_api::Plugin>::new(
        //         brocanth_middleware::CanonicalHostConfig::to("https://example.com"),
        //     ),
        // );

        // 415 before a handler parses nonsense.
        // let builder = builder.plugin_hooking(
        //     <brocanth_middleware::ContentTypes as brocanth::plugin_api::Plugin>::new(
        //         brocanth_middleware::ContentTypesConfig::json(),
        //     ),
        // );

        // A browser on another origin. Name the origins: a wildcard with
        // credentials is refused at startup, because a browser refuses it.
        // let builder = builder.plugin_hooking(
        //     <brocanth_middleware::Cors as brocanth::plugin_api::Plugin>::new(
        //         brocanth_middleware::CorsConfig::from(["https://app.example.com"]),
        //     ),
        // );
"#;

/// What the generated `AGENTS.md` says, so an agent reading the project
/// does not uncomment all five and call it hardening.
const AGENTS: &str = "\n\
    ## Edge middleware\n\
    \n\
    `src/plugins/mod.rs` holds five commented registrations from\n\
    `brocanth-middleware`. Each encodes a fact about the deployment --\n\
    which proxy to trust, which host is canonical, which origins may call\n\
    -- so each is turned on by somebody who knows that fact. Do not\n\
    enable them as a set, and do not invent an address or an origin to\n\
    make one compile. To turn one on, copy it to `build()` in\n\
    `src/app.rs` and widen `type Core` by its hook families; the comment\n\
    above the five shows the type.\n";

/// CSRF, as a choice on the menu that arrives ticked.
///
/// Part of `middleware` rather than a choice of its own, because the hook
/// lives in `brocanth-middleware` and that is the dependency the parent
/// brings. Ticked with it, because an application with forms and no CSRF
/// check is an application anybody can post to.
pub struct Csrf;

impl Module for Csrf {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "middleware-csrf",
            "a double-submit cookie, checked on every verb that is not a read",
        )
        .asked_in(
            "doors",
            "Refuse a form post this application did not hand out",
        )
        .part_of("middleware")
        .because("the hook is `brocanth_middleware::Csrf`, so the crate arrives with that choice")
        .after("middleware")
        .absent(
            "csrf",
            "no CSRF check; any page on any site can post a form here and the browser \
             sends the cookies with it",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        snippet::hooking(
            s,
            "middleware-csrf",
            "brocanth_middleware::Csrf",
            REGISTRATION.to_string(),
        );
        s.at(Slot::AgentsSections).block(CSRF_AGENTS.to_string());
    }
}

/// The registration, with the configuration already filled in.
const REGISTRATION: &str = r#"
        // -- brocanth-middleware: CSRF ------------------------------------
        //
        // Not one of the five above. Each of those encodes a fact about
        // this deployment; this one encodes a fact about browsers: any
        // page on any site can post a form here, and the cookies ride
        // along. That is true wherever this runs, so this hook is on, and
        // the only thing to decide is which paths are exempt -- a webhook,
        // or anything else whose caller is not a browser:
        //
        //     CsrfConfig::default().exempt(["/webhooks"])
        //
        // The token for the request is `ctx.hooked::<CsrfToken>()`;
        // `token.hidden_field()` is the input a form carries.
        let builder = builder.plugin_hooking(
            <brocanth_middleware::Csrf as brocanth::plugin_api::Plugin>::new(
                brocanth_middleware::CsrfConfig::default(),
            ),
        );
"#;

/// What the generated `AGENTS.md` says about forms.
const CSRF_AGENTS: &str = "\n\
    ## CSRF\n\
    \n\
    `brocanth_middleware::Csrf` is registered in `src/plugins/mod.rs`. It\n\
    is a double-submit cookie: the hook mints a token, sets it as its own\n\
    cookie, and refuses any verb that is not GET, HEAD, OPTIONS or TRACE\n\
    unless the request sends the same value back -- in the `_csrf_token`\n\
    field, or in the `X-CSRF-Token` header.\n\
    \n\
    So every form that posts carries the hidden field. In a page, read\n\
    the token with `ctx.hooked::<brocanth_middleware::CsrfToken>()` and\n\
    write `token.hidden_field()` inside the `<form>`. For HTML this\n\
    project did not template, `token.stamp_forms(html)` adds the field to\n\
    every posting form; the admin uses it.\n\
    \n\
    A 403 from this hook is a form that lost its token, or a caller that\n\
    is not a browser. Add that path to `exempt`; do not remove the\n\
    registration.\n";

/// The security headers, as a choice on the menu that arrives ticked.
///
/// Ticked for the reason CSRF is: every default it sends is a fact about
/// browsers, not about this deployment. The one header that depends on
/// the deployment is HSTS, and the hook sends it only on a request it
/// knows is HTTPS, so the default cannot pin `localhost`.
pub struct SecurityHeaders;

impl Module for SecurityHeaders {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "middleware-security-headers",
            "HSTS, nosniff, a referrer policy, no framing, a CSP that runs dp-swap by hash",
        )
        .asked_in("doors", "Send the headers a browser reads as policy")
        .part_of("middleware")
        .because(
            "the hook is `brocanth_middleware::SecurityHeaders`, so the crate arrives with that choice",
        )
        .after("middleware-csrf")
        .absent(
            "security-headers",
            "no security headers; a browser sniffs types, frames the pages and runs any inline script",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        // The admin layout inlines dp-swap's vendored copy, which the
        // default policy does not know. Its hash goes in, or the policy
        // blocks the admin's script.
        let config = if project.has("admin") {
            "brocanth_middleware::SecurityHeadersConfig::default().csp(\n\
             \x20                   brocanth_middleware::Csp::default()\n\
             \x20                       .inline_script(brocanth_admin::INLINE_SCRIPT),\n\
             \x20               ),"
        } else {
            "brocanth_middleware::SecurityHeadersConfig::default(),"
        };
        snippet::hooking(
            s,
            "middleware-security-headers",
            "brocanth_middleware::SecurityHeaders",
            SECURITY_HEADERS_REGISTRATION.replace("@CONFIG@", config),
        );
        s.at(Slot::AgentsSections)
            .block(SECURITY_HEADERS_AGENTS.to_string());
    }
}

/// The registration. `@CONFIG@` is the configuration expression.
const SECURITY_HEADERS_REGISTRATION: &str = r#"
        // -- brocanth-middleware: security headers ------------------------
        //
        // HSTS (over HTTPS only), nosniff, Referrer-Policy, X-Frame-Options
        // DENY, a Content-Security-Policy and a Permissions-Policy. The
        // policy allows dp-swap's two inline scripts by their SHA-256, so
        // there is no `'unsafe-inline'`. A script you inline yourself needs
        // `Csp::default().inline_script(TEXT)`.
        //
        // HSTS goes out only when the request is known to be HTTPS. Behind
        // a TLS terminator, say so: `SecurityHeadersConfig::default()
        // .behind_tls()`, or `behind_tls = true` in `[security_headers]`.
        //
        let builder = builder.plugin_hooking(
            <brocanth_middleware::SecurityHeaders as brocanth::plugin_api::Plugin>::new(
                @CONFIG@
            ),
        );
"#;

/// What the generated `AGENTS.md` says about the policy.
const SECURITY_HEADERS_AGENTS: &str = "\n\
    ## Security headers\n\
    \n\
    `brocanth_middleware::SecurityHeaders` is registered in\n\
    `src/plugins/mod.rs`. Its Content-Security-Policy\n\
    allows inline scripts by hash only. To inline a new script, add\n\
    `Csp::default().inline_script(TEXT)` with the exact text between the\n\
    tags. Do not add `'unsafe-inline'` to `script-src`. A value that\n\
    depends on the request goes in a `data-` attribute or a `<meta>` tag,\n\
    not in the script.\n\
    \n\
    HSTS is sent only over HTTPS. Set `behind_tls` when a TLS terminator\n\
    is in front. Set `include_subdomains` or `preload` only when the\n\
    person who owns the domain says so.\n";

/// Rate limiting, as a choice on the menu that arrives unticked.
///
/// Off, because a limit is a number about this application's traffic,
/// and the generator does not know that number.
pub struct RateLimit;

impl Module for RateLimit {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "middleware-rate-limit",
            "a fixed window per address, session or user, per group of routes; 429 with Retry-After",
        )
        .asked_in("doors", "Limit how often one caller may hit a route")
        .part_of_but_off("middleware")
        .because("the hook is `brocanth_middleware::RateLimit`, so the crate arrives with that choice")
        .after("middleware-security-headers")
        .absent(
            "rate-limit",
            "no rate limit; a password guesser gets as many tries as the server answers",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        snippet::hooking(
            s,
            "middleware-rate-limit",
            "brocanth_middleware::RateLimit",
            RATE_LIMIT_REGISTRATION.to_string(),
        );
        s.at(Slot::AgentsSections)
            .block(RATE_LIMIT_AGENTS.to_string());
    }
}

/// The registration, with an example rule that is marked as one.
const RATE_LIMIT_REGISTRATION: &str = r#"
        // -- brocanth-middleware: rate limit ------------------------------
        //
        // A fixed window per key. Each rule is a group of routes, a limit
        // and a window. THE RULE BELOW IS AN EXAMPLE for a login form, not
        // a recommendation: replace it with numbers from your own traffic.
        //
        // The key is the client address. Behind a proxy, register
        // `ProxyHeaders` above, or name the one header your edge sets and
        // strips: `.ip_header("fly-client-ip")`.
        //
        // This counts in this process. With more than one process, use
        // `SharedRateLimit` and `rate_limit::PgStore` (feature `postgres`).
        //
        let login = brocanth_middleware::Rule::new("login", 5, 60)
            .paths(["/login"])
            .methods(["POST"]);
        let builder = builder.plugin_hooking(
            <brocanth_middleware::RateLimit as brocanth::plugin_api::Plugin>::new(
                brocanth_middleware::RateLimitConfig::rules([login]),
            ),
        );
"#;

/// What the generated `AGENTS.md` says, so an agent does not invent a
/// limit.
const RATE_LIMIT_AGENTS: &str = "\n\
    ## Rate limit\n\
    \n\
    `brocanth_middleware::RateLimit` is registered in\n\
    `src/plugins/mod.rs` with one example rule (`login`, 5 per 60 s).\n\
    The numbers are an example. Do not invent limits: ask for the limit\n\
    and the window of each route group.\n\
    \n\
    A request whose key cannot be found is not counted. Behind a proxy,\n\
    the address comes from `ProxyHeaders` or from `ip_header`. Never set\n\
    `ip_header` to a header the client can send, such as\n\
    `X-Forwarded-For`, unless the edge overwrites it.\n";
