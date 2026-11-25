//! `backoffice` -- what `brocanth create` writes when somebody asks for
//! the admin as a service of its own (docs/backoffice.md §7).
//!
//! Four things, and each is one line or one file:
//!
//! * `src/bin/backoffice.rs`, a second binary that runs the application
//!   itself and serves only the admin's mount. It compiles the
//!   application's own modules (`Scaffold::app_binary`), so it links the
//!   same models, policies and plugins: one code path, two binaries.
//! * `src/plugins/backoffice/mod.rs`: the `[backoffice]` settings (`addr`,
//!   `embedded`, and the wire's: `wire`, `data_url`, `service_secret`,
//!   `tx_timeout_secs`, `tx_max`), a plugin that checks them at start, the
//!   server wrap that keeps each binary to its own paths and serves the
//!   wire when `wire` is on, and `contract`, which puts the admin's data
//!   source on the tab data contract: in this process, or over the wire
//!   in the backoffice binary when `data_url` is set
//!   (docs/backoffice.md §7.5).
//! * One line in `plugins::admin_panels`, a generated file:
//!   `config.data_through(crate::plugins::backoffice::contract)`. So an
//!   admin file written before this module needs no edit.
//! * A `mise` task, `backoffice`, and a section in `AGENTS.md`.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{configurable, redact_secrets, settings_derive};
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Manifest, Module, Project, Scaffold, Setting, Slot,
};

/// The backoffice as a second service, as a choice on the `brocanth
/// create` menu. Named for what it adds to the admin, because the admin's
/// own module is `brocanth_admin::scaffold::Backoffice`.
pub struct BackofficeService;

/// Where the second binary listens when nothing says else.
pub const DEFAULT_ADDR: &str = "127.0.0.1:3001";

impl Module for BackofficeService {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "backoffice",
            "the admin as a service of its own: a second binary that serves only the admin",
        )
        .contributes::<AgentsSection>(
            Contribution::new("backoffice", "how the second binary serves the admin"),
            true,
        )
        // The server wrap and the admin's data line; the mise task.
        .category(Category::Middleware)
        .category(Category::External)
        .asked_in(
            "doors",
            "The admin as a service of its own: a second binary that serves only the admin",
        )
        .needs(
            "admin",
            "it serves the admin from a second binary, so there is nothing to serve without it",
        )
        // Inside the observer's wrap, so the probes still answer in the
        // backoffice binary and every refused request is still logged.
        .after("observability")
        .adapts_to(
            "superuser",
            "the claim each call carries holds the signed-in member of staff's grants",
        )
        .adapts_to(
            "admin-model",
            "the wire serves the admin's models over the ORM, not the in-memory fixture",
        )
        .adapts_to(
            "rls",
            "with `policy` too, the wire runs each call as the staff member's database identity",
        )
        .adapts_to(
            "policy",
            "with `rls` too, the wire runs each call as the staff member's database identity",
        )
        .krate(Dep::brocanth("brocanth-backoffice"))
        .absent(
            "brocanth-backoffice",
            "no second binary; the admin is served by the application alone and reads its rows directly",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/backoffice/mod.rs", plugin_rs(project));
        s.app_binary("backoffice", "src/bin/backoffice.rs", BIN_RS);
        s.at(Slot::PluginsMod).line("pub mod backoffice;");
        configurable(s, project, "backoffice", "Backoffice", settings());
        s.at(Slot::ServerWrap).line("backoffice::Door");
        s.at(Slot::AdminPanels).block(DATA_THROUGH.to_string());
        s.at(Slot::MiseTasks).block(MISE_TASK.to_string());
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The `[backoffice]` section.
fn settings() -> Vec<Setting> {
    vec![
        Setting::text(
            "addr",
            DEFAULT_ADDR,
            "Where `src/bin/backoffice.rs` listens: an address and a port.",
        ),
        Setting::bool(
            "embedded",
            true,
            "Serve the admin from the application binary too. False when the backoffice runs as its own service.",
        ),
        Setting::bool(
            "wire",
            false,
            "Serve the admin's rows to the backoffice binary over HTTP, at /_backoffice/wire. Needs service_secret.",
        )
        .note("Read by the application binary. The backoffice binary never serves it."),
        Setting::text(
            "data_url",
            "",
            "Where the backoffice binary reads the admin's rows: the application's URL, such as http://127.0.0.1:3000. Needs service_secret.",
        )
        .optional()
        .note("Read by src/bin/backoffice.rs only. Empty: it reads them itself, as the application does."),
        Setting::text(
            "service_secret",
            "",
            "The secret both binaries sign the wire with: 32 bytes or more, such as `openssl rand -hex 32`.",
        )
        .optional()
        .secret(),
        Setting::integer(
            "tx_timeout_secs",
            30,
            "Seconds the application keeps an idle transaction of the wire before it rolls it back.",
        ),
        Setting::integer(
            "tx_max",
            32,
            "The most transactions the application holds open for the wire at once.",
        ),
    ]
}

/// The line in `plugins::admin_panels`.
const DATA_THROUGH: &str =
    "    // The admin reads and writes its rows through the backoffice's tab data
    // contract, in this binary and in `src/bin/backoffice.rs`. Each call
    // carries the staff member's claim (docs/backoffice.md §7.1).
    config = config.data_through(crate::plugins::backoffice::contract);
";

const MISE_TASK: &str = r#"[tasks.backoffice]
description = "the admin alone, as its own service, at [backoffice] addr"
run = "cargo run --bin backoffice"
"#;

const AGENTS: &str = "\n\
    ## The backoffice binary\n\
    \n\
    `src/bin/backoffice.rs` runs this application and serves only the admin,\n\
    at `[backoffice] addr` (`mise run backoffice`). The application binary\n\
    serves the admin too while `[backoffice] embedded` is true; set it false\n\
    when the backoffice runs as its own service. Both binaries take the same\n\
    flags and the same config file.\n\
    \n\
    The admin reads its rows through the tab data contract\n\
    (`brocanth_backoffice::TabData`), so a call carries the staff member's\n\
    `Claim`. An identity function of the admin's data source reads the\n\
    `Claim` (`principal.downcast::<brocanth_backoffice::Claim>()`), not the\n\
    session type. Do not reach application state from a tab by any other\n\
    path: a tab must be able to move to the backoffice service.\n";

/// `src/bin/backoffice.rs`. The generator writes the application's `mod`
/// lines on the [`brocanth_scaffold::APP_MODS`] line, each with its
/// `#[path]`.
const BIN_RS: &str = concat!(
    r#"//! The backoffice as a service of its own: this application, serving only
//! the admin at `mount_at` in `src/modules/admin.rs` (docs/backoffice.md
//! §7.2).
//!
//! ```text
//! cargo run --bin backoffice                                  # at [backoffice] addr
//! cargo run --bin backoffice -- --backoffice.addr=0.0.0.0:3001
//! ```
//!
//! It compiles the application's own modules, below, so it links the same
//! models, policies and plugins, and opens the same pool. Run the
//! application with `--backoffice.embedded=false` to serve the admin from
//! here alone.

// The application's modules, compiled a second time. Some of what they
// hold serves only the application binary.
#![allow(dead_code)]

"#,
    "// @app-mods@",
    r#"

use std::process::ExitCode;

fn main() -> ExitCode {
    let served = plugins::read_line().and_then(|helped| {
        if helped {
            Ok(())
        } else {
            plugins::backoffice::serve()
        }
    });
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            plugins::say!(error, "error: {message}");
            ExitCode::FAILURE
        }
    }
}
"#
);

/// `claim`, without `superuser`: the principal's claim, with no grant.
const CLAIM: &str = r#"/// Who a call is for, as the contract carries it. Anonymous, or somebody
/// with no grant: there is no staff type to read the grants from. With
/// `superuser`, this reads `brocanth_auth::Staff`.
fn claim(principal: &Principal<'_>) -> Claim {
    Claim::from_principal(principal)
}
"#;

/// `claim`, with `superuser`: the signed-in member of staff and their
/// grants.
const CLAIM_STAFF: &str = r#"/// Who a call is for, as the contract carries it: the signed-in member of
/// staff, whether they are a superuser, and their permission strings
/// (`post.edit`, `jobs.read`).
fn claim(principal: &Principal<'_>) -> Claim {
    match principal.downcast::<brocanth_auth::Staff>() {
        Some(staff) => Claim::staff(
            staff.account().id.to_string(),
            staff.is_superuser(),
            staff.permissions().iter().cloned(),
        ),
        None => Claim::from_principal(principal),
    }
}
"#;

/// The wire's data side without `admin-model`: the admin's in-memory
/// source.
const WIRE_SOURCE: &str = r#"/// The admin's data source, as the wire serves it (docs/backoffice.md
/// §7.5, W9). Keep it the same as the one `plugins::admin` builds.
fn wire_source(_ctx: &Ctx) -> SharedData {
    Arc::new(brocanth_admin::Fixture::new())
}
"#;

/// The wire's data side with `admin-model`: the admin's models over the
/// ORM, compiled a second time from the admin's own file.
const WIRE_SOURCE_ORM: &str = r#"/// The admin's models, compiled a second time from the admin's own file,
/// as `src/bin/backoffice.rs` compiles the application. The admin keeps its
/// own `mod models` private, so this is a second copy of one list, not a
/// second list (docs/backoffice.md §7.5, W9).
#[allow(clippy::duplicate_mod)]
#[path = "../admin/models.rs"]
mod models;

/// The admin's data source, as the wire serves it (docs/backoffice.md
/// §7.5, W9). Keep it the same as the one `plugins::admin` builds.
fn wire_source(ctx: &Ctx) -> SharedData {
    match ctx.provided::<brocanth_model::Orm, _>() {
        Some(orm) => {
            let models = models::Models::new(orm.connection().clone());
            Arc::new(models@AS_IDENTITY@)
        }
        None => Arc::new(brocanth_admin::Fixture::new()),
    }
}
"#;

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "backoffice");
    let wire_source = if project.has("admin-model") {
        WIRE_SOURCE_ORM.replace(
            "@AS_IDENTITY@",
            if project.has("rls") && project.has("policy") {
                ".as_identity(crate::plugins::rls::staff)"
            } else {
                ""
            },
        )
    } else {
        WIRE_SOURCE.to_string()
    };
    let file = PLUGIN_RS
        .replace("@WIRE_SOURCE@", &wire_source)
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace("@DEFAULT_ADDR@", DEFAULT_ADDR)
        .replace(
            "@CLAIM@",
            if project.has("superuser") {
                CLAIM_STAFF
            } else {
                CLAIM
            },
        );
    redact_secrets(&file, &settings())
}

const PLUGIN_RS: &str = r#"//! The backoffice as a service of its own (docs/backoffice.md §7).
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! - [`contract`] puts the admin's data source on the tab data contract:
//!   `plugins::admin_panels` hands it to the admin. Each call carries the
//!   staff member's [`Claim`]. In `src/bin/backoffice.rs` with `data_url`
//!   set, the contract goes over HTTP to the application.
//! - [`Backoffice`] checks `[backoffice]` at start, in both binaries.
//! - [`Door`] keeps each binary to its own paths: `src/bin/backoffice.rs`
//!   answers only the admin's mount, and this binary answers it only while
//!   `embedded` is true. With `wire` on, this binary also answers the
//!   backoffice binary's signed calls at `/_backoffice/wire`.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth_admin::policy::Principal;
use brocanth_admin::SharedData;
use brocanth_backoffice::wire::{ServiceSecret, WireClient, WireServer};
use brocanth_backoffice::{Claim, Contracted, Gate, InProcess, Role, SharedTabs};

use crate::pages::Ctx;
use crate::plugins::{Answer, Handler, ServerWrap};

/// Where the backoffice binary listens, and whether this binary serves the
/// admin too: `[backoffice]` in the config file, `$<APP>_BACKOFFICE__<KEY>`,
/// or `--backoffice.<key>`. Both binaries read it.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// Where `src/bin/backoffice.rs` listens: an address and a port.
    pub addr: String,
    /// Whether the application binary serves the admin at `mount_at` too.
    /// False when the backoffice runs as its own service.
    pub embedded: bool,
    /// Whether the application binary serves the admin's rows to the
    /// backoffice binary over HTTP, at `/_backoffice/wire`.
    pub wire: bool,
    /// Where the backoffice binary reads the admin's rows: the
    /// application's URL. `None`: it reads them itself.
    pub data_url: Option<String>,
    /// The secret both binaries sign the wire with. Only from a file, a
    /// variable or `--backoffice.service_secret_file`/`_env`.
    pub service_secret: Option<String>,
    /// Seconds the application keeps an idle transaction of the wire.
    pub tx_timeout_secs: u64,
    /// The most transactions the application holds open for the wire.
    pub tx_max: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            addr: "@DEFAULT_ADDR@".to_string(),
            embedded: true,
            wire: false,
            data_url: None,
            service_secret: None,
            tx_timeout_secs: 30,
            tx_max: 32,
        }
    }
}

/// What [`Door`] reads, from the checked settings.
struct Chosen {
    gate: Gate,
    /// The wire's secret and its limits, when this binary serves it.
    wire: Option<(ServiceSecret, Duration, usize)>,
}

/// Checks `[backoffice]` when the application is built, in both binaries,
/// so a bad address stops the start and not the first request.
pub struct Backoffice {
    settings: Settings,
}

impl Plugin for Backoffice {
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "backoffice";

    fn new(settings: Settings) -> Self {
        Self { settings }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let settings = &self.settings;
        brocanth_backoffice::check_addr(&settings.addr).map_err(PluginError::new)?;
        // A missing or short secret, or a bad URL, stops the start in both
        // binaries (docs/backoffice.md §7.5, W3).
        let secret = brocanth_backoffice::wire::check_settings(
            settings.wire,
            settings.data_url.as_deref(),
            settings.service_secret.as_deref(),
            settings.tx_timeout_secs,
            settings.tx_max,
        )
        .map_err(PluginError::new)?;
        let gate = Gate::new(
            brocanth_backoffice::role(),
            crate::modules::admin::MODULE.mount_at,
            settings.embedded,
        );
        // Only the application serves the wire.
        let wire = match secret {
            Some(secret) if settings.wire && gate.role() == Role::App => Some((
                secret,
                Duration::from_secs(settings.tx_timeout_secs),
                settings.tx_max,
            )),
            _ => None,
        };
        lc.services_mut().insert(Chosen { gate, wire });
        Ok(())
    }
}

/// The admin's data source, on the tab data contract: the admin's end
/// ([`Contracted`]) over the data side in this process ([`InProcess`]), or
/// over the application's wire in the backoffice binary with `data_url`
/// set (docs/backoffice.md §7.5, W8). `plugins::admin_panels` passes it to
/// `AdminConfig::data_through`.
pub fn contract(data: SharedData) -> SharedData {
    let tabs: SharedTabs = match wire_client() {
        Some(client) => client.shared(),
        None => Arc::new(InProcess::new(data)),
    };
    Contracted::over(tabs).claim(claim).shared()
}

/// The client of the application's wire, in the backoffice binary when
/// `[backoffice] data_url` is set. Nothing is sent until the admin is
/// built, and a failure is the admin's start-time error.
///
/// The binary's name is compiled in: `contract` runs before any plugin
/// registers, so no service can say which binary this is. The
/// application binary ignores `data_url`, so one configuration serves both.
fn wire_client() -> Option<WireClient> {
    if option_env!("CARGO_BIN_NAME") != Some("backoffice") {
        return None;
    }
    let settings =
        crate::plugins::layers().and_then(|layers| crate::plugins::backoffice_settings(&layers));
    match settings {
        Ok(settings) => {
            let url = settings.data_url.filter(|url| !url.trim().is_empty())?;
            Some(WireClient::lazy(&url, settings.service_secret.as_deref()))
        }
        Err(message) => Some(WireClient::failed(message)),
    }
}

@WIRE_SOURCE@
@CLAIM@
/// Serves the admin alone, at `[backoffice] addr`. `src/bin/backoffice.rs`
/// calls it; the application binary does not.
#[allow(dead_code)]
pub fn serve() -> Result<(), String> {
    brocanth_backoffice::serve_as(Role::Backoffice);
    let settings = crate::plugins::backoffice_settings(&crate::plugins::layers()?)?;
    let addr = brocanth_backoffice::check_addr(&settings.addr)?;
    crate::app::serve(&addr.to_string())
}

/// A server wrap: a path this binary does not serve answers 404. With
/// `wire` on, the application answers the backoffice binary's signed calls
/// here, before the request hooks: a machine call needs no session.
pub struct Door;

impl ServerWrap for Door {
    fn wrap(app: &Arc<crate::app::App>, next: Handler) -> Handler {
        let chosen = app
            .core
            .services()
            .get::<Chosen>()
            .expect("`backoffice` is registered in `register!`");
        let gate = chosen.gate.clone();
        let wire = chosen.wire.clone();
        // Built by the first call, when the plugins have started and the
        // admin's source can be reached. This application's, not the
        // process's.
        let server: Arc<OnceLock<WireServer>> = Arc::default();
        let app = Arc::clone(app);
        Arc::new(move |request: brocanth_server::Request| -> Answer {
            if let Some((secret, timeout, max)) = &wire {
                if request.path == brocanth_backoffice::wire::PATH {
                    let server = server
                        .get_or_init(|| {
                            let ctx = Ctx::new(
                                brocanth::routing::Verb::Post,
                                brocanth_backoffice::wire::PATH,
                                brocanth::routing::Params::empty(),
                            )
                            .with_plugins(app.core.context());
                            WireServer::new(wire_source(&ctx), secret.clone())
                                .holding(*timeout, *max)
                        })
                        .clone();
                    return Box::pin(async move { server.handle(request).await });
                }
            }
            if gate.admits(&request.path) {
                next(request)
            } else {
                Box::pin(async { brocanth_server::Response::text(404, "Not Found") })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use brocanth::routing::Verb;
    use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
    use brocanth_admin::{Admin, AdminConfig, Fixture, LateCells, ListQuery, Principal};
    use brocanth_backoffice::{Gate, Role};

    /// The application binary serves the admin while it is embedded: the
    /// admin answers, locked, and so do the pages.
    #[tokio::test]
    async fn the_application_serves_the_admin_while_it_is_embedded() {
        let app = crate::app::build().expect("the application builds");
        let at = crate::modules::admin::MODULE.mount_at;
        let admin = crate::app::handle(&app, brocanth_server::Request::new(Verb::Get, at)).await;
        assert_eq!(admin.status, 403);
        // The gate lets a page through: whatever it answers, it is not
        // the gate's 404. A locale module may redirect it.
        let home = crate::app::handle(&app, brocanth_server::Request::new(Verb::Get, "/")).await;
        assert_ne!(home.status, 404);
    }

    /// The backoffice binary answers the admin's mount and nothing else;
    /// the application hands the mount over when it is not embedded.
    #[test]
    fn each_binary_answers_its_own_paths() {
        let at = crate::modules::admin::MODULE.mount_at;
        let backoffice = Gate::new(Role::Backoffice, at, true);
        assert!(backoffice.admits(at));
        assert!(!backoffice.admits("/"));
        let split = Gate::new(Role::App, at, false);
        assert!(!split.admits(at));
        assert!(split.admits("/"));
    }

    /// The admin reads its rows through the contract: the line in
    /// `plugins::admin_panels` maps its source.
    #[tokio::test]
    async fn the_admin_reads_its_rows_through_the_contract() {
        let mut late = LateCells::new();
        let config = crate::plugins::admin_panels(AdminConfig::new(), &mut late)
            .unscoped_rows_unsafe_for_development()
            .with_policy(AllowAllUnsafeForDevelopment);
        let admin = Admin::try_new(Arc::new(Fixture::blog()), config).expect("the admin builds");
        let page = admin
            .list(Principal::anonymous(), "post", ListQuery::new())
            .await
            .expect("a page");
        assert_eq!(page.status(), 200);
        assert!(page.html().is_some_and(|html| html.contains("Compilers")));
    }

    /// The default address passes the check the plugin runs at start.
    #[test]
    fn the_default_address_is_an_address() {
        let settings = super::Settings::default();
        assert!(brocanth_backoffice::check_addr(&settings.addr).is_ok());
    }

    /// The wire with no secret stops the start, with the flag to fix it.
    #[test]
    fn the_wire_without_a_secret_stops_the_start() {
        use brocanth::plugin_api::Plugin as _;
        let settings = super::Settings {
            wire: true,
            ..super::Settings::default()
        };
        let built = brocanth::plugin_app::AppBuilder::new()
            .plugin(super::Backoffice::new(settings))
            .build();
        let Err(error) = built else {
            panic!("the wire without a secret must not build");
        };
        let error = error.to_string();
        assert!(error.contains("service_secret_file"), "{error}");
    }

    /// The wire is off by default: its path is not served, and nothing
    /// signs an answer there.
    #[tokio::test]
    async fn the_wire_is_not_served_while_it_is_off() {
        let app = Arc::new(crate::app::build().expect("the application builds"));
        let post = || brocanth_server::Request::new(Verb::Post, brocanth_backoffice::wire::PATH);
        let handler = crate::plugins::wrap(&app, {
            let app = Arc::clone(&app);
            move |request| {
                let app = Arc::clone(&app);
                async move { crate::app::handle(&app, request).await }
            }
        });
        // Off by default: whatever the pages answer, it is not the wire.
        let off = handler(post()).await;
        assert_ne!(off.status, 401);
        assert!(off
            .header(brocanth_backoffice::wire::SIGNATURE_HEADER)
            .is_none());
    }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth_scaffold::APP_MODS;

    #[test]
    fn the_binary_holds_the_app_mods_line_once() {
        assert_eq!(
            BIN_RS
                .lines()
                .filter(|line| line.trim() == APP_MODS)
                .count(),
            1
        );
    }
}
