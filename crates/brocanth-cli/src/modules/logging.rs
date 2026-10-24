//! `logging` -- a line per request.

use brocanth_scaffold::snippet::{plugin, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Manifest, Module, Project, Scaffold, Slot};

/// A request logger, written out into the project rather than linked.
///
/// It has no crate of its own on purpose: the whole of it is a plugin with
/// two hooks, and a reader who can see it can change it. The core assumes no
/// log layer at all, which is the thing this choice makes visible.
pub struct Logging;

impl Module for Logging {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "logging",
            "a line per request; the core assumes no log layer",
        )
        .asked_in("foundations", "A line per request")
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/logging/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod logging;");
        plugin(s, project, "logging", "Logging");
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        // A plugin *offers* middleware and cannot mount it. The pattern is
        // a decision, and the decision is the application's -- so the line
        // is in `main` where it can be read and changed.
        s.at(Slot::AppSetup)
            .line(r#"            setup.router().mount("/{*rest}", "request_log");"#);
        // What `src/app.rs` reaches for once that mount is read back. The
        // mount is the application's decision; which function answers to
        // the name is not, so it is written here and not there.
        s.at(Slot::PluginsLayers)
            .line(r#"        "request_log" => Some(logging::layer),"#);
        if project.has("config") {
            s.at(Slot::ConfigSections)
                .block("\n[logging]\nlevel = \"debug\"\n".to_string());
        }
    }
}

pub(crate) fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "logging");
    format!(
        r##"//! Request logging.
//!
//! The core assumes no log layer at all -- not `log`, not `tracing`, not a
//! trait with a no-op default. This file is the whole of logging in this
//! binary: delete the registration in `main` and nothing logs, delete this file
//! and nothing about logging is compiled.

use brocanth::plugin_api::{{Lifecycle, MiddlewareCatalog, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::routing::Next;

use crate::pages::{{Ctx, Response}};

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

    fn new(config: Settings) -> Self {{
        Self {{
            level: config.level,
        }}
    }}

    fn configure_router(&mut self, catalog: &mut MiddlewareCatalog<'_>) -> Result<(), PluginError> {{
        // Offered, not mounted. Where it runs is the application's call, made
        // in `main`, at a pattern this file never sees.
        catalog.offer("request_log", "one line per request: verb, path, status");
        Ok(())
    }}

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        eprintln!("[{{}}] level {{}}", lc.instance(), self.level);
        Ok(())
    }}
}}

/// The layer `request_log` mounts.
///
/// A plain function is a layer: `Layer` is implemented for anything callable
/// with a context and the rest of the chain. Not calling `next.run` is how a
/// layer short-circuits, and dropping it is all that takes.
pub fn layer(ctx: &mut Ctx, next: Next<'_, Ctx, Response>) -> Response {{
    let verb = ctx.verb;
    let path = ctx.path.clone();
    let response = next.run(ctx);
    eprintln!("{{verb}} {{path}} -> {{}}", response.status);
    response
}}
"##
    )
}
