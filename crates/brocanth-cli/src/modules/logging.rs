//! `logging` -- a line per request.

use brocanth_scaffold::snippet::{configurable_hooking, settings_derive, Hooking};
use brocanth_scaffold::{Category, Hook, Manifest, Module, Project, Scaffold, Setting, Slot};

/// A request logger, written out into the project rather than linked.
///
/// It has no crate of its own on purpose: the whole of it is a plugin with
/// one request hook, and a reader who can see it can change it. The core
/// assumes no log layer at all, which is the thing this choice makes visible.
///
/// It logs every response, from `after_dispatch`: a 404, a 405, a CORS
/// preflight and a 401 that a request hook answered included. It was a
/// layer mounted at `/{*rest}`, and a layer runs inside dispatch, so it saw
/// only the routes that matched. Static files never reach `handle`, so they
/// are not logged.
pub struct Logging;

impl Module for Logging {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "logging",
            "a plain-text line per response on stderr, for development; for production use observability-logs",
        )
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .conflicts(
            "observability-logs",
            "both write a line per request; `observability-logs` writes JSON on stdout",
        )
        .asked_in(
            "foundations",
            "A text line per response, for development",
        )
        .hook(
            "line",
            "write one line per response on stderr",
            Hook::required(),
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/logging/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod logging;");
        // A request hook and not a mounted layer. A layer runs inside
        // dispatch, for a route that matched, so it never saw a 404, a 405,
        // a preflight or a 401 a request hook answered. `after_dispatch`
        // runs for every one of them.
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "logging",
                section: "logging",
                ty: "crate::plugins::logging::Logging",
                config_ty: "crate::plugins::logging::Settings",
                prelude: String::new(),
                adjust: None,
            },
            // `debug` in the development file; `info` without one.
            vec![Setting::text(
                "level",
                "debug",
                "The least severe line written: error, warn, info or debug.",
            )],
        );
    }
}

pub(crate) fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "logging");
    format!(
        r##"//! Request logging.
//!
//! The core assumes no log layer at all -- not `log`, not `tracing`, not a
//! trait with a no-op default. This file is the whole of logging in this
//! binary: delete the registration in `src/plugins/mod.rs` and nothing logs,
//! delete this file and nothing about logging is compiled.
//!
//! One line per response, written by a request hook's `after_dispatch`. That
//! runs for every request `handle` sees: a page, a 404, a 405, a CORS
//! preflight, and a 401 or a 429 that another request hook answered. A
//! middleware layer would see only the requests that matched a route.

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{{
    HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState, Response,
}};
use brocanth::plugin_order::Order;

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// `error`, `warn`, `info` or `debug`.
    pub level: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            level: "info".to_string(),
        }}
    }}
}}

pub struct Logging {{
    level: String,
}}

impl Plugin for Logging {{
    type Config = Settings;
    /// Nothing on the context: a logger writes, it does not hand values out.
    type Provides = NoContext;
    const NAME: &'static str = "logging";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Settings) -> Self {{
        Self {{
            level: config.level,
        }}
    }}

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        let (instance, level) = (lc.instance(), &self.level);
        crate::plugins::say!(info, "[{{instance}}] level {{level}}"; instance, %level);
        Ok(())
    }}
}}

impl HookSource for Logging {{
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {{
        registrar.request("line", Order::new(), Line);
        Ok(())
    }}
}}

/// The hook that writes the line.
struct Line;

impl RequestHook for Line {{
    /// After the response is final, whatever answered: the response hooks
    /// have run, so the status is the one the client gets.
    ///
    /// A page can add to the line: put a value on the request state with
    /// `ctx.hook_insert`, and read it here from `state`.
    fn after_dispatch(&self, request: &Request, response: &Response, _state: &mut RequestState) {{
        eprintln!(
            "{{}} {{}} -> {{}}",
            request.method(),
            request.uri().path(),
            response.status().as_u16()
        );
    }}
}}
"##
    )
}
