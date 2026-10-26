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
