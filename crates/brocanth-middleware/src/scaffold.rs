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
//! the rate limit's. Each goes through [`snippet::configurable_hooking`],
//! which also writes the plugin into the generated `plugins::Hooks` record,
//! and the generated `src/app.rs` runs every request through
//! `App::begin_async` and `App::finish`. So a hook this module registers
//! runs.
//!
//! # Settings
//!
//! The three declare their settings, so each gets a section in
//! `config/<env>.toml` when the project has `config`, and a flag and an
//! environment variable either way: `--csrf.secure`, `--security_headers.
//! behind_tls`, `--rate_limit.ip_header`. The section is the plugin's
//! instance name, which is what `brocanth-config` checks a section
//! against. The configuration types are this crate's own, so the generated
//! readers start from their `Default`; the `config` feature derives
//! `Deserialize` on them only when the project has the config file.
//!
//! A list has no flag. CSRF's `exempt` and the rate limit's rules are
//! file-only: a rule is a record of six fields, and `[[rate_limit.rules]]`
//! is how TOML writes a list of records.
//!
//! [`snippet::configurable_hooking`]: brocanth_scaffold::snippet::configurable_hooking

use brocanth_scaffold::snippet::{configurable_hooking, Hooking};
use brocanth_scaffold::{Category, Dep, Hook, Manifest, Module, Project, Scaffold, Setting, Slot};

/// The edge hooks, as a choice on the `brocanth create` menu.
pub struct Middleware;

impl Module for Middleware {
    fn manifest(&self) -> Manifest {
        // No `hook`: the five plugins are commented lines that a person
        // copies to `src/app.rs`, not a registration this module writes.
        // A toggle in src/modules/middleware.rs would reach none of them,
        // and five instances do not fit one `hooks` field.
        Manifest::new(
            "middleware",
            "proxy headers, method override, canonical host, content types, CORS",
        )
        .category(Category::Middleware)
        .asked_in("doors", "The edge hooks: proxies, CORS, a canonical host")
        .krate(Dep::brocanth("brocanth-middleware").feature_with("config", "config"))
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
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in(
            "doors",
            "Refuse a form post this application did not hand out",
        )
        .part_of("middleware")
        .because("the hook is `brocanth_middleware::Csrf`, so the crate arrives with that choice")
        .after("middleware")
        .hook(
            "check",
            "refuse an unsafe request that does not send the token back",
            Hook::required(),
        )
        .hook(
            "mint",
            "set the token cookie on a response",
            Hook::required(),
        )
        .absent(
            "csrf",
            "no CSRF check; any page on any site can post a form here and the browser \
             sends the cookies with it",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let exempt = if project.has("config") {
            "        //     exempt = [\"/webhooks\"]    under [csrf] in config/<env>.toml\n"
        } else {
            "        //     a list has no flag: `brocanth add config` gives `exempt`\n\
             \x20       //     a home under [csrf] in config/<env>.toml\n"
        };
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "middleware-csrf",
                section: "csrf",
                ty: "brocanth_middleware::Csrf",
                config_ty: "brocanth_middleware::CsrfConfig",
                prelude: CSRF_PRELUDE.replace("@EXEMPT@\n", exempt),
                adjust: None,
            },
            vec![
                Setting::text("cookie", "csrf_token", "The cookie this hook owns."),
                Setting::text(
                    "field",
                    "_csrf_token",
                    "The form field the token comes back in.",
                ),
                Setting::text(
                    "header",
                    "x-csrf-token",
                    "The header a fetch caller may send instead of the field.",
                )
                .optional()
                .note("While it is set, the cookie is not HttpOnly: a script reads it."),
                Setting::list(
                    "exempt",
                    &[],
                    "Paths never checked, each with everything under it.",
                )
                .note("For a caller that is not a browser: a webhook, a device."),
                Setting::bool("secure", true, "Send the cookie over HTTPS only."),
            ],
        );
        s.at(Slot::AgentsSections).block(CSRF_AGENTS.to_string());
    }
}

/// The comment above the registration. `@EXEMPT@` is where an exempt path
/// goes, which depends on whether there is a file.
const CSRF_PRELUDE: &str = r#"
        // -- brocanth-middleware: CSRF ------------------------------------
        //
        // Not one of the five above. Each of those encodes a fact about
        // this deployment; this one encodes a fact about browsers: any
        // page on any site can post a form here, and the cookies ride
        // along. That is true wherever this runs, so this hook is on, and
        // the only thing to decide is which paths are exempt -- a webhook,
        // or anything else whose caller is not a browser:
        //
@EXEMPT@
        //
        // The token for the request is `ctx.hooked::<CsrfToken>()`;
        // `token.hidden_field()` is the input a form carries.
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
        .category(Category::Middleware)
        .adapts_to("admin", "the CSP allows the admin's inline script by hash")
        .adapts_to(
            "gtag",
            "the CSP allows the consent script by hash and Google's origins",
        )
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in("doors", "Send the headers a browser reads as policy")
        .part_of("middleware")
        .because(
            "the hook is `brocanth_middleware::SecurityHeaders`, so the crate arrives with that choice",
        )
        .after("middleware-csrf")
        .hook(
            "set",
            "send the policy headers on each response",
            Hook::required(),
        )
        .absent(
            "security-headers",
            "no security headers; a browser sniffs types, frames the pages and runs any inline script",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        // The admin layout inlines dp-swap's vendored copy, which the
        // default policy does not know. Its hash goes in on top of what the
        // file says, or the policy blocks the admin's script.
        //
        // The gtag consent script likewise, and the origins Google's
        // tag loads from once a visitor accepts.
        let mut adjust = String::new();
        if project.has("admin") {
            adjust.push_str(".inline_script(brocanth_admin::INLINE_SCRIPT)");
        }
        if project.has("gtag") {
            adjust.push_str(".page_script(brocanth_gtag::SCRIPT, brocanth_gtag::CSP)");
        }
        let adjust = (!adjust.is_empty()).then_some(adjust);
        let behind_tls = if project.has("config") {
            "`behind_tls = true` under\n        // [security_headers] in config/<env>.toml"
        } else {
            "`--security_headers.behind_tls=true`,\n        // or the environment variable `--help` names"
        };
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "middleware-security-headers",
                section: "security_headers",
                ty: "brocanth_middleware::SecurityHeaders",
                config_ty: "brocanth_middleware::SecurityHeadersConfig",
                prelude: SECURITY_HEADERS_PRELUDE.replace("@BEHIND_TLS@", behind_tls),
                adjust,
            },
            vec![
                Setting::bool(
                    "behind_tls",
                    false,
                    "Every request arrives through a TLS terminator, so HSTS always goes out.",
                )
                .note("False in development: HSTS from localhost pins the name in the browser."),
                Setting::bool("nosniff", true, "Send `X-Content-Type-Options: nosniff`."),
                Setting::text(
                    "referrer_policy",
                    "strict-origin-when-cross-origin",
                    "The `Referrer-Policy`.",
                )
                .optional(),
                Setting::text(
                    "permissions_policy",
                    "camera=(), microphone=(), geolocation=(), payment=(), usb=()",
                    "The `Permissions-Policy`.",
                )
                .optional(),
                Setting::bool("hsts.enabled", true, "Send `Strict-Transport-Security`."),
                Setting::integer(
                    "hsts.max_age",
                    31_536_000,
                    "How long the browser remembers HTTPS-only, in seconds.",
                ),
                Setting::bool(
                    "hsts.include_subdomains",
                    false,
                    "The HSTS rule covers every subdomain too.",
                )
                .note("Only when the person who owns the domain says so."),
                Setting::bool(
                    "hsts.preload",
                    false,
                    "Ask to be on the browsers' built-in HSTS list.",
                )
                .note("Hard to undo: removal takes months to reach users."),
                Setting::bool("csp.enabled", true, "Send a `Content-Security-Policy`."),
                Setting::bool(
                    "csp.report_only",
                    false,
                    "Send the policy as `Content-Security-Policy-Report-Only`.",
                ),
                Setting::text(
                    "csp.report_uri",
                    "/csp-reports",
                    "Where a browser reports what the policy blocked.",
                )
                .optional(),
            ],
        );
        s.at(Slot::AgentsSections)
            .block(SECURITY_HEADERS_AGENTS.to_string());
    }
}

/// The comment above the registration. `@BEHIND_TLS@` says where to set
/// `behind_tls`, which depends on whether there is a file.
const SECURITY_HEADERS_PRELUDE: &str = r#"
        // -- brocanth-middleware: security headers ------------------------
        //
        // HSTS (over HTTPS only), nosniff, Referrer-Policy, X-Frame-Options
        // DENY, a Content-Security-Policy and a Permissions-Policy. The
        // policy allows dp-swap's two inline scripts by their SHA-256, so
        // there is no `'unsafe-inline'`. A script you inline yourself needs
        // `.inline_script(TEXT)` on the configuration.
        //
        // HSTS goes out only when the request is known to be HTTPS. Behind
        // a TLS terminator, say so: @BEHIND_TLS@.
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
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in("doors", "Limit how often one caller may hit a route")
        .part_of_but_off("middleware")
        .because("the hook is `brocanth_middleware::RateLimit`, so the crate arrives with that choice")
        .after("middleware-security-headers")
        .hook(
            "count",
            "count each request against its rule, and answer 429 past the limit",
            Hook::required(),
        )
        .hook(
            "headers",
            "send the RateLimit headers on each response a rule covers",
            Hook::on(),
        )
        .absent(
            "rate-limit",
            "no rate limit; a password guesser gets as many tries as the server answers",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        // The rules are records, so they live in the file when there is
        // one. Without it they are code: a list of records has no flag.
        let (prelude, adjust) = if project.has("config") {
            (RATE_LIMIT_PRELUDE.replace("@RULES@", RULES_IN_FILE), None)
        } else {
            (
                RATE_LIMIT_PRELUDE.replace("@RULES@", RULES_IN_CODE) + LOGIN_RULE,
                Some(".rule(login)".to_string()),
            )
        };
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "middleware-rate-limit",
                section: "rate_limit",
                ty: "brocanth_middleware::RateLimit",
                config_ty: "brocanth_middleware::RateLimitConfig",
                prelude,
                adjust,
            },
            vec![
                Setting::text(
                    "ip_header",
                    "fly-client-ip",
                    "A header that holds the client address, set by your edge.",
                )
                .optional()
                .note(
                    "Only a header the edge overwrites. Never `X-Forwarded-For` as the client sent it.",
                ),
                Setting::text(
                    "session_cookie",
                    &format!("_{}_session", project.ident()),
                    "The session cookie, for a rule keyed by session.",
                )
                .optional(),
                Setting::bool(
                    "fail_open",
                    true,
                    "Let a request through when the counter store fails.",
                ),
            ],
        );
        if project.has("config") {
            s.at(Slot::ConfigSections).block(RULES_TOML.to_string());
        }
        s.at(Slot::AgentsSections)
            .block(RATE_LIMIT_AGENTS.to_string());
    }
}

/// The comment above the registration. `@RULES@` says where the rules are.
const RATE_LIMIT_PRELUDE: &str = r#"
        // -- brocanth-middleware: rate limit ------------------------------
        //
        // A fixed window per key. Each rule is a group of routes, a limit
        // and a window. THE EXAMPLE RULE IS FOR A LOGIN FORM, not a
        // recommendation: replace it with numbers from your own traffic.
        // @RULES@
        //
        // The key is the client address. Behind a proxy, register
        // `ProxyHeaders` above, or name the one header your edge sets and
        // strips in `ip_header`.
        //
        // This counts in this process. With more than one process, use
        // `SharedRateLimit` and `rate_limit::PgStore` (feature `postgres`).
"#;

/// Where the rules are, with a file.
const RULES_IN_FILE: &str = "The rules are `[[rate_limit.rules]]` in config/<env>.toml.";

/// Where the rules are, without one.
const RULES_IN_CODE: &str = "The rule is below: `brocanth add config` moves rules to a file.";

/// The example rule, as code.
const LOGIN_RULE: &str = r#"        let login = brocanth_middleware::Rule::new("login", 5, 60)
            .paths(["/login"])
            .methods(["POST"]);
"#;

/// The example rule, as the file holds it. An array of tables: a rule is a
/// record of six fields, and this is how TOML writes a list of records.
/// `key` and `paths` default to the client address and every path.
const RULES_TOML: &str = r#"
# THE RULE BELOW IS AN EXAMPLE for a login form, not a recommendation.
# One table per group of routes; a request counts against each that matches.
# `key` is a list of "ip", "session" and "user", first found wins.
[[rate_limit.rules]]
name = "login"
paths = ["/login"]
methods = ["POST"]
limit = 5
window_secs = 60
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

/// Conditional GET, as a choice on the menu that arrives unticked.
///
/// Off, like every child but CSRF and the headers: it hashes every page,
/// and a project that puts a CDN in front may have the CDN do it.
pub struct ConditionalGet;

impl Module for ConditionalGet {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "middleware-conditional-get",
            "a weak ETag over every page, and 304 for If-None-Match or If-Modified-Since",
        )
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in("doors", "Answer 304 when the browser already has the page")
        .part_of_but_off("middleware")
        .because(
            "the hook is `brocanth_middleware::ConditionalGet`, so the crate arrives with that choice",
        )
        .after("middleware-rate-limit")
        // The module is this one hook. A switch in the module file would
        // only repeat `brocanth remove middleware-conditional-get`, and
        // `etag` and `last_modified` already turn each half off.
        .hook(
            "check",
            "tag each 200 to a GET with a weak ETag, and answer 304 when it matches",
            Hook::required(),
        )
        .absent(
            "conditional-get",
            "no ETag and no 304; a browser downloads an unchanged page again in full",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "middleware-conditional-get",
                section: "conditional_get",
                ty: "brocanth_middleware::ConditionalGet",
                config_ty: "brocanth_middleware::ConditionalGetConfig",
                prelude: CONDITIONAL_GET_PRELUDE.to_string(),
                adjust: None,
            },
            vec![
                Setting::bool(
                    "etag",
                    true,
                    "Set a weak ETag over the body when the page set none.",
                ),
                Setting::bool(
                    "last_modified",
                    true,
                    "Answer If-Modified-Since from a Last-Modified the page set.",
                ),
            ],
        );
        s.at(Slot::AgentsSections)
            .block(CONDITIONAL_GET_AGENTS.to_string());
    }
}

/// The comment above the registration.
const CONDITIONAL_GET_PRELUDE: &str = r#"
        // -- brocanth-middleware: conditional GET -------------------------
        //
        // A weak ETag over the body of every GET that answers 200, and a
        // 304 with no body when the browser sends the same tag back. A
        // page that sets `Last-Modified` also answers If-Modified-Since.
        // The page still renders: this saves the transfer, not the work.
"#;

/// What the generated `AGENTS.md` says about tags.
const CONDITIONAL_GET_AGENTS: &str = "\n\
    ## Conditional GET\n\
    \n\
    `brocanth_middleware::ConditionalGet` is registered in\n\
    `src/plugins/mod.rs`. It tags every `200` answer to a GET with a weak\n\
    ETag over the body and answers `304` when the browser already has it.\n\
    So a page must render the same bytes for the same content: no\n\
    timestamp or random value in the markup, or every tag differs and no\n\
    request is ever a `304`.\n\
    \n\
    A page that knows when its data changed sets `Last-Modified` as an\n\
    IMF-fixdate (`Sun, 06 Nov 1994 08:49:37 GMT`). A page whose answer is\n\
    for one user only sets `Cache-Control: private`; one that must not be\n\
    kept sets `no-store`, and gets no tag.\n";

/// Compression, as a choice on the menu that arrives unticked.
///
/// Off, because most deployments put a proxy or a CDN in front that
/// compresses already, and the `compression` feature links brotli.
pub struct Compression;

impl Module for Compression {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "middleware-compression",
            "gzip and brotli per Accept-Encoding on text responses, with Vary; never a stream",
        )
        .category(Category::Middleware)
        .krate(
            Dep::brocanth("brocanth-middleware")
                .feature_with("config", "config")
                .feature("compression"),
        )
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in("doors", "Compress pages for the browsers that ask")
        .part_of_but_off("middleware")
        .because(
            "the hook is `brocanth_middleware::Compression`, so the crate arrives with that choice",
        )
        .after("middleware-conditional-get")
        // The module is this one hook, so it has no switch of its own: a
        // project behind a CDN that compresses removes the module, and one
        // that wants only gzip sets `brotli = false`.
        .hook(
            "encode",
            "compress a buffered text body with gzip or brotli, as Accept-Encoding asks",
            Hook::required(),
        )
        .absent(
            "compression",
            "no compression; an HTML page goes over the wire five to ten times its gzip size",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let types = if project.has("config") {
            "the list is `types` under [compression] in config/<env>.toml"
        } else {
            "a list has no flag: `brocanth add config` gives `types` a home"
        };
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "middleware-compression",
                section: "compression",
                ty: "brocanth_middleware::Compression",
                config_ty: "brocanth_middleware::CompressionConfig",
                prelude: COMPRESSION_PRELUDE.replace("@TYPES@", types),
                adjust: None,
            },
            vec![
                Setting::integer(
                    "min_bytes",
                    1024,
                    "The smallest body that is compressed, in bytes.",
                ),
                Setting::bool("gzip", true, "Offer gzip."),
                Setting::bool("brotli", true, "Offer brotli (`br`)."),
                Setting::integer("gzip_level", 6, "The gzip level, 1 (fast) to 9 (small)."),
                Setting::integer(
                    "brotli_quality",
                    4,
                    "The brotli quality, 0 (fast) to 11 (small).",
                )
                .note("4 or 5 per request; 11 is for files compressed once at build time."),
                Setting::list(
                    "types",
                    &[
                        "text/*",
                        "application/json",
                        "application/javascript",
                        "application/xml",
                        "application/rss+xml",
                        "application/atom+xml",
                        "application/manifest+json",
                        "image/svg+xml",
                    ],
                    "The media types that are compressed. `text/*` takes every text type.",
                ),
            ],
        );
        s.at(Slot::AgentsSections)
            .block(COMPRESSION_AGENTS.to_string());
    }
}

/// The comment above the registration. `@TYPES@` says where the list is.
const COMPRESSION_PRELUDE: &str = r#"
        // -- brocanth-middleware: compression -----------------------------
        //
        // gzip or brotli, as the browser's Accept-Encoding asks, on a
        // buffered text body of `min_bytes` or more. Never on a stream:
        // an SSE event must not wait for a compressor's window. The media
        // types are an allow-list; @TYPES@.
        //
        // Behind a proxy or a CDN that compresses, turn this off: the
        // work is done twice and the second pass finds nothing to gain.
"#;

/// What the generated `AGENTS.md` says about encodings.
const COMPRESSION_AGENTS: &str = "\n\
    ## Compression\n\
    \n\
    `brocanth_middleware::Compression` is registered in\n\
    `src/plugins/mod.rs`. Its response hook runs after every hook that\n\
    reads or rewrites the body, so a page never compresses its own\n\
    output. Do not set `Content-Encoding` in a page unless the body is\n\
    already encoded; the hook then leaves it alone.\n\
    \n\
    A stream is never compressed. To keep a buffered body as it is, set\n\
    `Cache-Control: no-transform`. A hook of your own that rewrites the\n\
    body goes in `runs_after`, so compression waits for it.\n";
