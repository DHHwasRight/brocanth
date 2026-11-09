//! `superuser` and `roles` -- the people who operate the application.
//!
//! Both write over [`crate::staff`]: a superuser flag on an account, and
//! roles that grant permissions. The admin reads them through
//! [`crate::admin::StaffPolicy`].
//!
//! # Why the admin's own file changes for `superuser`
//!
//! The policy is a line in `plugins::admin_panels`, behind
//! `integrate.admin` in `src/modules/superuser.rs`. The principal is not:
//! the admin asks for it in its own `screen`, and reading a session is
//! async, which no hook or provider in the core may be. So `admin` adapts
//! to `superuser` and its `screen` awaits [`staff`](crate::staff::Staff)
//! before it dispatches. With the integration off, the principal is still
//! the member of staff, so a policy you write can read it.
//!
//! # Why `roles` needs `superuser`
//!
//! Only a superuser changes roles: a role holder who could edit roles could
//! give itself every permission. Without a superuser nobody could give the
//! first role. `superuser` also owns the function that reads the signed-in
//! member of staff, which `roles` uses.

use brocanth_scaffold::snippet::{configurable, integration, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

use super::uses_postgres;

/// Operator accounts, as a choice on the `brocanth create` menu.
pub struct Superuser;

impl Module for Superuser {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "superuser",
            "operator accounts that pass every admin check, and a createsuperuser command",
        )
        .category(Category::Enhancer)
        .needs(
            "auth",
            "a superuser is an account with a flag, and `auth` holds the accounts",
        )
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "commands",
            "the binary gets `createsuperuser`, when the accounts are in Postgres",
        )
        .adapts_to("jobs", "`createsuperuser` opens the accounts in Postgres")
        .adapts_to("cache", "`createsuperuser` opens the accounts in Postgres")
        .integration(
            "admin",
            "the admin's policy is StaffPolicy: a superuser passes every check, a role holder what its roles grant",
            true,
        )
        .asked_in(
            "doors",
            "Superusers who can open the admin, and a createsuperuser command",
        )
        .krate(
            Dep::brocanth("brocanth-auth")
                .feature_with("admin", "admin")
                .note("StaffPolicy and the roles tab, with the admin"),
        )
        // The plugin reads the auth services in `before_start`, so it
        // registers after the auth plugin publishes them.
        .after("auth")
        .absent(
            "superuser",
            "no operator account; the admin opens only for a policy you write",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let (derive, note) = settings_derive(project, "superuser");
        s.file(
            "src/plugins/superuser/mod.rs",
            SUPERUSER_RS
                .replace("@DERIVE@", &derive)
                .replace("@NOTE@", &note)
                .replace("@VAR@", &password_var(project)),
        );
        s.at(Slot::PluginsMod).line("pub mod superuser;");
        configurable(s, project, "superuser", "Superuser", settings(project));
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("admin") {
            integration(s, "superuser", "admin", Slot::AdminPanels, ADMIN_POLICY);
        }
        // An account made by a command against a `MemoryStore` is gone when
        // the command exits, so the command exists only over Postgres. Both
        // are asked, so each declared adaptation is asked in every project.
        let postgres = [uses_postgres(project), project.has("cache")].contains(&true);
        if project.has("commands") && postgres {
            s.file(
                "src/commands/createsuperuser.rs",
                command_rs(project.has("config")),
            );
            s.at(Slot::CommandsMod).line("pub mod createsuperuser;");
        }
        s.at(Slot::AgentsSections)
            .block(SUPERUSER_AGENTS.to_string());
    }
}

/// `MY_BLOG_SUPERUSER_PASSWORD`.
fn password_var(project: &Project) -> String {
    format!("{}_SUPERUSER_PASSWORD", project.ident().to_uppercase())
}

fn settings(project: &Project) -> Vec<Setting> {
    let var = password_var(project);
    vec![
        Setting::text(
            "email",
            "",
            "The address of the superuser the server makes at start.",
        )
        .optional()
        .note(
            "Idempotent: an account that exists is promoted and keeps its password.\n\
             With accounts in memory this is the only way to have a superuser.",
        ),
        Setting::text(
            "password_env",
            &var,
            "The name of the variable that holds that superuser's password.",
        )
        .note("Not the password. It is read only to create the account."),
    ]
}

/// The admin's policy, behind `integrate.admin`.
const ADMIN_POLICY: &str =
    "// Superusers pass every check; a role holder passes what its roles grant.
config = config.with_policy(brocanth_auth::admin::StaffPolicy);";

const SUPERUSER_RS: &str = r##"//! Superusers: operator accounts that pass every check of the admin.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! A superuser is a flag on an account. An operator sets it, never a form a
//! visitor can reach:
//!
//! * at start, from `[superuser] email` and the password in the variable
//!   that `password_env` names, when `email` is set;
//! * with `createsuperuser --email <address>`, when the accounts are in
//!   Postgres.
//!
//! Both are idempotent. An account that exists is promoted and keeps its
//! password, so a second run does not reset a password somebody changed.
//!
//! [`staff`] reads the signed-in member of staff off a request: a
//! superuser, or an account that holds a role. The admin uses it as its
//! principal. A page of yours can too:
//!
//! ```ignore
//! let Some(staff) = crate::plugins::superuser::staff(ctx).await? else {
//!     return Err(Error::new(403, "staff only"));
//! };
//! ```

use std::sync::{Arc, LazyLock, OnceLock};

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth_auth::{ensure_superuser, Staff, SuperuserOutcome};

use crate::pages::{Ctx, Error};
use crate::plugins::auth::{Services, Store};

/// The superuser the server makes at start: `[superuser]` in the config
/// file, or `$<APP>_SUPERUSER__<KEY>`.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// The superuser's address. `None` makes no superuser at start.
    pub email: Option<String>,
    /// The *name* of the variable that holds its password, not the
    /// password.
    pub password_env: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            email: None,
            password_env: "@VAR@".to_string(),
        }
    }
}

/// The accounts store, once the auth plugin has built it. The admin's
/// roles tab reads it: the admin is built with the route table, before any
/// plugin starts.
static STORE: LazyLock<Arc<OnceLock<Arc<Store>>>> = LazyLock::new(Default::default);

/// The cell [`STORE`] fills at startup.
#[allow(dead_code)] // read by the roles tab, with `roles`
pub fn store() -> Arc<OnceLock<Arc<Store>>> {
    Arc::clone(&STORE)
}

/// The signed-in member of staff, or `None`: no session, a pre-login one,
/// or an account that is neither a superuser nor holds a role.
///
/// # Errors
/// A store that fails is a 500. A missing or forged cookie is `None`.
#[allow(dead_code)] // read by the admin, with `admin`
pub async fn staff(ctx: &Ctx) -> Result<Option<Staff>, Error> {
    let (Some(auth), Some(cookies)) = (ctx.provided::<Services, _>(), ctx.header("cookie")) else {
        return Ok(None);
    };
    Staff::from_cookie_header(auth, cookies)
        .await
        .map_err(|e| Error::internal(format!("superuser: {e}")))
}

pub struct Superuser {
    settings: Settings,
    /// Read in `configure`, used once in `start`.
    password: Option<String>,
}

impl Plugin for Superuser {
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "superuser";

    fn new(config: Settings) -> Self {
        Self {
            settings: config,
            password: None,
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if self.settings.email.is_none() {
            return Ok(());
        }
        let name = &self.settings.password_env;
        match std::env::var(name) {
            Ok(password) if !password.is_empty() => self.password = Some(password),
            // This project's own tests build the application with no secret
            // in the environment, and make no superuser.
            _ if cfg!(test) => {}
            _ => {
                return Err(PluginError::new(format!(
                    "[superuser] email is set and ${name} is not\n\
                     help: export {name}=<a long passphrase>, or unset [superuser] email"
                )));
            }
        }
        Ok(())
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let services = lc.require_service::<Services>()?;
        let _ = STORE.set(Arc::clone(services.store()));
        Ok(())
    }

    fn start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let (Some(email), Some(password)) = (self.settings.email.clone(), self.password.take())
        else {
            return Ok(());
        };
        let services = lc.require_service::<Services>()?.clone();
        // `start` runs after the startup gate, so the auth tables exist.
        tokio::spawn(async move {
            let outcome = ensure_superuser(
                services.accounts(),
                services.store().as_ref(),
                &email,
                &password,
            )
            .await;
            match outcome {
                Ok(SuperuserOutcome::Created(_)) => {
                    crate::plugins::say!(info, "superuser: created {email}"; %email);
                }
                Ok(SuperuserOutcome::Promoted(_)) => {
                    crate::plugins::say!(info, "superuser: promoted {email}"; %email);
                }
                Ok(SuperuserOutcome::Unchanged(_)) => {}
                Err(e) => {
                    crate::plugins::say!(warn, "superuser: {email}: {e}"; %email, error = %e);
                }
            }
        });
        Ok(())
    }
}
"##;

fn command_rs(configured: bool) -> String {
    COMMAND_RS.replace(
        "@SETTINGS@",
        if configured {
            COMMAND_SETTINGS_FROM_CONFIG
        } else {
            COMMAND_SETTINGS_FROM_ENV
        },
    )
}

const COMMAND_RS: &str = r#"//! `createsuperuser` -- an operator account that passes every check of the
//! admin.
//!
//! Opens the accounts at `[auth] url` and makes the account with the
//! address a superuser. It creates the account first when there is none,
//! with the password in the variable that `[superuser] password_env` names.
//! An account that exists keeps its password. Run it twice and nothing
//! changes.
//!
//! Ordinary project code, like `serve`: rename it or delete it, and delete
//! its line in `src/commands/mod.rs`.

use std::sync::Arc;

use brocanth_auth::{
    ensure_superuser, Accounts, Passwords, PgStore, SuperuserOutcome, SystemClock,
};
use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Make the account with this address a superuser, creating it if needed
#[cmd]
#[derive(clap::Args)]
pub struct Createsuperuser {
    /// The account's address
    #[arg(long, value_name = "ADDRESS")]
    email: String,
}

impl Command for Createsuperuser {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        std::env::set_current_dir(ctx.root())?;
        let (auth, superuser) = settings()?;
        let name = superuser.password_env;
        let password = std::env::var(&name).unwrap_or_default();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let outcome = runtime.block_on(async {
            let store = Arc::new(PgStore::connect(&auth.url).await?);
            store.migrate().await?;
            let accounts = Accounts::new(
                Arc::clone(&store),
                Arc::new(SystemClock),
                Arc::new(Passwords::recommended()?),
            )?;
            anyhow::Ok(ensure_superuser(&accounts, store.as_ref(), &self.email, &password).await)
        })?;
        match outcome {
            Ok(SuperuserOutcome::Created(_)) => println!("created superuser {}", self.email),
            Ok(SuperuserOutcome::Promoted(_)) => println!("{} is now a superuser", self.email),
            Ok(SuperuserOutcome::Unchanged(_)) => println!("{} is a superuser already", self.email),
            // A new account with no password: say where it goes.
            Err(e) if password.is_empty() => anyhow::bail!(
                "{e}\nhelp: a new account needs a password; export {name}=<a long passphrase>"
            ),
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }
}
@SETTINGS@"#;

const COMMAND_SETTINGS_FROM_CONFIG: &str = r#"
/// `[auth]` and `[superuser]`, through the loader the server uses.
fn settings() -> anyhow::Result<(
    crate::plugins::auth::Settings,
    crate::plugins::superuser::Settings,
)> {
    let loaded = brocanth_config::Loader::new(crate::app::APP).load()?;
    Ok((loaded.section("auth")?, loaded.section("superuser")?))
}
"#;

const COMMAND_SETTINGS_FROM_ENV: &str = r#"
/// `[auth]` and `[superuser]`: the defaults, or the environment, as the
/// server reads them.
fn settings() -> anyhow::Result<(
    crate::plugins::auth::Settings,
    crate::plugins::superuser::Settings,
)> {
    Ok((
        crate::plugins::auth_settings(&[]).map_err(anyhow::Error::msg)?,
        crate::plugins::superuser_settings(&[]).map_err(anyhow::Error::msg)?,
    ))
}
"#;

const SUPERUSER_AGENTS: &str = "\n\
    ## Superusers\n\
    \n\
    `src/plugins/superuser/mod.rs` reads the signed-in member of staff with\n\
    `staff(ctx)`. A superuser passes every check of the admin. Never set the\n\
    superuser flag from a form or a page: an operator sets it, with\n\
    `[superuser] email` at start or the `createsuperuser` command.\n";

/// Roles and permissions, as a choice on the `brocanth create` menu.
pub struct Roles;

impl Module for Roles {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "roles",
            "roles that grant permissions, stored with the accounts, for staff and the admin",
        )
        .category(Category::Enhancer)
        .needs(
            "superuser",
            "only a superuser gives a role, and `superuser` reads the signed-in member of staff",
        )
        .integration(
            "admin",
            "a Roles tab where a superuser defines roles and gives them to accounts",
            true,
        )
        .asked_in("doors", "Roles and permissions for staff")
        .krate(
            Dep::brocanth("brocanth-auth")
                .feature("policy")
                .note("Staff::gate, a permission as a brocanth-policy predicate"),
        )
        .after("superuser")
        .absent(
            "roles",
            "no roles; a member of staff is a superuser, and nobody else is",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/roles/mod.rs", ROLES_RS.to_string());
        s.at(Slot::PluginsMod).line("pub mod roles;");
        if project.has("admin") {
            integration(s, "roles", "admin", Slot::AdminPanels, ADMIN_TAB);
        }
        s.at(Slot::AgentsSections).block(ROLES_AGENTS.to_string());
    }
}

/// The roles tab, behind `integrate.admin`.
const ADMIN_TAB: &str = "// Roles: a superuser defines them and gives them to accounts.
let roles = brocanth_auth::admin::RolesPanel::late(crate::plugins::superuser::store());
config = config.panel(roles);";

const ROLES_RS: &str = r##"//! Roles: named sets of permissions, given to accounts.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! A permission is a string. The admin asks for `<model>.view`, `.add`,
//! `.change` and `.delete` for a model, and `<panel>.view` and `.act` for a
//! tab. A page of yours may ask for any other. A superuser holds every
//! permission. Only a superuser changes roles, in the admin's Roles tab.
//!
//! Who may see a page is a role question:
//!
//! ```ignore
//! let staff = crate::plugins::roles::require(ctx, "report.view").await?;
//! ```
//!
//! Which rows is a row question, and it goes into the query through
//! `brocanth-policy`. `Staff::gate` turns a permission into a predicate, so
//! both travel in one `condition()`:
//!
//! ```ignore
//! fn update(&self, who: &Visitor) -> EntityPred<Post> {
//!     // Editors change every post; everybody else changes their own.
//!     who.staff.gate("post.change").or(Pred::eq(PostColumn::AuthorId, who.id))
//! }
//! ```

use brocanth_auth::Staff;

use crate::pages::{Ctx, Error};

/// The signed-in member of staff, when it holds `permission`.
///
/// # Errors
/// 401 when nobody on the staff is signed in, 403 when the account lacks
/// the permission.
#[allow(dead_code)] // for your pages
pub async fn require(ctx: &Ctx, permission: &str) -> Result<Staff, Error> {
    let Some(staff) = crate::plugins::superuser::staff(ctx).await? else {
        return Err(Error::new(401, "sign in with a staff account"));
    };
    if !staff.allows(permission) {
        return Err(Error::new(
            403,
            format!("this needs the permission {permission}"),
        ));
    }
    Ok(staff)
}
"##;

const ROLES_AGENTS: &str = "\n\
    ## Roles\n\
    \n\
    Check a permission with `crate::plugins::roles::require(ctx, \"<name>\")`.\n\
    A role answers which people; it does not answer which rows. Put a row\n\
    rule in a `brocanth_policy::Policy`, and combine it with a permission\n\
    through `Staff::gate`. Never filter rows after a paginated load.\n";
