//! `model` -- SeaORM, validations and migrations, over the `db` pool.

use brocanth_scaffold::snippet::{plugin, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Slot};

/// The model layer.
///
/// Recommended and absent, which is section 8 point 4 word for word: an
/// application that stores nothing has no ORM in its lock file.
///
/// It opens no pool. ADR 2: the ORM is registered after the pool because it
/// needs the pool. So `model` needs `db`, and its plugin wraps the
/// `brocanth_db::Db` the `db` plugin published.
pub struct Model;

impl Module for Model {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "model",
            "SeaORM, validations, migrations, over the `db` pool",
        )
        .category(Category::Enhancer)
        .category(Category::Middleware)
        // `ctx.orm()` is an `impl Ctx` in `src/pages/mod.rs`, a slot the
        // renderers own.
        .category(Category::Renderer)
        .needs(
            "db",
            "the ORM runs over the pool the `db` plugin opens, and without `db` there is none",
        )
        .adapts_to(
            "config",
            "`migrate` reads `[db]` from the config file, not the default literal",
        )
        .adapts_to("commands", "a `migrate` subcommand is written")
        .asked_in("data", "An ORM with validations and migrations")
        // Postgres, the default driver: the pool it wraps is `brocanth-db`'s,
        // and that is Postgres. `db` builds `Orm` from a `Db`; `plugin`
        // makes `Orm` a context item.
        .krate(
            Dep::brocanth("brocanth-model")
                .feature("db")
                .feature("plugin")
                .note("`Orm` over the `db` pool, and on the request context"),
        )
        .absent(
            "brocanth-model",
            "no ORM, no sea-orm anywhere in the lock file",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/model/mod.rs", plugin_rs(project));
        // The list `brocanth migrate make` keeps, and the command that
        // applies it. Without a command line nothing could run the list,
        // so `src/main.rs` reaches it only when `commands` is here too.
        s.file(
            crate::owned::MIGRATOR,
            crate::commands::migrate::migrator_file(&[]),
        );
        if project.has("commands") {
            s.file("src/commands/migrate.rs", migrate_rs(project));
            s.at(Slot::CommandsMod).line("pub mod migrate;");
        }
        s.at(Slot::PluginsMod).line("pub mod model;");
        // No settings: the URL and the pool are `[db]`'s. The plugin is
        // still registered, so it can publish the ORM.
        plugin(s, project, "model", "Model");
        s.at(Slot::PagesItems).block(CTX_ORM.to_string());
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
    }
}

/// `ctx.orm()`, into `src/pages/mod.rs`, the way `db` writes `ctx.db()`.
const CTX_ORM: &str = r#"/// The ORM, on every request the application dispatches.
///
/// `allow(dead_code)`: it is here for the first page that reads a row, and
/// a new project has none.
#[allow(dead_code)]
impl Ctx {
    /// SeaORM's connection over the pool the `db` plugin opened: the same
    /// connections as `ctx.db()`.
    ///
    /// ```ignore
    /// let posts = post::Entity::find().all(ctx.orm()).await?;
    /// ```
    ///
    /// # Panics
    ///
    /// On a `Ctx` built by hand, which has no application behind it.
    pub fn orm(&self) -> &brocanth_model::sea_orm::DatabaseConnection {
        self.provided::<brocanth_model::Orm, _>()
            .expect(
                "this Ctx has no ORM: it was built by hand, not dispatched by the application\n\
                 help: send the request through `app::dispatch`, or give the code under test a \
                 `brocanth_model::Orm` of its own",
            )
            .connection()
    }
}
"#;

pub(crate) fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "model");
    // An empty struct has nothing to default by hand, so `Default` joins the
    // derive: one attribute, as rustfmt would merge two.
    let derive = derive.replacen("#[derive(Debug", "#[derive(Debug, Default", 1);
    format!(
        r##"//! The model layer.
//!
//! The ORM is recommended and absent. SeaORM is the recommendation, it is not a
//! dependency of the core and never will be, and it arrives through
//! `brocanth-model` -- which is in `Cargo.toml` because this file is here and
//! through no other route. An application that stores nothing pays nothing for
//! it: not a crate in the lock file, not a section in the config, not a byte.
//!
//! It opens no pool. The `db` plugin opens one and publishes it as
//! `brocanth_db::Db`; this plugin wraps that pool in SeaORM's connection, so an
//! entity query and a sea-query statement share connections, limits and
//! session settings. A page reads it with `ctx.orm()`.
//!
//! The entity modules go under `src/models/`, and you write them by hand: a
//! struct deriving `DeriveEntityModel` with `#[brocanth_model::validates(..)]`
//! above it, as `brocanth_model`'s own documentation sets out. There is no
//! `brocanth generate model` -- the CLI scaffolds a project and adds a module
//! to one, and a generator for a single entity is not written yet.

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ProvidesContext}};
use brocanth_model::{{ModelRegistry, Orm}};

/// What this plugin is built from: nothing. The URL and the pool are
/// `[db]`'s.
///
/// {note}
{derive}
pub struct Settings {{}}

pub struct Model {{
    /// Set in `before_start`, once the `db` plugin has opened the pool.
    orm: Option<Orm>,
}}

impl Plugin for Model {{
    type Config = Settings;
    type Provides = ProvidesContext<Orm>;
    const NAME: &'static str = "model";

    fn new(_config: Settings) -> Self {{
        Self {{ orm: None }}
    }}

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // The `db` plugin is registered first and opened the pool in its own
        // `before_start`. This wraps it and opens nothing.
        let orm = Orm::from_db(lc.require_service::<brocanth_db::Db>()?);
        // For another plugin: `lc.require_service::<Orm>()`.
        lc.services_mut().insert(orm.clone());
        self.orm = Some(orm);

        // The registry below is what would make a generator and an admin UI
        // tractable: every entity compiled into this binary can be enumerated,
        // with its columns, nullability and keys. It reads an inventory that
        // `DeriveEntityModel` fills, so it counts every model linked in,
        // whoever wrote it. The count is zero until the first entity module
        // exists under `src/models/`, and the line is here so that it is not
        // a surprise when it fills.
        let registry = ModelRegistry::new();
        let entities = registry.names().len();
        crate::plugins::say!(info, "[model] {{entities}} entities registered"; entities);
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // Nothing before `before_start`, so `ctx.orm()` answers an error
        // rather than a connection to nowhere.
        if let Some(orm) = &self.orm {{
            cx.insert(orm.clone());
        }}
    }}
}}
"##
    )
}

/// `src/commands/migrate.rs`: `up`, `down [N]` and `status`.
///
/// `brocanth migrate up` hands over to this command. It lives in the
/// project because the migrations are compiled into the project's binary,
/// and a container that has the binary and no `brocanth` can still run it.
fn migrate_rs(project: &Project) -> String {
    let url = if project.has("config") {
        URL_FROM_CONFIG
    } else {
        URL_FROM_LITERAL
    };
    MIGRATE_RS.replace("@URL@", url)
}

const URL_FROM_CONFIG: &str = r#"/// The pool's configuration: `[db]`, through the loader the server uses, so
/// the files, `$<APP>_ENV` and `$<APP>_DB__URL` agree with what the server
/// sees.
fn database() -> anyhow::Result<DbConfig> {
    let config = brocanth_config::Loader::new(crate::app::APP).load()?;
    Ok(config
        .section::<crate::plugins::db::Settings>("db")?
        .config())
}
"#;

const URL_FROM_LITERAL: &str = r#"/// The pool's configuration, as the server reads `[db]` without a file:
/// the defaults in `src/plugins/db/mod.rs`, then `$<APP>_DB__URL` and the
/// other variables, then the flags.
fn database() -> anyhow::Result<DbConfig> {
    let given = crate::plugins::given().map_err(anyhow::Error::msg)?;
    let settings = crate::plugins::db_settings(&given).map_err(anyhow::Error::msg)?;
    Ok(settings.config())
}
"#;

const MIGRATE_RS: &str = r#"//! `migrate` -- apply the migrations in `migrations/` to `[db] url`.
//!
//! `brocanth migrate up`, `brocanth migrate down [N]` and
//! `brocanth migrate status` run this command through this binary. Where
//! there is no `brocanth` -- a container, a release job -- run the binary
//! itself: `<binary> migrate up`.
//!
//! `brocanth migrate make` writes a migration and lists it in
//! `migrations/mod.rs`. This command applies what that list holds.

use brocanth_command::{anyhow, clap, cmd, Command, Context};
use brocanth_db::{Db, DbConfig};
use brocanth_model::sea_orm_migration::{MigrationStatus, MigratorTrait};
use brocanth_model::Orm;

use crate::migrations::Migrator;

/// Apply, roll back or list the migrations in migrations/
#[cmd]
#[derive(clap::Args)]
pub struct Migrate {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Apply each pending migration, oldest first
    Up,
    /// Roll back the newest N applied migrations
    Down {
        /// How many migrations to roll back
        #[arg(value_name = "N", default_value_t = 1)]
        n: u32,
    },
    /// List each migration as applied or pending
    Status,
}

impl Command for Migrate {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        // The config files resolve from the project root, wherever the
        // command started.
        std::env::set_current_dir(ctx.root())?;
        let config = database()?;
        tokio::runtime::Runtime::new()?.block_on(async {
            // The pool the server opens, and the ORM over it, as the
            // `model` plugin builds it.
            let pool = Db::connect(&config).await?;
            let orm = Orm::from_db(&pool);
            let db = orm.connection();
            let before = Migrator::get_migration_with_status(db).await?;
            let named = |status: MigrationStatus| -> Vec<String> {
                before
                    .iter()
                    .filter(|m| m.status() == status)
                    .map(|m| m.name().to_string())
                    .collect()
            };
            match self.action {
                Action::Up => {
                    let pending = named(MigrationStatus::Pending);
                    Migrator::up(db, None).await?;
                    if pending.is_empty() {
                        println!("Nothing to apply: each migration is applied.");
                    }
                    for name in pending {
                        println!("applied      {name}");
                    }
                }
                Action::Down { n } => {
                    let applied = named(MigrationStatus::Applied);
                    Migrator::down(db, Some(n)).await?;
                    if applied.is_empty() {
                        println!("Nothing to roll back: no migration is applied.");
                    }
                    for name in applied.iter().rev().take(n as usize) {
                        println!("rolled back  {name}");
                    }
                }
                Action::Status => {
                    if before.is_empty() {
                        println!("No migration in migrations/.");
                    }
                    for m in &before {
                        let status = match m.status() {
                            MigrationStatus::Applied => "applied",
                            MigrationStatus::Pending => "pending",
                        };
                        println!("{status:<12} {}", m.name());
                    }
                }
            }
            Ok(())
        })
    }
}

@URL@"#;
