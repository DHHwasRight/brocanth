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
//!   `embedded`), a plugin that checks them at start, the server wrap that
//!   keeps each binary to its own paths, and `contract`, which puts the
//!   admin's data source on the tab data contract.
//! * One line in `plugins::admin_panels`, a generated file:
//!   `config.data_through(crate::plugins::backoffice::contract)`. So an
//!   admin file written before this module needs no edit.
//! * A `mise` task, `backoffice`, and a section in `AGENTS.md`.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{configurable, settings_derive};
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

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "backoffice");
    PLUGIN_RS
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
        )
}

const PLUGIN_RS: &str = r#"//! The backoffice as a service of its own (docs/backoffice.md §7).
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! - [`contract`] puts the admin's data source on the tab data contract:
//!   `plugins::admin_panels` hands it to the admin. Each call carries the
//!   staff member's [`Claim`].
//! - [`Backoffice`] checks `[backoffice]` at start, in both binaries.
//! - [`Door`] keeps each binary to its own paths: `src/bin/backoffice.rs`
//!   answers only the admin's mount, and this binary answers it only while
//!   `embedded` is true.

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth_admin::policy::Principal;
use brocanth_admin::SharedData;
use brocanth_backoffice::{Claim, Contracted, Gate, InProcess, Role};

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            addr: "@DEFAULT_ADDR@".to_string(),
            embedded: true,
        }
    }
}

/// The gate [`Door`] reads, from the checked settings.
struct Chosen(Gate);

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
        brocanth_backoffice::check_addr(&self.settings.addr).map_err(PluginError::new)?;
        lc.services_mut().insert(Chosen(Gate::new(
            brocanth_backoffice::role(),
            crate::modules::admin::MODULE.mount_at,
            self.settings.embedded,
        )));
        Ok(())
    }
}

/// The admin's data source, on the tab data contract: the admin's end
/// ([`Contracted`]) over the data side in this process ([`InProcess`]).
/// `plugins::admin_panels` passes it to `AdminConfig::data_through`.
pub fn contract(data: SharedData) -> SharedData {
    Contracted::new(InProcess::new(data)).claim(claim).shared()
}

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

/// A server wrap: a path this binary does not serve answers 404.
pub struct Door;

impl ServerWrap for Door {
    fn wrap(app: &Arc<crate::app::App>, next: Handler) -> Handler {
        let gate = app
            .core
            .services()
            .get::<Chosen>()
            .map(|chosen| chosen.0.clone())
            .expect("`backoffice` is registered in `register!`");
        Arc::new(move |request: brocanth_server::Request| -> Answer {
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
        let home = crate::app::handle(&app, brocanth_server::Request::new(Verb::Get, "/")).await;
        assert_eq!(home.status, 200);
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
