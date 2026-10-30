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
//! Its line is still commented, and that is a gap rather than the choice
//! the five make. Two pieces of generated code are in the way: the
//! `plugins::Hooks` alias, which says `NoHooks` whatever registered, and
//! a `dispatch` that goes from the route table to the handler without
//! entering the hook pipeline. The block this module writes names both
//! where somebody will read them, and the `AGENTS.md` paragraph opens by saying the
//! check is not running, because a project that looks protected and is
//! not is worse than one that says so.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

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
        // deployment, so each is uncommented by somebody who knows it.

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
    `src/app.rs` holds five commented registrations from\n\
    `brocanth-middleware`. Each encodes a fact about the deployment --\n\
    which proxy to trust, which host is canonical, which origins may call\n\
    -- so each is uncommented by somebody who knows that fact. Do not\n\
    enable them as a set, and do not invent an address or an origin to\n\
    make one compile.\n";

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
        s.at(Slot::AppPlugins).block(REGISTRATION.to_string());
        s.at(Slot::AgentsSections).block(CSRF_AGENTS.to_string());
    }
}

/// The registration, with the configuration already filled in.
///
/// Commented, and not for the reason the five above it are. Those are a
/// decision somebody has to make; this one is two gaps in the generator,
/// both named in the block so the person who meets them does not have to
/// find them the way this was found -- by generating a project, serving
/// it, and posting to it.
const REGISTRATION: &str = r#"
        // -- brocanth-middleware: CSRF ------------------------------------
        //
        // Not one of the five above. Each of those encodes a fact about
        // this deployment; this one encodes a fact about browsers: any
        // page on any site can post a form here, and the cookies ride
        // along. That is true wherever this runs, so this is the hook
        // that should be on, and the only thing to decide is which paths
        // are exempt -- a webhook, or anything else whose caller is not a
        // browser:
        //
        //     CsrfConfig::default().exempt(["/webhooks"])
        //
        // The token for the request is on the request state as a
        // `CsrfToken`; `token.hidden_field()` is the input a form carries.
        //
        // READ THIS BEFORE YOU UNCOMMENT: this project has no CSRF check
        // today, and uncommenting the line below does not give it one.
        // Two things in the generated code are in the way.
        //
        // 1. `pub type Hooks` below says `NoHooks` whatever registered,
        //    and `plugin_hooking` widens that record, so the line does
        //    not compile until the alias says:
        //
        //        pub type Hooks = ::brocanth::plugin_hooks::Both<
        //            <::brocanth_middleware::Csrf
        //                as ::brocanth::plugin_hooks::HookSource>::Families,
        //            ::brocanth::plugin_hooks::NoHooks,
        //        >;
        //
        // 2. `dispatch` in `src/app.rs` goes from the route table to the
        //    handler. The hook pipeline is `App::run`, which nothing in
        //    that file calls, so no hook in this crate runs -- these
        //    five included. `brocanth_server::Bridge` is the piece that
        //    runs it, and serving through it is the change.
        //
        // let builder = builder.plugin_hooking(
        //     <brocanth_middleware::Csrf as brocanth::plugin_api::Plugin>::new(
        //         brocanth_middleware::CsrfConfig::default(),
        //     ),
        // );
"#;

/// What the generated `AGENTS.md` says, which is first of all that the
/// check is not running.
///
/// An agent that read "CSRF" in `src/app.rs` and stopped there would
/// report a protected application. This says the opposite in the first
/// line.
const CSRF_AGENTS: &str = "\n\
    ## CSRF\n\
    \n\
    **This project does not check CSRF tokens yet.** `src/app.rs` holds a\n\
    commented registration for `brocanth_middleware::Csrf` and, beside\n\
    it, the two things in the generated code that are in the way: the\n\
    `plugins::Hooks` alias, and a `dispatch` that never enters the hook\n\
    pipeline. Read that block before you tell anybody this is covered.\n\
    \n\
    What it will do once it runs: it is a double-submit cookie. The hook\n\
    mints a token, sets it as its own cookie, and refuses any verb that\n\
    is not GET, HEAD, OPTIONS or TRACE unless the request sends the same\n\
    value back -- in the `_csrf_token` field, or in the `X-CSRF-Token`\n\
    header. So a form needs the hidden field: read `CsrfToken` off the\n\
    request state and write `token.hidden_field()`.\n\
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

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.at(Slot::AppPlugins)
            .block(SECURITY_HEADERS_REGISTRATION.to_string());
        s.at(Slot::AgentsSections)
            .block(SECURITY_HEADERS_AGENTS.to_string());
    }
}

/// The registration. Commented for the two reasons the CSRF block above
/// it gives, and for no reason of its own.
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
        // Commented for the two reasons the CSRF block gives: the `Hooks`
        // alias and the hook pipeline.
        //
        // let builder = builder.plugin_hooking(
        //     <brocanth_middleware::SecurityHeaders as brocanth::plugin_api::Plugin>::new(
        //         brocanth_middleware::SecurityHeadersConfig::default(),
        //     ),
        // );
"#;

/// What the generated `AGENTS.md` says about the policy.
const SECURITY_HEADERS_AGENTS: &str = "\n\
    ## Security headers\n\
    \n\
    `src/app.rs` holds a commented registration for\n\
    `brocanth_middleware::SecurityHeaders`. Its Content-Security-Policy\n\
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
        s.at(Slot::AppPlugins)
            .block(RATE_LIMIT_REGISTRATION.to_string());
        s.at(Slot::AgentsSections)
            .block(RATE_LIMIT_AGENTS.to_string());
    }
}

/// The registration, with an example rule that is marked as one.
const RATE_LIMIT_REGISTRATION: &str = r#"
        // -- brocanth-middleware: rate limit ------------------------------
        //
        // A fixed window per key. Each rule is a group of routes, a limit
        // and a window. The numbers below are an example for a login form,
        // not a recommendation: pick them from your own traffic.
        //
        // The key is the client address. Behind a proxy, register
        // `ProxyHeaders` above, or name the one header your edge sets and
        // strips: `.ip_header("fly-client-ip")`.
        //
        // This counts in this process. With more than one process, use
        // `SharedRateLimit` and `rate_limit::PgStore` (feature `postgres`).
        //
        // let builder = builder.plugin_hooking(
        //     <brocanth_middleware::RateLimit as brocanth::plugin_api::Plugin>::new(
        //         brocanth_middleware::RateLimitConfig::rules([
        //             brocanth_middleware::Rule::new("login", 5, 60)
        //                 .paths(["/login"])
        //                 .methods(["POST"]),
        //         ]),
        //     ),
        // );
"#;

/// What the generated `AGENTS.md` says, so an agent does not invent a
/// limit.
const RATE_LIMIT_AGENTS: &str = "\n\
    ## Rate limit\n\
    \n\
    `src/app.rs` holds a commented registration for\n\
    `brocanth_middleware::RateLimit`. Its numbers are an example. Do not\n\
    enable it with invented limits. Ask for the limit and the window of\n\
    each route group.\n\
    \n\
    A request whose key cannot be found is not counted. Behind a proxy,\n\
    the address comes from `ProxyHeaders` or from `ip_header`. Never set\n\
    `ip_header` to a header the client can send, such as\n\
    `X-Forwarded-For`, unless the edge overwrites it.\n";
