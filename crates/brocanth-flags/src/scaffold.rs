//! `flags` -- what `brocanth create` writes when somebody asks for feature
//! flags.
//!
//! The module is here and not in the CLI because what it writes is a
//! statement about this crate: `FlagsPlugin`, the fields of `FlagsConfig`,
//! `Flag::new` and `Flags::enabled`.
//!
//! What a project gets:
//!
//! - `src/plugins/flags/mod.rs`: the list of flags, [`enabled`] for a
//!   page, and the function that says who is asking;
//! - one registration in `src/plugins/mod.rs`, after the `db` plugin, so
//!   the plugin finds the pool and keeps the rules in `PgStore`;
//! - `[flags]` in the config file, or flags and variables without one;
//! - with `admin`, a `flags` tab in the admin, behind `integrate.admin` in
//!   `src/modules/flags.rs`.
//!
//! Compiled only under the `scaffold` feature, which is off.
//!
//! [`enabled`]: https://docs.rs/brocanth-flags

use brocanth_scaffold::snippet::{configurable, integration, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// Feature flags, as a choice on the `brocanth create` menu.
pub struct FeatureFlags;

impl Module for FeatureFlags {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "flags",
            "feature flags: on for all, a percentage, users or groups; rows in your Postgres",
        )
        .category(Category::Enhancer)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in(
            "operations",
            "Feature flags: turn code on for some users without a deploy",
        )
        .needs(
            "db",
            "the rules are rows in the database the `db` module opens; in memory a restart would turn every flag off",
        )
        .krate(
            Dep::brocanth("brocanth-flags")
                .feature_with("config", "config")
                .feature_with("admin", "admin")
                .note("the flag rules, their in-process copy and their Postgres store"),
        )
        // The plugin reads the pool from the services the `db` plugin
        // fills, so it registers after it.
        .after("db")
        .integration(
            "admin",
            "a `flags` tab in the admin that turns each declared flag on or off",
            true,
        )
        .absent(
            "brocanth-flags",
            "no feature flags; new code is on for everybody the moment it ships",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/flags/mod.rs", FLAGS_RS.to_string());
        s.at(Slot::PluginsMod).line("pub mod flags;");
        configurable(s, project, "flags", "Flags", settings());
        // The plugin's own configuration type, not a generated one: the
        // generated readers start from its `Default` and set its fields.
        s.settings_type("flags", "brocanth_flags::FlagsConfig");
        if !project.has("config") {
            // The literal form calls `Plugin::new`, a trait method.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("admin") {
            integration(s, "flags", "admin", Slot::AdminPanels, ADMIN_TAB);
        }
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The flags tab, with the `admin` module. The panel finds the plugin's
/// copy when it renders, because the admin's `mount` runs before a plugin
/// could hand it one. It lists the flags the project declares.
const ADMIN_TAB: &str = "// The feature flags, as a tab that turns each one on or off.
config = config.panel(brocanth_flags::admin::FlagsPanel::installed(
    crate::plugins::flags::ALL,
));";

fn settings() -> Vec<Setting> {
    vec![
        Setting::integer(
            "refresh_secs",
            10,
            "Seconds between two reads of the table. 0 reads only at start.",
        )
        .note("A change made in another process shows here within this time."),
        Setting::bool(
            "migrate",
            true,
            "Create the brocanth_flags table on the first readiness check.",
        )
        .note("Off, for a project that keeps brocanth_flags::PgStore::SCHEMA in a migration."),
    ]
}

/// `src/plugins/flags/mod.rs`.
const FLAGS_RS: &str = r#"//! Feature flags: the list this application reads, and the call a page
//! makes.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! Declare a flag here as a constant, add it to [`ALL`], and read it in a
//! page with [`enabled`]:
//!
//! ```ignore
//! if crate::plugins::flags::enabled(ctx, crate::plugins::flags::NEW_CHECKOUT) {
//!     // the new flow
//! }
//! ```
//!
//! The constant, not a string: a misspelt flag does not compile. A flag
//! with no stored rule is off, so code can ship before anybody turns it on.
//!
//! The rules are rows in the `brocanth_flags` table of the `[db]` database.
//! The plugin reads them before the server listens and again every
//! `refresh_secs`, so a page never waits on the database for a flag. With
//! the `admin` module, the admin's `flags` tab turns each flag on for
//! everyone, for a percentage of signed-in users, or for a list of users
//! and groups.

use brocanth_flags::{Actor, Flag};

use crate::pages::Ctx;

/// The plugin, from the library. `[flags]` configures it.
pub use brocanth_flags::FlagsPlugin as Flags;

/// An example. Rename it, or delete it and its line in [`ALL`].
pub const NEW_CHECKOUT: Flag = Flag::new("new_checkout", "The checkout flow from the redesign.");

/// Every flag this application reads. The admin tab lists these, so a flag
/// can be turned on before anybody stored a rule for it.
#[allow(dead_code)]
pub const ALL: &[Flag] = &[NEW_CHECKOUT];

/// Whether `flag` is on for whoever made this request.
///
/// Off on a `Ctx` built by hand, which has no plugin behind it.
#[allow(dead_code)]
pub fn enabled(ctx: &Ctx, flag: Flag) -> bool {
    ctx.provided::<brocanth_flags::Flags, _>()
        .is_some_and(|flags| flags.enabled(flag, &actor(ctx)))
}

/// Who is asking, as the flags see it.
///
/// Anonymous until this reads your user off the request, and an anonymous
/// request sees only a flag that is on for everyone. With a user on the
/// context, this is two lines:
///
/// ```ignore
/// ctx.provided::<CurrentUser, _>()
///     .map_or_else(Actor::anonymous, |user| Actor::new(user.id.to_string()).in_group("staff"))
/// ```
fn actor(_ctx: &Ctx) -> Actor {
    Actor::anonymous()
}
"#;

/// What the generated `AGENTS.md` says about flags.
const AGENTS: &str = "\n\
    ## Feature flags\n\
    \n\
    Declare every flag as a `Flag` constant in `src/plugins/flags/mod.rs`\n\
    and add it to `ALL`. Read it with\n\
    `crate::plugins::flags::enabled(ctx, crate::plugins::flags::NAME)`, never\n\
    with a string key: a constant cannot be misspelt.\n\
    \n\
    A flag with no stored rule is off. Ship the code first, then turn the\n\
    flag on. When a flag is on for everyone for good, delete the constant\n\
    and the branch that reads it; the admin tab then lists the stored rule\n\
    as not declared, so it can be removed.\n";
