//! `model` -- SeaORM, validations and migrations.

use brocanth_scaffold::snippet::{plugin, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// The model layer.
///
/// Recommended and absent, which is section 8 point 4 word for word: an
/// application that stores nothing has no SQL driver in its lock file.
pub struct Model;

impl Module for Model {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "model",
            "SeaORM, validations, migrations; without it no SQL driver at all",
        )
        .asked_in("data", "An ORM with validations and migrations")
        // Both drivers: the development database is a SQLite file, and the
        // default features are Postgres, which production uses.
        .krate(
            Dep::brocanth("brocanth-model")
                .feature("sqlite")
                .note("Postgres by default; SQLite for config/development.toml"),
        )
        .absent(
            "brocanth-model",
            "no ORM, no SQL driver, no sea-orm anywhere in the lock file",
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
        plugin(s, project, "model", "Model");
        s.at(Slot::AppContext).block(
            "    let db: &crate::plugins::model::Db = cx.extract();\n\
            \x20   eprintln!(\"database: {}\", db.url);"
                .to_string(),
        );
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("config") {
            s.at(Slot::ConfigSections).block(
                "\n[model]\nurl = \"sqlite://db/development.sqlite?mode=rwc\"\n".to_string(),
            );
        }
    }
}

pub(crate) fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "model");
    format!(
        r##"//! The model layer.
//!
//! The ORM is recommended and absent. SeaORM is the recommendation, it is not a
//! dependency of the core and never will be, and it arrives through
//! `brocanth-model` -- which is in `Cargo.toml` because this file is here and
//! through no other route. An application that stores nothing pays nothing for
//! it: not a crate in the lock file, not a section in the config, not a byte.
//!
//! The entity modules go under `src/models/`, and you write them by hand: a
//! struct deriving `DeriveEntityModel` with `#[brocanth_model::validates(..)]`
//! above it, as `brocanth_model`'s own documentation sets out. There is no
//! `brocanth generate model` -- the CLI scaffolds a project and adds a module
//! to one, and a generator for a single entity is not written yet.

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth_model::ModelRegistry;

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// `sqlite://db/development.sqlite?mode=rwc`, or a Postgres URL.
    pub url: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            url: "sqlite://db/development.sqlite?mode=rwc".to_string(),
        }}
    }}
}}

/// The handle a handler asks the context for.
pub struct Db {{
    pub url: String,
}}

impl ContextItem for Db {{}}

pub struct Model {{
    url: String,
}}

impl Plugin for Model {{
    type Config = Settings;
    type Provides = ProvidesContext<Db>;
    const NAME: &'static str = "model";

    fn new(config: Settings) -> Self {{
        Self {{ url: config.url }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.url.is_empty() {{
            return Err(PluginError::new(
                "[model] url is required; there is no default database",
            ));
        }}
        Ok(())
    }}

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // The pool opens here, at the last hook before the server is handed the
        // application, and drains on the matching shutdown hook.
        //
        // The registry below is what would make a generator and an admin UI
        // tractable: every entity compiled into this binary can be enumerated,
        // with its columns, nullability and keys. It reads an inventory that
        // `DeriveEntityModel` fills, so it counts every model linked in,
        // whoever wrote it. The count is zero until the first entity module
        // exists under `src/models/`, and the line is here so that it is not
        // a surprise when it fills.
        let registry = ModelRegistry::new();
        eprintln!("[model] {{}} entities registered", registry.names().len());
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        cx.insert(Db {{
            url: self.url.clone(),
        }});
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

const URL_FROM_CONFIG: &str = r#"/// `[model] url`, through the loader the server uses, so the files,
/// `$<APP>_ENV` and `$<APP>_MODEL__URL` agree with what the server sees.
fn database_url() -> anyhow::Result<String> {
    let config = brocanth_config::Loader::new(crate::app::APP).load()?;
    Ok(config
        .section::<crate::plugins::model::Settings>("model")?
        .url)
}
"#;

const URL_FROM_LITERAL: &str = r#"/// The URL the model plugin is built with. This project has no config
/// file, so it is the default in `src/plugins/model/mod.rs`.
fn database_url() -> anyhow::Result<String> {
    Ok(crate::plugins::model::Settings::default().url)
}
"#;

const MIGRATE_RS: &str = r#"//! `migrate` -- apply the migrations in `migrations/` to `[model] url`.
//!
//! `brocanth migrate up`, `brocanth migrate down [N]` and
//! `brocanth migrate status` run this command through this binary. Where
//! there is no `brocanth` -- a container, a release job -- run the binary
//! itself: `<binary> migrate up`.
//!
//! `brocanth migrate make` writes a migration and lists it in
//! `migrations/mod.rs`. This command applies what that list holds.

use brocanth_command::{anyhow, clap, cmd, Command, Context};
use brocanth_model::sea_orm::Database;
use brocanth_model::sea_orm_migration::{MigrationStatus, MigratorTrait};

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
        // The config files and a relative SQLite path both resolve from the
        // project root, wherever the command started.
        std::env::set_current_dir(ctx.root())?;
        let url = database_url()?;
        sqlite_directory(&url)?;
        tokio::runtime::Runtime::new()?.block_on(async {
            let db = Database::connect(url.as_str()).await?;
            let before = Migrator::get_migration_with_status(&db).await?;
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
                    Migrator::up(&db, None).await?;
                    if pending.is_empty() {
                        println!("Nothing to apply: each migration is applied.");
                    }
                    for name in pending {
                        println!("applied      {name}");
                    }
                }
                Action::Down { n } => {
                    let applied = named(MigrationStatus::Applied);
                    Migrator::down(&db, Some(n)).await?;
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

@URL@
/// Makes the directory of a SQLite file. `mode=rwc` creates the file, and
/// not the directory it is in.
fn sqlite_directory(url: &str) -> anyhow::Result<()> {
    let Some(rest) = url.strip_prefix("sqlite://") else {
        return Ok(());
    };
    let path = rest.split('?').next().unwrap_or_default();
    if path.is_empty() || path == ":memory:" {
        return Ok(());
    }
    if let Some(dir) = std::path::Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}
"#;
