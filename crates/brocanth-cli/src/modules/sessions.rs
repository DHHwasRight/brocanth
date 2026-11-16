//! `sessions` -- a cookie name on the context, and an empty `session` layer.
//!
//! A seam, not a session: the layer passes every request through, and
//! nothing reads or writes the cookie. Signed sessions, sign-in and
//! sign-out are `auth` (`brocanth-auth`), which signs its own cookie. The
//! manifest and the generated file say so, so nobody picks this for
//! accounts.

use brocanth_scaffold::snippet::{configurable, settings_derive};
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Manifest, Module, Project, Scaffold, SessionCookie,
    Setting, Slot,
};

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
            "a cookie name on the context and an empty `session` layer to build on; no session store, for sign-in pick `auth`",
        )
        .category(Category::Middleware)
        .asked_in(
            "doors",
            "A session seam to build on (no store; signed sessions are `auth`)",
        )
        .provides::<SessionCookie>("a cookie name and an empty layer; nothing signs or reads it")
        .contributes::<AgentsSection>(Contribution::new("sessions", "the seam is not a session; accounts are `auth`"), true)
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/sessions/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod sessions;");
        // Declared once: the `[sessions]` section with the config module,
        // and `--sessions.cookie` and `--sessions.secure` on the command
        // line. The values are the file's, so `secure` is false here and
        // true in `Settings::default`, which is what runs without a file.
        configurable(
            s,
            project,
            "sessions",
            "Sessions",
            vec![
                Setting::text(
                    "cookie",
                    &format!("_{}_session", project.ident()),
                    "The session cookie's name.",
                ),
                Setting::bool("secure", false, "Send the cookie over HTTPS only.")
                    .note("Development only. Anywhere a browser reaches over HTTPS, this is true."),
            ],
        );
        // A plugin *offers* middleware and cannot mount it. The pattern is
        // a decision, and the decision is the application's -- so the line
        // is in `main` where it can be read and changed.
        s.at(Slot::AppSetup)
            .line(r#"            setup.router().mount("/users/{*rest}", "session");"#);
        s.at(Slot::PluginsLayers)
            .line(r#"        "session" => Some(sessions::layer),"#);
        s.at(Slot::AgentsSections).block(AGENTS);
        s.at(Slot::AppContext).block(
            "    let session: &crate::plugins::sessions::Session = cx.extract();\n\
            \x20   let cookie = &session.cookie;\n\
            \x20   crate::plugins::say!(info, \"session cookie: {cookie}\"; %cookie);"
                .to_string(),
        );
    }
}

/// What the generated `AGENTS.md` says, so an agent does not take the seam
/// for a session.
const AGENTS: &str = "\n\
    ## Sessions seam\n\
    \n\
    `src/plugins/sessions/mod.rs` is a seam, not a session. Its `layer`\n\
    passes every request through; nothing reads, signs or writes the\n\
    cookie, and `Session` holds only the cookie's name. For accounts,\n\
    sign-in and a signed session cookie, use the `auth` module\n\
    (`brocanth-auth`): `auth.sessions()` on the context. Do not build a\n\
    second session beside it.\n";

pub(crate) fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "sessions");
    format!(
        r##"//! A sessions seam: a cookie name on the context and an empty layer.
//!
//! **This is not a session.** The `session` layer passes every request
//! through, and nothing reads, signs or writes the cookie. Signed sessions
//! with sign-in and sign-out are the `auth` module (`brocanth-auth`):
//! `brocanth add auth`. Build here only a session that is not an account.

use brocanth::plugin_api::{{Lifecycle, MiddlewareCatalog, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth::plugin_routes::BoxFuture;
use brocanth::routing::AsyncNext;

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
            return Err(PluginError::new(
                "[sessions] cookie must not be empty\n\
                 help: set `cookie` under [sessions] in the config file, such as \"_session\"",
            ));
        }}
        Ok(())
    }}

    fn configure_router(&mut self, catalog: &mut MiddlewareCatalog<'_>) -> Result<(), PluginError> {{
        catalog.offer(
            "session",
            "placeholder: passes the request through; reads and signs no cookie yet",
        );
        Ok(())
    }}

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if !self.secure {{
            crate::plugins::say!(warn, "[sessions] Secure is off: development only");
        }}
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        cx.insert(Session {{
            cookie: self.cookie.clone(),
        }});
    }}
}}

/// The layer `session` mounts. It does nothing yet: it passes the request
/// through. Reading the cookie off the request, checking its signature and
/// writing `Set-Cookie` on the way out go here; the seam is the right shape.
///
/// A layer may wait, which is what a session store wants: the lookup is
/// awaited here and holds no thread.
pub fn layer<'a>(ctx: &'a mut Ctx, next: AsyncNext<'a, Ctx, Response>) -> BoxFuture<'a, Response> {{
    next.run(ctx)
}}
"##,
        ident = project.ident()
    )
}
