//! `sessions` -- a cookie, and a `Session` on the context.

use brocanth_scaffold::snippet::settings_derive;
use brocanth_scaffold::{Manifest, Module, Project, Scaffold, Slot};

/// Cookie sessions.
///
/// Written into the project like the logger, and for the same reason: it is
/// short enough to read, and a session policy is a thing applications
/// disagree about.
pub struct Sessions;

impl Module for Sessions {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "sessions",
            "a cookie, and a `Session` the context hands a handler",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/sessions.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod sessions;");
        s.at(Slot::MainBuilder).block(if project.has("config") {
            // Read from `[sessions]` in the file the config plugin loaded.
            "        .plugin_from_config::<plugins::sessions::Sessions>(\"sessions\")\n\
             \x20       .map_err(|e| e.to_string())?\n"
        } else {
            // No file to read, so `main` hands it a literal the compiler
            // checks, and no deserialiser is linked.
            "        .plugin(plugins::sessions::Sessions::new(plugins::sessions::Settings::default()))\n"
        });
        // A plugin *offers* middleware and cannot mount it. The pattern is
        // a decision, and the decision is the application's -- so the line
        // is in `main` where it can be read and changed.
        s.at(Slot::MainSetup)
            .line(r#"            setup.router().mount("/users/{*rest}", "session");"#);
        if project.has("config") {
            s.at(Slot::ConfigSections).block(format!(
                "\n[sessions]\ncookie = \"_{ident}_session\"\n\
                 # Development only. Anywhere a browser reaches over HTTPS, this is true.\n\
                 secure = false\n",
                ident = project.ident()
            ));
        }
    }
}

pub(crate) fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "sessions");
    format!(
        r##"//! Cookie sessions, and the one value they put on the context.

use brocanth::plugin_api::{{Lifecycle, MiddlewareCatalog, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth::routing::Next;

use crate::pages::{{Ctx, Response}};

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    pub cookie: String,
    /// `Secure`, so the cookie never leaves over plain HTTP. False in
    /// development and nowhere else.
    pub secure: bool,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            cookie: "_{ident}_session".to_string(),
            secure: true,
        }}
    }}
}}

/// What this plugin puts on the context.
///
/// One provider owns one type, so there is no string name to misspell and no
/// `responds_to?` guard to forget. A handler that asks for a `Session` in an
/// application that never registered this plugin does not compile.
#[derive(Debug, Default)]
pub struct Session {{
    pub cookie: String,
}}

impl ContextItem for Session {{}}

pub struct Sessions {{
    cookie: String,
    secure: bool,
}}

impl Plugin for Sessions {{
    type Config = Settings;
    type Provides = ProvidesContext<Session>;
    const NAME: &'static str = "sessions";

    fn new(config: Settings) -> Self {{
        Self {{
            cookie: config.cookie,
            secure: config.secure,
        }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.cookie.is_empty() {{
            // A missing or ill-typed key fails at startup, not at first use,
            // and the plugin never quietly chooses a name the operator did not.
            return Err(PluginError::new("[sessions] cookie must not be empty"));
        }}
        Ok(())
    }}

    fn configure_router(&mut self, catalog: &mut MiddlewareCatalog<'_>) -> Result<(), PluginError> {{
        catalog.offer(
            "session",
            "reads the cookie and puts a Session on the context",
        );
        Ok(())
    }}

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if !self.secure {{
            eprintln!("[sessions] Secure is off: development only");
        }}
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        cx.insert(Session {{
            cookie: self.cookie.clone(),
        }});
    }}
}}

/// The layer `session` mounts. Reading the cookie off the request and writing
/// `Set-Cookie` on the way out both need the request type that arrives with the
/// HTTP server; the seam is here and it is already the right shape.
pub fn layer(ctx: &mut Ctx, next: Next<'_, Ctx, Response>) -> Response {{
    next.run(ctx)
}}
"##,
        ident = project.ident()
    )
}
