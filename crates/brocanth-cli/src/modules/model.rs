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
        // The `migrate` tool, `src/bin/migrate.rs`: it reads `src/models/`
        // with a Rust parser and writes migrations. A dependency of the
        // package, because cargo has no per-binary dependencies, but only
        // the tool names it, so the server binary links none of it.
        .krate(
            Dep::brocanth("brocanth-migrate")
                .note("the `migrate` tool: `ids`, `make` and `sync`, in src/bin/migrate.rs"),
        )
        .absent(
            "brocanth-model",
            "no ORM, no sea-orm anywhere in the lock file",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/model/mod.rs", plugin_rs(project));
        // The list the `migrate` tool keeps, and the tool. The tool is a
        // second binary so the server never links a Rust parser: it reads
        // `src/models/` as text, writes source, and is for development.
        s.file(crate::owned::MIGRATOR, migrator_file());
        s.file("src/bin/migrate.rs", MIGRATE_BIN);
        s.binary("migrate", "src/bin/migrate.rs");
        // The command that applies the list, and runs the tool for `ids`,
        // `make` and `sync`. Without a command line the tool still runs:
        // `cargo run --bin migrate -- make <name>`.
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

/// `migrations/mod.rs` as `create` writes it: stamped, and listing nothing.
///
/// The `migrate` tool (`brocanth_migrate::migrator`) writes every later
/// list, and must write these same bytes for an empty one, or it would read
/// this file as edited. `crates/brocanth-migrate/tests/golden/migrator-empty.rs`
/// pins them, and a test on each side compares with it.
pub(crate) fn migrator_file() -> String {
    crate::owned::stamp(crate::owned::MIGRATOR, EMPTY_MIGRATOR)
}

const EMPTY_MIGRATOR: &str = "//! The migrations in this directory, oldest first.
//!
//! A file name starts with its UTC time, so the order of the names is the
//! order the migrations were written. `src/commands/migrate.rs` applies this
//! list, and `src/main.rs` reaches this file through `#[path]`.

use brocanth_model::sea_orm_migration::prelude::*;

/// Every migration this binary applies, oldest first.
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![]
    }
}
";

/// `src/bin/migrate.rs`: the `migrate` tool, a development binary.
const MIGRATE_BIN: &str = r#"//! The `migrate` tool: write migrations from the models in `src/models/`.
//!
//! ```text
//! cargo run --bin migrate -- ids              # an id on each model and field
//! cargo run --bin migrate -- make add_posts   # a migration for what changed
//! cargo run --bin migrate -- sync             # list every file in migrations/mod.rs
//! ```
//!
//! A development tool, and a binary of its own: it parses Rust source, and
//! the server has no reason to link a parser. It applies nothing. Apply
//! with the application's own `migrate up`.

fn main() -> std::process::ExitCode {
    brocanth_migrate::tool::main()
}
"#;

/// `src/commands/migrate.rs`: `up`, `down [N]` and `status`, plus `ids`,
/// `make` and `sync`, which run the `migrate` tool.
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
//! `up`, `down [N]` and `status` run in this binary, because the
//! migrations are compiled into it. Where there is no `brocanth` -- a
//! container, a release job -- run the binary itself: `<binary> migrate up`.
//! `brocanth migrate up` hands over to this command too.
//!
//! `ids`, `make <NAME>` and `sync` write source, so they run the `migrate`
//! tool, `src/bin/migrate.rs`, through cargo. The tool is a binary of its
//! own so this one links no Rust parser.

use std::collections::BTreeSet;

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
    /// Write an id on each model and field in src/models that has none
    Ids,
    /// Write a migration for what changed in src/models since the last one
    Make {
        /// The name of the migration, in snake case: add_posts, rename_title
        #[arg(value_name = "NAME")]
        name: String,
        /// Write the snapshot and no migration
        #[arg(long)]
        snapshot_only: bool,
    },
    /// List every migrations/m*.rs file in migrations/mod.rs
    Sync,
}

impl Command for Migrate {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        // The config files resolve from the project root, wherever the
        // command started.
        std::env::set_current_dir(ctx.root())?;
        match self.action {
            Action::Ids => tool(&["ids"]),
            Action::Sync => tool(&["sync"]),
            Action::Make {
                name,
                snapshot_only,
            } => {
                let mut args = vec!["make", name.as_str()];
                if snapshot_only {
                    args.push("--snapshot-only");
                }
                tool(&args)
            }
            Action::Up => apply(Apply::Up),
            Action::Down { n } => apply(Apply::Down(n)),
            Action::Status => apply(Apply::Status),
        }
    }
}

/// What `up`, `down` and `status` do to the database.
enum Apply {
    Up,
    Down(u32),
    Status,
}

fn apply(what: Apply) -> anyhow::Result<()> {
    unlisted()?;
    let config = database()?;
    tokio::runtime::Runtime::new()?.block_on(async {
        // The pool the server opens, and the ORM over it, as the `model`
        // plugin builds it.
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
        match what {
            Apply::Up => {
                let pending = named(MigrationStatus::Pending);
                Migrator::up(db, None).await?;
                if pending.is_empty() {
                    println!("Nothing to apply: each migration is applied.");
                }
                for name in pending {
                    println!("applied      {name}");
                }
            }
            Apply::Down(n) => {
                let applied = named(MigrationStatus::Applied);
                Migrator::down(db, Some(n)).await?;
                if applied.is_empty() {
                    println!("Nothing to roll back: no migration is applied.");
                }
                for name in applied.iter().rev().take(n as usize) {
                    println!("rolled back  {name}");
                }
            }
            Apply::Status => {
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

/// Runs the `migrate` tool with these arguments, from the project root.
fn tool(args: &[&str]) -> anyhow::Result<()> {
    let status = std::process::Command::new("cargo")
        .args(["run", "--quiet", "--bin", "migrate", "--"])
        .args(args)
        .status()
        .map_err(|e| {
            anyhow::anyhow!(
                "cannot run cargo: {e}\n\
                 help: run `cargo run --bin migrate -- {}` where cargo is installed",
                args.join(" ")
            )
        })?;
    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Refuses when a file in `migrations/` is not in the list this binary was
/// built with: it would be skipped without a word. A `.sql` file is the
/// same case: the SQL migrations of a project with `db` and no `model`.
fn unlisted() -> anyhow::Result<()> {
    let mut sql: Vec<String> = std::fs::read_dir("migrations")
        .map(|dir| {
            dir.flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| name.ends_with(".sql"))
                .collect()
        })
        .unwrap_or_default();
    sql.sort();
    if !sql.is_empty() {
        anyhow::bail!(
            "migrations/ holds SQL migrations ({}), and this command applies only the \
             migrations of `model`\n\
             help: the SQL files made tables that are already there. Record them with \
             `migrate make <name> --snapshot-only`, then move the .sql files out of \
             migrations/, for example to migrations/sql/",
            sql.join(", ")
        );
    }
    let built: BTreeSet<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let mut missing: Vec<String> = std::fs::read_dir("migrations")
        .map(|dir| {
            dir.flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let stem = name.strip_suffix(".rs")?.to_string();
                    (stem.starts_with('m') && stem != "mod" && !built.contains(&stem))
                        .then_some(stem)
                })
                .collect()
        })
        .unwrap_or_default();
    missing.sort();
    if missing.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "this binary does not contain {}: migrations/mod.rs does not list it\n\
         help: run `migrate sync` (or `cargo run --bin migrate -- sync`), then run this \
         command again",
        missing.join(", ")
    )
}

@URL@"#;

#[cfg(test)]
mod tests {
    /// The `migrate` tool reads a list as the tool's own only while its
    /// digest matches what the tool would write. So the empty list `create`
    /// writes must be, byte for byte, the empty list the tool writes. The
    /// tool's side compares with the same file.
    #[test]
    fn the_empty_list_is_the_one_the_migrate_tool_writes() {
        assert_eq!(
            super::migrator_file(),
            include_str!("../../../brocanth-migrate/tests/golden/migrator-empty.rs")
        );
    }
}
