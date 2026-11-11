//! `audit` -- what `brocanth create` writes when somebody asks for a
//! change history.
//!
//! The module is here and not in the CLI because what it writes is a
//! statement about this crate: `AuditPlugin`, the fields of `AuditConfig`,
//! `Trail::by` and `admin::install`.
//!
//! What a project gets:
//!
//! - `src/plugins/audit/mod.rs`: [`sink`] for a page that writes, and the
//!   function that says who is asking;
//! - one registration in `src/plugins/mod.rs`, after the `db` plugin, so
//!   the plugin finds the pool and keeps the history in `PgStore`;
//! - `[audit]` in the config file, or flags and variables without one;
//! - with `admin`, the History tab, the journal and the History link,
//!   behind `integrate.admin` in `src/modules/audit.rs`.
//!
//! Compiled only under the `scaffold` feature, which is off.
//!
//! [`sink`]: https://docs.rs/brocanth-audit

use brocanth_scaffold::snippet::{configurable_hooking, integration, Hooking};
use brocanth_scaffold::{Category, Dep, Hook, Manifest, Module, Project, Scaffold, Setting, Slot};

/// The change history, as a choice on the `brocanth create` menu.
pub struct Audit;

impl Module for Audit {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "audit",
            "change history: who changed which row, when, and what; rows in your Postgres",
        )
        // Middleware first: the plugin installs a hook (a model hook), and
        // the hook record is a middleware's slot.
        .category(Category::Middleware)
        .category(Category::Enhancer)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in(
            "operations",
            "Change history: who changed which row, and when",
        )
        .needs(
            "db",
            "the history is rows in the database the `db` module opens; in memory a restart would lose it",
        )
        .needs(
            "model",
            "it records the changes the model layer publishes, and without `model` there are none",
        )
        .krate(
            Dep::brocanth("brocanth-audit")
                .feature_with("config", "config")
                .feature_with("admin", "admin")
                .note("the change history, its model hook and its Postgres store"),
        )
        // The plugin reads the pool from the services the `db` plugin
        // fills, so it registers after it.
        .after("db")
        .hook(
            "record",
            "record each change the model layer publishes through `HookSink`",
            Hook::on(),
        )
        .integration(
            "admin",
            "a History tab in the admin, a History link on every record, and every admin write recorded with its values",
            true,
        )
        .absent(
            "brocanth-audit",
            "no change history; who changed a row is not recorded",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/audit/mod.rs", AUDIT_RS.to_string());
        s.at(Slot::PluginsMod).line("pub mod audit;");
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "audit",
                section: "audit",
                ty: "brocanth_audit::AuditPlugin",
                config_ty: "brocanth_audit::AuditConfig",
                prelude: PRELUDE.to_string(),
                adjust: None,
            },
            settings(),
        );
        if project.has("admin") {
            integration(s, "audit", "admin", Slot::AdminPanels, ADMIN_TAB);
        }
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

fn settings() -> Vec<Setting> {
    vec![
        Setting::integer(
            "retention_days",
            365,
            "Days a change is kept. 0 keeps every change.",
        )
        .note("Older changes are deleted once a day."),
        Setting::integer("flush_secs", 5, "Seconds between two writes of the queue."),
        Setting::bool(
            "migrate",
            true,
            "Create the brocanth_audit table on the first readiness check.",
        )
        .note("Off, for a project that keeps brocanth_audit::PgStore::SCHEMA in a migration."),
    ]
}

/// The history in the admin. The panel and the journal find the plugin's
/// trail when they run, because the admin's `mount` runs before a plugin
/// could hand it one.
const ADMIN_TAB: &str = "// The change history: a tab, a link on every record, every admin write.
config = brocanth_audit::admin::install(config);";

const PRELUDE: &str = r#"
        // -- brocanth-audit: change history ---------------------------------
        //
        // A model hook: each change the model layer publishes becomes a row
        // of `brocanth_audit`, written every `flush_secs` seconds, never
        // during a request. Rows older than `retention_days` go once a day.
"#;

/// `src/plugins/audit/mod.rs`.
const AUDIT_RS: &str = r#"//! The change history: who is asking, and the sink a page writes through.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! A write through the model layer is recorded when it is published to the
//! sink below:
//!
//! ```ignore
//! post.insert_published(ctx.orm(), &crate::plugins::audit::sink(ctx)).await?;
//! ```
//!
//! The history is the `brocanth_audit` table of the `[db]` database. With
//! the `admin` module, the admin's History tab lists it, every
//! record has a History link, and every admin write is recorded with the
//! values before and after.

use brocanth_audit::{By, Trail};

use crate::pages::Ctx;

/// A sink that records each change as made by whoever made this request.
///
/// Records nothing on a `Ctx` built by hand, which has no plugin behind it.
#[allow(dead_code)]
pub fn sink(ctx: &Ctx) -> By {
    match ctx.provided::<Trail, _>() {
        Some(trail) => trail.sink(actor(ctx)),
        None => By::nothing(),
    }
}

/// Who is asking, as the history names them.
///
/// Nobody until this reads your user off the request. With a user on the
/// context, this is one line:
///
/// ```ignore
/// ctx.provided::<CurrentUser, _>().map(|user| user.email.clone())
/// ```
#[allow(dead_code)]
fn actor(_ctx: &Ctx) -> Option<String> {
    None
}
"#;

/// What the generated `AGENTS.md` says about the history.
const AGENTS: &str = "\n\
    ## Change history\n\
    \n\
    Publish every write of the model layer to\n\
    `crate::plugins::audit::sink(ctx)`: `insert_published`,\n\
    `update_published`, `delete_published`. A write that is not published\n\
    is not in the history. Fill `actor` in `src/plugins/audit/mod.rs` once\n\
    the project has users, so each change names who made it.\n\
    \n\
    Do not put a secret in a column you write through the sink with the\n\
    admin: the admin records the values before and after.\n";
