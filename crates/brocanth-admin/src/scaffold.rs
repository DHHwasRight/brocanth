//! `admin` and `admin-model` -- what `brocanth create` writes when somebody
//! asks for a backoffice.
//!
//! The module is here rather than in the CLI because what it writes is a
//! statement about this crate: that [`Admin::dispatch`](crate::Admin::dispatch)
//! takes a method, a path, a query and a [`Submission`](crate::Submission),
//! that the admin owns its own path grammar, that the default policy is
//! [`DenyAll`](crate::DenyAll), and that a model with no
//! [`AdminResource`](crate::AdminResource) answers 403. Those move when this
//! crate moves.
//!
//! # Why the routes are page-shaped and not the `RouteSource`
//!
//! `AdminPlugin` contributes the screens as a
//! `RouteSource`, and a generated project cannot mount one: its table holds
//! the project's own `Handler`, and `RouteCx` needs a context the
//! generated `Ctx` does not lend out. `brocanth-resource` measured the same
//! walls (`brocanth_resource::pages`). So the module writes a `mount` with the
//! shape `Slot::RouterMounts` expects, four routes into the one table, each
//! calling [`Admin::dispatch`](crate::Admin::dispatch). That is forty lines in
//! a file the developer owns, which is also where they say who is asking.
//!
//! # It ships locked
//!
//! The generated admin has no models, the [`DenyAll`](crate::DenyAll) policy
//! and an anonymous principal, so `/admin` answers 403. The generated test
//! asserts exactly that, which proves the routes are in the table and that
//! nothing was opened on the developer's behalf.
//!
//! # Why `admin-model` is listed and not available
//!
//! `brocanth-admin-model` reads rows over a SeaORM `DatabaseConnection`, and
//! the generated `model` plugin opens none: it publishes a URL and no pool.
//! It also takes `brocanth-model` with its default driver, which is Postgres,
//! while the generated config names a SQLite file. A module that wrote
//! `admin_source!` over that would compile and fail on the first request.
//! The reason is on its row until the model plugin opens a pool.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{configurable, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// The backoffice, as a choice on the `brocanth create` menu.
///
/// Named `Backoffice` and not `Admin`, which is this crate's own type: a
/// second item under that name makes rustc print fully qualified paths in
/// every error that mentions either. `brocanth-resource` hit that with
/// `Resource`.
pub struct Backoffice;

impl Module for Backoffice {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "admin",
            "a Django-shaped backoffice at /admin, locked until you say who may in",
        )
        .category(Category::Routes)
        .category(Category::Enhancer)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to("middleware-csrf", "every form carries the CSRF token")
        .adapts_to(
            "commands",
            "the binary gets `admin eject`, which runs `brocanth admin eject` in the project",
        )
        .asked_in("doors", "A backoffice at /admin, locked until you open it")
        .krate(
            Dep::brocanth("brocanth-admin")
                // The `RouteSource` is for a host that mounts one. A
                // generated project mounts the page-shaped routes it writes
                // itself, so the plugin half is not compiled.
                .no_default_features(),
        )
        .absent(
            "brocanth-admin",
            "no backoffice; a row is changed through a page somebody wrote, or SQL",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let title = format!("{} admin", project.title);
        let (derive, note) = settings_derive(project, "admin");
        s.file(
            "src/plugins/admin/mod.rs",
            ADMIN_RS
                .replace("@TITLE@", &format!("{title:?}"))
                .replace("@DERIVE@", &derive)
                .replace("@NOTE@", &note)
                // With CSRF on, every form the admin posts carries the
                // token, or the hook refuses the admin's own forms.
                .replace(
                    "@PAGE@",
                    if project.has("middleware-csrf") {
                        "brocanth_admin::Response::Page { status, html } => {\n\
                         \x20                   Response::new(status, stamped(ctx, html))\n\
                         \x20               }"
                    } else {
                        "brocanth_admin::Response::Page { status, html } => Response::new(status, html),"
                    },
                )
                .replace(
                    "@STAMPED@",
                    if project.has("middleware-csrf") {
                        STAMPED_FN
                    } else {
                        ""
                    },
                ),
        );
        s.at(Slot::PluginsMod).line("pub mod admin;");
        // The titles, the "View site" link and the theme are declared, so
        // each is a key in `[admin]`, a variable and a flag. A plugin holds
        // them, because the settings arrive when the application is built
        // and `mount` runs after that, when the route table is.
        configurable(s, project, "admin", "AdminSettings", settings(&title));
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        // The prefix is the application's decision, so it is written at the
        // call and not inside `mount`.
        s.at(Slot::RouterMounts)
            .line("    crate::plugins::admin::mount(routes, \"/admin\")?;");
        // Two patterns, and the table's own test lists every route.
        s.at(Slot::PagesRoutes).line("/admin");
        s.at(Slot::PagesRoutes).line("/admin/{*rest}");
        // `admin eject` belongs to the `brocanth` CLI, which writes source.
        // With a command line, the binary offers it too and hands it to the
        // CLI, so the app never links the code that writes code.
        if project.has("commands") {
            s.file("src/commands/admin.rs", ADMIN_COMMAND_RS.to_string());
            s.at(Slot::CommandsMod).line("pub mod admin;");
        }
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// `src/commands/admin.rs`: `admin eject`, handed to the `brocanth` CLI.
///
/// Root: `brocanth_command::Context::root`, the nearest
/// directory with `brocanth.toml` above the current one. That is the rule
/// the CLI applies to itself, so both agree on the project, and it holds
/// for a binary run from anywhere inside the tree. `CARGO_MANIFEST_DIR`
/// would name the directory the binary was compiled in, which is wrong for
/// a binary copied elsewhere and for a second checkout that shares a
/// target directory.
const ADMIN_COMMAND_RS: &str = r##"//! `admin eject` -- a shipped admin screen, copied into this project.
//!
//! The work is the `brocanth` CLI's. This command runs
//! `brocanth admin eject <screen> [--model <key>]` in the project root and
//! exits with its status, so `cargo run -- admin eject list` and
//! `brocanth admin eject list` do the same thing. The application does not
//! link the code that writes source.
//!
//! This file is yours. See `docs/admin-customization.md`, level 8.

use std::ffi::OsStr;
use std::path::Path;
use std::process;

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Work on the admin: `eject` copies a shipped screen into this project.
#[cmd]
#[derive(clap::Args)]
pub struct Admin {
    #[command(subcommand)]
    pub action: Action,
}

#[derive(clap::Subcommand)]
pub enum Action {
    /// Copy a shipped screen into this project and install it over the
    /// default. `brocanth admin eject --help` lists the screens.
    Eject {
        /// The screen to copy, such as `list` or `layout`.
        screen: String,
        /// Install the copy for this model only. list, detail, form and
        /// delete take one.
        #[arg(long, value_name = "KEY")]
        model: Option<String>,
    },
}

impl Command for Admin {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        let status = brocanth(OsStr::new("brocanth"), ctx.root(), &self.action.args())?;
        if !status.success() {
            // `brocanth` printed the error and its help line.
            process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }
}

impl Action {
    /// The `brocanth` arguments this action stands for.
    fn args(&self) -> Vec<String> {
        match self {
            Action::Eject { screen, model } => {
                let mut args = vec!["admin".to_string(), "eject".to_string(), screen.clone()];
                if let Some(model) = model {
                    args.extend(["--model".to_string(), model.clone()]);
                }
                args
            }
        }
    }
}

/// Runs `program` with `args` in `root`. A program that is not installed
/// is an error that says how to install it.
fn brocanth(program: &OsStr, root: &Path, args: &[String]) -> anyhow::Result<process::ExitStatus> {
    let name = program.to_string_lossy();
    process::Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => anyhow::anyhow!(
                "`{name}` is not installed or not on PATH\n\
                 help: install the brocanth CLI: `cargo install --git \
                 https://github.com/DHHwasRight/brocanth brocanth-cli`, or \
                 `cargo install --path crates/brocanth-cli` in a brocanth checkout"
            ),
            _ => anyhow::anyhow!("could not run `{name}`: {e}"),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eject_passes_the_screen_and_the_model_through() {
        let action = Action::Eject {
            screen: "list".to_string(),
            model: Some("post".to_string()),
        };
        assert_eq!(action.args(), ["admin", "eject", "list", "--model", "post"]);
        let action = Action::Eject {
            screen: "layout".to_string(),
            model: None,
        };
        assert_eq!(action.args(), ["admin", "eject", "layout"]);
    }

    #[test]
    fn a_missing_cli_says_how_to_install_it() {
        let error = brocanth(
            OsStr::new("brocanth-that-is-not-installed"),
            Path::new("."),
            &[],
        )
        .expect_err("no such program");
        let message = error.to_string();
        assert!(
            message.contains("not installed or not on PATH"),
            "{message}"
        );
        assert!(
            message.contains("help: install the brocanth CLI"),
            "{message}"
        );
    }
}
"##;

/// The admin's level-1 and level-2 customisation, as declared settings. See
/// `docs/admin-customization.md`.
fn settings(title: &str) -> Vec<Setting> {
    vec![
        Setting::text(
            "title",
            title,
            "The end of every <title>, and the header and index heading unless set.",
        ),
        Setting::text(
            "header",
            "Acme backoffice",
            "What the top of the sidebar says.",
        )
        .optional(),
        Setting::text(
            "index_title",
            "Site administration",
            "The index page's heading.",
        )
        .optional(),
        Setting::text(
            "site_url",
            "/",
            "Where the sidebar's \"View site\" link points.",
        )
        .optional(),
        Setting::text(
            "stylesheet",
            "/admin-theme.css",
            "A stylesheet every screen links after the shipped one: the theme.",
        )
        .optional(),
    ]
}

/// The admin over SeaORM, listed and not available.
pub struct BackofficeModels;

impl Module for BackofficeModels {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "admin-model",
            "the admin's rows over SeaORM, one line per entity",
        )
        .category(Category::Routes)
        .asked_in("doors", "")
        .part_of_but_off("admin")
        .because("it is the admin's data source, so there is nothing to read rows for without it")
        .needs(
            "model",
            "it reads rows through the ORM, and without `model` there is none",
        )
        .blocked("the generated model plugin opens no pool, so there is no connection to read")
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // Never reached while the CLI honours `Manifest::blocked`.
        s.refuse(
            "admin-model cannot be scaffolded: `brocanth_admin_model::admin_source!` takes a \
             SeaORM `DatabaseConnection`, and the generated `model` plugin opens none. Write \
             the source by hand over your own connection and pass it to `Admin::new` in \
             `src/plugins/admin/mod.rs`",
        );
    }
}

/// `src/plugins/admin/mod.rs`. `@TITLE@` is the quoted title.
const ADMIN_RS: &str = r##"//! The backoffice, at `/admin`.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! It ships locked, and two answers are yours to give:
//!
//! 1. **Who may open a screen.** The policy is `DenyAll` until
//!    `AdminConfig::with_policy` names another, so every screen answers 403.
//! 2. **Who is asking.** [`principal`] returns an anonymous principal until
//!    it reads your user off the request.
//!
//! It has no models yet. A model needs a data source that reads its rows and
//! a row policy: `AdminConfig::resource(..)`, one per model. A model with no
//! resource answers 403 on every screen. `brocanth_admin::rows` has the
//! worked example, and `brocanth-admin-model` is the data source over
//! SeaORM.
//!
//! The routes are in the one table `src/pages/mod.rs` builds:
//! `pages::mounted` calls [`mount`] with the prefix. A page at `/admin` is
//! therefore a build error, not a race.

use std::sync::{Arc, OnceLock};

use brocanth::plugin_api::Plugin;
use brocanth::plugin_context::NoContext;
use brocanth::routing::{BuildError, RouterBuilder, Verb};
use brocanth_admin::policy::Principal;
use brocanth_admin::{Admin, AdminConfig, Fixture, Submission};

use crate::pages::{Ctx, Handler, Response};

/// The admin's titles, its "View site" link and its theme: `[admin]` in
/// the config file, `$<APP>_ADMIN__<KEY>`, or `--admin.<key>`.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// The end of every `<title>`, and the header and the index heading
    /// when those are not set.
    pub title: String,
    /// What the top of the sidebar says.
    pub header: Option<String>,
    /// The index page's heading.
    pub index_title: Option<String>,
    /// Where the sidebar's "View site" link points. No link when unset.
    pub site_url: Option<String>,
    /// A stylesheet every screen links after the shipped one. More than one
    /// is `AdminConfig::stylesheet` again, in [`mount`].
    pub stylesheet: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            title: @TITLE@.to_string(),
            header: None,
            index_title: None,
            site_url: None,
            stylesheet: None,
        }
    }
}

/// The settings this run chose, kept for [`mount`].
///
/// `mount` runs when the route table is built, after the plugins are, and
/// takes no argument it could carry them in. A test that builds the table
/// without the application gets the defaults.
static CHOSEN: OnceLock<Settings> = OnceLock::new();

/// Holds the admin's settings. It puts nothing on the context.
pub struct AdminSettings;

impl Plugin for AdminSettings {
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "admin";

    fn new(config: Settings) -> Self {
        // The first application built in this process decides.
        let _ = CHOSEN.set(config);
        Self
    }
}

/// Every screen of the admin, under `prefix`.
///
/// Four routes: the index and everything under it, for GET and for POST.
/// The admin parses its own paths, so the table gives it the whole path and
/// does not split it into twelve routes.
pub fn mount(routes: &mut RouterBuilder<Handler>, prefix: &str) -> Result<(), BuildError> {
    let defaults = Settings::default();
    let settings = CHOSEN.get().unwrap_or(&defaults);
    let mut config = AdminConfig::new().titled(&settings.title).at(prefix);
    if let Some(header) = &settings.header {
        config = config.headed(header);
    }
    if let Some(heading) = &settings.index_title {
        config = config.index_titled(heading);
    }
    if let Some(href) = &settings.site_url {
        config = config.site_url(href);
    }
    if let Some(href) = &settings.stylesheet {
        config = config.stylesheet(href);
    }
    let admin = Arc::new(Admin::new(
        // An empty in-memory source: no models, so no rows to read. Put
        // your own `AdminData` here, or `brocanth_admin_model::admin_source!`
        // over your database connection.
        Arc::new(Fixture::new()),
        config,
    ));
    let rest = format!("{}/{{*rest}}", prefix.trim_end_matches('/'));
    for pattern in [prefix, rest.as_str()] {
        routes.route(pattern, Verb::Get, screen(Arc::clone(&admin)))?;
        routes.route(pattern, Verb::Post, screen(Arc::clone(&admin)))?;
    }
    Ok(())
}

/// One handler for every screen. The admin reads the path and decides.
///
/// The admin is async because a database is, and so is a handler in this
/// table: the answer is awaited and holds no thread while it waits.
fn screen(admin: Arc<Admin>) -> Handler {
    Box::new(move |ctx: &mut Ctx| {
        let admin = Arc::clone(&admin);
        Box::pin(async move {
            let ctx = &*ctx;
            // HEAD reaches the GET handler, and the admin knows two verbs.
            let method = match ctx.verb {
                Verb::Post => "POST",
                _ => "GET",
            };
            let form = submission(ctx);
            let who = principal(ctx);
            // `?` on the admin's own failure: a data source that failed, or
            // a template that did not render. A refusal is not one: it is a
            // page.
            let answer = admin
                .dispatch(who, method, &ctx.path, &ctx.query, &form)
                .await?;
            Ok(match answer {
                @PAGE@
                brocanth_admin::Response::Redirect { status, location } => {
                    Response::empty(status).with_header("location", location)
                }
            })
        })
    })
}

@STAMPED@/// Who is asking, as the admin's policy sees it.
///
/// Anonymous until this reads your user off the request. When a plugin
/// publishes one, this is one line:
///
/// ```ignore
/// ctx.provided::<CurrentUser, _>()
///     .map_or_else(Principal::anonymous, |user| Principal::new(user))
/// ```
fn principal(_ctx: &Ctx) -> Principal<'_> {
    Principal::anonymous()
}

/// The form a POST sent. Anything that is not urlencoded is an empty form,
/// so the screen shows its errors and writes nothing.
fn submission(ctx: &Ctx) -> Submission {
    let urlencoded = ctx
        .content_type()
        .is_some_and(|t| t.starts_with("application/x-www-form-urlencoded"));
    match ctx.body_text() {
        Some(body) if urlencoded => Submission::from_urlencoded(body),
        _ => Submission::new(),
    }
}

#[cfg(test)]
mod tests {
    use brocanth::routing::{Dispatch, Verb};

    use crate::pages::Ctx;

    /// The admin is in the table, and it answers 403 until somebody opens it.
    #[tokio::test]
    async fn the_admin_is_mounted_and_ships_locked() {
        let router = crate::pages::router().expect("the table builds");
        let Dispatch::Found { handler, matched } = router.dispatch(Verb::Get, "/admin") else {
            panic!("/admin is not in the table");
        };
        let mut ctx = Ctx::new(Verb::Get, "/admin", matched.params());
        let answer = handler(&mut ctx).await.expect("the admin answers");
        assert_eq!(answer.status, 403);
    }
}
"##;

/// The `AGENTS.md` section.
const AGENTS: &str = "\n\
    ## The admin\n\
    \n\
    `src/plugins/admin/mod.rs` mounts the backoffice at `/admin`. It ships\n\
    locked: the policy is `DenyAll` and `principal` returns an anonymous\n\
    principal, so every screen answers 403. Do not open it with\n\
    `AllowAllUnsafeForDevelopment` outside a test.\n\
    \n\
    A model reaches the admin through `AdminConfig::resource(..)`: a\n\
    `brocanth_admin::AdminResource` with a `brocanth_policy::Policy` over\n\
    the model's rows. A model with no resource answers 403. Never filter\n\
    rows after a read; the row policy goes into the query.\n";

/// The admin's page with the CSRF field in every form that posts.
/// Written only when `middleware-csrf` is on.
const STAMPED_FN: &str = r#"/// The page with the CSRF token in every form that posts.
///
/// The admin renders its own forms and does not know the token, so the
/// field goes in here. Without a token on the request -- a test that built
/// the context by hand -- the page goes out unchanged.
fn stamped(ctx: &Ctx, html: String) -> String {
    match ctx.hooked::<brocanth_middleware::CsrfToken>() {
        Some(token) => token.stamp_forms(&html),
        None => html,
    }
}

"#;
