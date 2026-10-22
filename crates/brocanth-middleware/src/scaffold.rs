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
