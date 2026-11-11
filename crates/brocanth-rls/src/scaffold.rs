//! `rls` -- what `brocanth create` writes when somebody asks for Postgres
//! row-level security.
//!
//! The module is here and not in the CLI because what it writes is a
//! statement about this crate: `Schema`, `migration::make`,
//! `catalog::drift`, and `Db::transact_as` with a `Principal`.
//!
//! What a project gets:
//!
//! - `src/plugins/rls/mod.rs`: `schema()`, the roles, tables and policies of
//!   the project, in Rust. The project owns it.
//! - `src/commands/rls.rs`: `rls make <name>` writes the migration and
//!   `migrations/rls.json`; `rls sql` prints the whole schema as SQL;
//!   `rls check` compares the database with the snapshot.
//! - A section in `AGENTS.md`: a request that reads tenant data runs in
//!   `transact_as`.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Slot};

/// Row-level security, as a choice on the `brocanth create` menu.
pub struct Rls;

impl Module for Rls {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "rls",
            "Postgres row-level security declared in Rust; `rls make` writes the migration",
        )
        .category(Category::Enhancer)
        .asked_in(
            "data",
            "Row-level security: Postgres hides the rows of other tenants",
        )
        .needs(
            "db",
            "the policies live in the database the `db` module opens, and `Db::transact_as` sets the principal",
        )
        .needs(
            "commands",
            "the migration is written by `rls make`, a command of the project's binary that runs `schema()`",
        )
        .adapts_to(
            "model",
            "`rls make` writes a SeaORM migration file, not a SQL file",
        )
        .adapts_to(
            "config",
            "`rls check` reads `[db] url` through the config loader, not from the environment alone",
        )
        .krate(
            Dep::brocanth("brocanth-rls")
                .note("roles, grants and policies in Rust, diffed into migrations"),
        )
        .after("db")
        .after("model")
        .absent(
            "brocanth-rls",
            "no row-level security; a query that forgets its WHERE shows every tenant's rows",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/rls/mod.rs", SCHEMA_RS.to_string());
        s.at(Slot::PluginsMod).line("pub mod rls;");
        s.file("src/commands/rls.rs", command_rs(project));
        s.at(Slot::CommandsMod).line("pub mod rls;");
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

fn command_rs(project: &Project) -> String {
    let format = if project.has("model") {
        "SeaOrm"
    } else {
        "Sql"
    };
    let url = if project.has("config") {
        URL_FROM_CONFIG
    } else {
        URL_FROM_ENV
    };
    let doc = if project.has("model") {
        "a SeaORM migration, because the project has `model`"
    } else {
        "a SQL file for `migrate up`"
    };
    let next = if project.has("model") {
        "List it with `migrate sync`, then apply it with `migrate up`."
    } else {
        "Apply it with `migrate up`."
    };
    COMMAND_RS
        .replace("@NEXT@", next)
        .replace("@FORMAT_DOC@", doc)
        .replace("@FORMAT@", format)
        .replace("@URL@", url)
}

/// `src/plugins/rls/mod.rs`.
const SCHEMA_RS: &str = r#"//! Row-level security: the roles, tables and policies of this database.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! Declare the rules here, then run `brocanth rls make <name>`. It compares
//! `schema()` with `migrations/rls.json` and writes a migration with the
//! difference. `brocanth migrate up` applies it. See `docs/rls.md` in
//! brocanth for the design.
//!
//! ```ignore
//! use brocanth_rls::{Policy, Privilege, Role, Setting, SqlExpr, SqlType, Table};
//!
//! let member = Role::new("app_member").member("app_login");
//! let tenant = Setting::new("tenant_id", SqlType::BigInt);
//! let posts = Table::new("posts").force().grant(&member, Privilege::CRUD);
//! let mine = SqlExpr::col("tenant_id").eq(tenant.get());
//! Schema::new()
//!     .role(member.clone())
//!     .table(posts.clone())
//!     .policy(Policy::new("posts_tenant", &posts).to(&member).using(mine.clone()).with_check(mine))
//! ```
//!
//! A request then runs as its principal, inside one transaction:
//!
//! ```ignore
//! let me = brocanth_db::Principal::new().role("app_member").set("tenant_id", tenant_id);
//! let posts: Vec<(i64, String)> = ctx.db().transact_as(&me, async |tx| tx.all(&q).await).await?;
//! ```

use brocanth_rls::Schema;

/// Every role, table, function and policy row-level security needs.
pub fn schema() -> Schema {
    Schema::new()
}
"#;

const COMMAND_RS: &str = r#"//! `rls` -- the row-level security of `src/plugins/rls/mod.rs`, as
//! migrations.
//!
//! `rls make <name>` compares `schema()` with `migrations/rls.json` and
//! writes the difference as a migration, with its `-- down` part. It
//! applies nothing: `migrate up` does. `rls sql` prints the whole schema as
//! SQL. `rls check` compares the database in `[db] url` with the snapshot.

use std::path::Path;

use brocanth_command::{anyhow, clap, cmd, Command, Context};
use brocanth_db::{Db, DbConfig};
use brocanth_rls::migration::{self, Format};

/// The migration file `rls make` writes: @FORMAT_DOC@.
const FORMAT: Format = Format::@FORMAT@;

/// What to run after `rls make`.
const NEXT: &str = "@NEXT@";

/// Write, print or check the row-level security migrations
#[cmd]
#[derive(clap::Args)]
pub struct Rls {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Write migrations/<UTC time>_<NAME> with what changed since migrations/rls.json
    Make {
        /// The name of the migration, in snake case: add_post_policies
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Print the SQL that creates the whole schema
    Sql,
    /// Compare the database in `[db] url` with migrations/rls.json
    Check,
}

impl Command for Rls {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        std::env::set_current_dir(ctx.root())?;
        let dir = Path::new("migrations");
        let schema = crate::plugins::rls::schema();
        match self.action {
            Action::Make { name } => match migration::make(&schema, dir, &name, FORMAT)? {
                None => println!("Nothing to write: migrations/rls.json matches schema()."),
                Some(made) => {
                    println!("wrote {}", made.path.display());
                    for step in made.steps {
                        println!("  {step}");
                    }
                    println!("{NEXT}");
                }
            },
            Action::Sql => print!("{}", schema.create_sql()?),
            Action::Check => {
                let snapshot = migration::read_snapshot(dir)?;
                let config = database()?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let drift = runtime.block_on(async {
                    let db = Db::connect(&config).await?;
                    let drift = brocanth_rls::catalog::drift(&db, &snapshot).await;
                    db.close().await;
                    anyhow::Ok(drift?)
                })?;
                if drift.is_empty() {
                    println!("The database matches migrations/rls.json.");
                } else {
                    for line in &drift {
                        println!("{line}");
                    }
                    anyhow::bail!(
                        "the database differs from migrations/rls.json\n\
                         help: run `migrate up`, or `rls make <name>` after a change to schema()"
                    );
                }
            }
        }
        Ok(())
    }
}
@URL@"#;

const URL_FROM_CONFIG: &str = r#"
/// The pool's configuration: `[db]`, through the loader the server uses.
fn database() -> anyhow::Result<DbConfig> {
    let config = brocanth_config::Loader::new(crate::app::APP).load()?;
    Ok(config
        .section::<crate::plugins::db::Settings>("db")?
        .config())
}
"#;

const URL_FROM_ENV: &str = r#"
/// The pool's configuration, as the server reads `[db]` without a file.
fn database() -> anyhow::Result<DbConfig> {
    let given = crate::plugins::given().map_err(anyhow::Error::msg)?;
    let settings = crate::plugins::db_settings(&given).map_err(anyhow::Error::msg)?;
    Ok(settings.config())
}
"#;

const AGENTS: &str = r#"
## Row-level security

`src/plugins/rls/mod.rs` declares the roles, tables and policies in Rust.
Do not write `CREATE POLICY` in a migration by hand. Change `schema()`,
then run `brocanth rls make <name>` and read the file it writes. Commit it
with `migrations/rls.json`.

A request that reads or writes a table with a policy runs in one
transaction, as its principal:

```rust
let me = brocanth_db::Principal::new().role("app_member").set("tenant_id", tenant_id);
ctx.db().transact_as(&me, async |tx| tx.all(&q).await).await?;
```

The role and the values end with the transaction, so they never reach the
next request on the same connection. A statement on `ctx.db()` outside
`transact_as` runs as the login role, and the policies see no value.
Never connect as a superuser: a superuser bypasses every policy.
"#;
