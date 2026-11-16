//! `db` -- what `brocanth create` writes when somebody asks for a database.
//!
//! The module is here and not in the CLI because what it writes is a
//! statement about this crate: `Db::connect_lazy`, the fields of `DbConfig`,
//! `Db::ping` for readiness, `Db::close` for shutdown, and `Db` as a context
//! item. Those move when this crate moves.
//!
//! What a project gets:
//!
//! - `src/plugins/db/mod.rs`: a plugin that builds the pool in `before_start`
//!   with no connection, registers `Db::ping` as its readiness check (which
//!   `serve` awaits once before it listens), publishes the `Db` in the
//!   application's services for a later plugin
//!   (`lc.require_service::<brocanth_db::Db>()`), puts it on every request's
//!   context, and closes it on shutdown.
//! - `ctx.db()` on the generated `Ctx`, which returns `&brocanth_db::Db`.
//! - `[db]` in the config file: the URL and the pool and session settings.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{configurable, redact_secrets, settings_derive};
use brocanth_scaffold::{
    AgentsSection, Category, ConfigSource, Contribution, Dep, Manifest, Module, PgPool, Project,
    Scaffold, Setting, Slot,
};

pub use crate::scaffold_embedded::DbEmbedded;

/// The database, as a choice on the `brocanth create` menu.
///
/// Named `Database` and not `Db`, which is this crate's own type: a second
/// item under that name makes rustc print full paths in every error that
/// names either.
pub struct Database;

impl Module for Database {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "db",
            "one Postgres pool and sea-query statements, read as ctx.db(); without it no SQL driver",
        )
        .contributes::<AgentsSection>(Contribution::new("db", "how a page reads the pool, and how a migration is written"), true)
        .category(Category::Enhancer)
        // The pool goes on the request context.
        .category(Category::Middleware)
        // `ctx.db()` is a function every page calls.
        .category(Category::Renderer)
        // The one shared pool. Every Postgres module requires it and opens no
        // pool of its own, so `brocanth create --jobs` adds this module.
        .provides::<PgPool>("the project's one Postgres pool, published as `brocanth_db::Db`")
        .uses::<ConfigSource>(
            "the development URL is in the development file, so the default is empty and each environment names its own",
        )
        .adapts_to(
            "commands",
            "a `dbshell` command opens psql on `[db] url`, and a `migrate` command applies the SQL files in migrations/",
        )
        .adapts_to(
            "model",
            "no SQL `migrate` command: `model` writes its own, over SeaORM",
        )
        .adapts_to(
            "db-embedded",
            "`configure` opens the pool on the embedded server's URL, not on `[db] url`",
        )
        .asked_in("data", "A Postgres pool, and SQL built with sea-query")
        .recommends(
            "observability-health",
            "its readiness check, `Db::ping`, is read only by /readyz; without it a deploy sends traffic to an instance whose database is down",
        )
        .krate(
            Dep::brocanth("brocanth-db")
                .feature("plugin")
                .note("the pool, read as `ctx.db()`"),
        )
        // Its child registers first: the server runs before the pool reads
        // its URL, and stops after the pool closes (shutdown runs in reverse).
        .after("db-embedded")
        .absent(
            "brocanth-db",
            "no database pool; a page that needs SQL has nowhere to send it",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file(
            "src/plugins/db/mod.rs",
            redact_secrets(&plugin_rs(project), &settings(project)),
        );
        s.at(Slot::PluginsMod).line("pub mod db;");
        configurable(s, project, "db", "Database", settings(project));
        if project.has("commands") {
            s.file("src/commands/dbshell.rs", DBSHELL_RS);
            s.at(Slot::CommandsMod).line("pub mod dbshell;");
        }
        // Plain SQL migrations, for a project with no ORM. With `model`,
        // the `migrate` command is the model's: one per project.
        if project.has("commands") && !project.has("model") {
            s.file("src/commands/migrate.rs", MIGRATE_RS);
            s.at(Slot::CommandsMod).line("pub mod migrate;");
            s.at(Slot::AgentsSections).block(AGENTS_MIGRATE.to_string());
        }
        // Load-bearing: the provider list is in the application's type, so
        // this stops compiling the moment the registration is deleted.
        s.at(Slot::AppContext).block(
            "    let db: &brocanth_db::Db = cx.extract();\n\
            \x20   let connections = db.pool().size();\n\
            \x20   crate::plugins::say!(info, \"db: {connections} connection(s) open\"; connections);"
                .to_string(),
        );
        s.at(Slot::PagesItems).block(CTX_DB.to_string());
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The `[db]` section.
fn settings(project: &Project) -> Vec<Setting> {
    vec![
        Setting::text(
            "url",
            "postgres://localhost/development",
            "The Postgres URL: postgres://user:password@host:5432/database.",
        )
        .note("A URL can carry a password, so no flag sets it.")
        .secret(),
        Setting::integer(
            "max_connections",
            10,
            "The most connections the pool opens.",
        )
        .note(
            "A statement holds a connection only while it runs, so this bounds\n\
             concurrent statements, not concurrent requests.",
        ),
        Setting::integer(
            "min_connections",
            0,
            "The connections the pool keeps open when idle.",
        ),
        Setting::integer(
            "acquire_timeout",
            30,
            "Seconds a statement waits for a connection, opening one included.",
        ),
        Setting::integer(
            "idle_timeout",
            600,
            "Seconds before an idle connection closes. 0 keeps it open.",
        ),
        Setting::integer(
            "statement_timeout",
            30,
            "Seconds before the server cancels a statement. 0 is no limit.",
        )
        .note("`migrate` ignores it: a migration runs with no statement limit."),
        Setting::integer(
            "lock_timeout",
            10,
            "Seconds a migration statement waits for a table lock. 0 waits forever.",
        )
        .note("Only `migrate` sets it: a busy table fails the run, not every query behind it."),
        Setting::list(
            "search_path",
            &[],
            "The schemas for a table name with no schema. Empty is the server's default.",
        ),
        Setting::bool(
            "read_only",
            false,
            "Every transaction is read-only: for a replica, or a reporting process.",
        ),
        Setting::text(
            "application_name",
            project.name,
            "The name pg_stat_activity shows. Empty sends none.",
        ),
        Setting::text(
            "ca_file",
            "/etc/ssl/certs/db-ca.pem",
            "A PEM bundle to trust, with verify-full.",
        )
        .note(
            "Unset, the URL's sslmode applies. The default, prefer, verifies no\n\
             certificate and can fall back to plain text: for a remote server, set\n\
             this or sslmode=verify-full in the URL.",
        )
        .optional(),
    ]
}

/// `ctx.db()`, in `src/pages/mod.rs`.
const CTX_DB: &str = r#"/// The database, on every request the application dispatches.
///
/// `allow(dead_code)`: it is here for the first page that queries, and a
/// new project has none.
#[allow(dead_code)]
impl Ctx {
    /// The pool the `db` plugin opened. Each statement takes a connection
    /// and gives it back, so a page holds none while it waits on anything
    /// else.
    ///
    /// ```ignore
    /// use brocanth_db::Exec;
    /// let rows: Vec<(i64, String)> = ctx.db().all(&query).await?;
    /// ```
    ///
    /// A `brocanth_db::DbError` goes through `?`: it is a 500, its message
    /// goes to the log and not to the client.
    ///
    /// # Panics
    ///
    /// On a `Ctx` built by hand, which has no application behind it.
    pub fn db(&self) -> &brocanth_db::Db {
        self.provided::<brocanth_db::Db, _>().expect(
            "this Ctx has no database: it was built by hand, not dispatched by the application\n\
             help: send the request through `app::dispatch`, or give the code under test a \
             `brocanth_db::Db` of its own",
        )
    }
}
"#;

/// `src/commands/migrate.rs`: `brocanth_db::migrate` on the database the
/// server reads.
const MIGRATE_RS: &str = r#"//! `migrate` -- apply the SQL files in `migrations/` to `[db] url`.
//!
//! A migration is one file, `migrations/<UTC time>_<name>.sql`. The
//! statements before a `-- down` line apply it; the statements after that
//! line undo it. `migrate new <name>` writes an empty one. Each file runs in
//! one transaction, and `brocanth_migrations` records it.
//!
//! The files are read when this command runs, not built into the binary.
//! The container image copies `migrations/` next to the binary, so
//! `<binary> migrate up` works in a release job. See `brocanth_db::migrate`.

use std::path::Path;

use brocanth_command::{anyhow, clap, cmd, Command, Context};
use brocanth_db::migrate::{self, Migrator};
use brocanth_db::{Db, DbConfig};

/// Apply, roll back or list the SQL migrations in migrations/
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
    /// Roll back the last N applied migrations, the last applied first
    Down {
        /// How many migrations to roll back
        #[arg(value_name = "N", default_value_t = 1)]
        n: usize,
    },
    /// List each migration as applied or pending
    Status,
    /// Write an empty migration, migrations/<UTC time>_<NAME>.sql
    New {
        /// The name of the migration, in snake case: add_posts
        #[arg(value_name = "NAME")]
        name: String,
    },
}

impl Command for Migrate {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        // `migrations/` and the config files resolve from the project
        // root, wherever the command started.
        std::env::set_current_dir(ctx.root())?;
        let dir = Path::new("migrations");
        if let Action::New { name } = &self.action {
            let path = migrate::new_file(dir, name)?;
            println!("wrote {}", path.display());
            return Ok(());
        }
        // With no directory, `up` would apply nothing and say all is
        // applied: in a release image that forgot to copy it, a lie.
        if matches!(self.action, Action::Up | Action::Down { .. }) && !dir.is_dir() {
            anyhow::bail!(
                "{} has no migrations/ directory, so there is nothing to apply or roll back\n\
                 help: run this command from the project root, or copy migrations/ next to \
                 the binary in the image",
                ctx.root().display()
            );
        }
        let files = migrate::read_dir(dir)?;
        let config = database()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let db = Db::connect(&config).await?;
            let migrator = Migrator::new(&db, files).lock_timeout(config.lock_timeout);
            let result = apply(&migrator, self.action).await;
            db.close().await;
            result
        })
    }
}

async fn apply(migrator: &Migrator<'_>, action: Action) -> anyhow::Result<()> {
    match action {
        Action::Up => {
            let late = migrate::out_of_order(&migrator.status().await?);
            if !late.is_empty() {
                eprintln!(
                    "warning: {} sorts before the newest applied migration, and is applied \
                     after it. `migrate down` undoes it first.",
                    late.join(", ")
                );
            }
            let applied = migrator.up().await?;
            if applied.is_empty() {
                println!("Nothing to apply: each migration is applied.");
            }
            for name in applied {
                println!("applied      {name}");
            }
        }
        Action::Down { n } => {
            let undone = migrator.down(n).await?;
            if undone.is_empty() {
                println!("Nothing to roll back: no migration is applied.");
            }
            for name in undone {
                println!("rolled back  {name}");
            }
        }
        Action::Status => {
            let states = migrator.status().await?;
            if states.is_empty() {
                println!("No migration in migrations/. Write one with `migrate new <name>`.");
            }
            for (name, state) in states {
                println!("{:<12} {name}", state.as_str());
            }
        }
        Action::New { .. } => {}
    }
    Ok(())
}

/// The pool's configuration: `[db]`, as the server reads it, through every
/// settings layer: the default, the module file, the files, the
/// environment and the flags.
fn database() -> anyhow::Result<DbConfig> {
    let layers = crate::plugins::layers().map_err(anyhow::Error::msg)?;
    let settings = crate::plugins::db_settings(&layers).map_err(anyhow::Error::msg)?;
    Ok(settings.config())
}
"#;

const AGENTS_MIGRATE: &str = r#"
## Migrations

A schema change is a SQL file in `migrations/`. Write one with
`brocanth migrate new add_posts` (or `cargo run -- migrate new add_posts`),
then fill it:

```sql
CREATE TABLE posts (id bigserial PRIMARY KEY, title text NOT NULL);

-- down
DROP TABLE posts;
```

`migrate up` applies each pending file in name order, each in one
transaction. `migrate down [N]` runs the `-- down` part of the last N
applied, the last applied first.
`migrate status` lists each file. Do not edit a file that is applied: write
a new one.
"#;

/// `src/commands/dbshell.rs`: `brocanth_db::shell` on the URL the server reads.
///
/// A command of the project and not of the `brocanth` CLI: only the
/// project knows how its configuration is read, and the CLI links no
/// configuration loader.
const DBSHELL_RS: &str = r#"//! `dbshell` -- the database's own client, on this application's database.
//!
//! Reads `[db] url` the way the server does and replaces itself with `psql`
//! on it. `--print` shows the command line; a password goes in
//! `PGPASSWORD` and never on it. See `brocanth_db::shell`.
//!
//! Ordinary project code, like `serve`: rename it or delete it, and delete
//! its line in `src/commands/mod.rs`.

use std::ffi::OsString;

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Open psql on the database in `[db] url`
#[cmd]
#[derive(clap::Args)]
pub struct Dbshell {
    /// Print the command line instead of running it. The password is not printed
    #[arg(long)]
    print: bool,
    /// Passed to psql unchanged, after `--`
    #[arg(last = true, value_name = "CLIENT ARGS")]
    rest: Vec<OsString>,
}

impl Command for Dbshell {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        // The config files resolve from the project root.
        std::env::set_current_dir(ctx.root())?;
        let layers = crate::plugins::layers().map_err(anyhow::Error::msg)?;
        let url = crate::plugins::db_settings(&layers)
            .map_err(anyhow::Error::msg)?
            .url;
        brocanth_db::shell::run(&url, ctx.root(), self.print, &self.rest)?;
        Ok(())
    }
}
"#;

const AGENTS: &str = r#"
## The database

`src/plugins/db/mod.rs` opens one Postgres pool at startup from `[db] url`.
A page reads it with `ctx.db()`, a `&brocanth_db::Db`. Build a statement
with `brocanth_db::sea_query`. Do not format values into SQL text: sea-query
sends each value as a bound parameter.

```rust
use brocanth_db::sea_query::{Expr, ExprTrait, Order, Query};
use brocanth_db::Exec;

use crate::pages::{Ctx, Error, IntoResponse};

pub async fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let q = Query::select()
        .columns(["id", "title"])
        .from("posts")
        .and_where(Expr::col("published").eq(true))
        .order_by("id", Order::Desc)
        .limit(10)
        .to_owned();
    let posts: Vec<(i64, String)> = ctx.db().all(&q).await?;
    Ok(posts
        .iter()
        .map(|(id, title)| format!("{id} {title}\n"))
        .collect::<String>())
}
```

`ctx.db().json(&q).await?` returns each row as a `serde_json::Value`,
encoded by Postgres. Writes that belong together go in one transaction.
`Ok` commits, and `Err` rolls back:

```rust
ctx.db()
    .transact(async |tx| {
        tx.execute(&debit).await?;
        tx.execute(&credit).await?;
        Ok(())
    })
    .await?;
```

A `brocanth_db::DbError` goes through `?` as a 500. Its message goes to the
log, never to the client.
"#;

/// The top of `configure` with `db-embedded`: its plugin, registered
/// before this one, started the server in its own `configure`.
const EMBEDDED_URL: &str =
    "        // `db-embedded` started a Postgres server in its own `configure`,
        // registered before this plugin: the pool opens on it, and `[db] url`
        // is not read. `brocanth remove db-embedded` goes back to the URL.
        if let Some(embedded) = lc.services().get::<brocanth_db::embedded::EmbeddedUrl>() {
            self.config.url = embedded.0.clone();
        }
";

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "db");
    let name = project.name;
    // With a config file, `config/development.toml` holds the development
    // URL, and any other environment must name its own: an empty default
    // makes `configure` say so. Without one, the literal is the only source,
    // and it is for development.
    let (url, url_doc) = if project.provider::<ConfigSource>().is_some() {
        (
            "",
            "Empty by default: each\n    \
             /// environment names its database, and `configure` refuses an empty one.",
        )
    } else {
        (
            "postgres://localhost/development",
            "The default is a\n    \
             /// local development database; set the variable for `db.url` everywhere else.",
        )
    };
    let (lc, embedded) = if project.has("db-embedded") {
        ("lc", EMBEDDED_URL)
    } else {
        ("_lc", "")
    };
    format!(
        r##"//! The database, as this application registers it.
//!
//! One Postgres pool, built at startup and shared by every request. It opens
//! no connection in the build; `serve` checks it once before it listens. A page
//! reads it with `ctx.db()`. Another plugin reads it from the application's
//! services: `lc.require_service::<brocanth_db::Db>()`, from its own
//! `before_start`, registered after this one.
//!
//! `brocanth-db` is in `Cargo.toml` because this file is here, and through no
//! other route. Delete the registration in `src/plugins/mod.rs` and this
//! file, and the application links no SQL driver.

use std::time::Duration;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ProvidesContext}};
use brocanth::plugin_shutdown::ShutdownCx;
use brocanth_db::{{Db, DbConfig, DbError}};

/// What this plugin is built from: `[db]`.
///
/// A duration is a whole number of seconds, and 0 turns an optional one
/// off.
///
/// {note}
{derive}
pub struct Settings {{
    /// `postgres://user:password@host:5432/database`. {url_doc}
    pub url: String,
    /// The most connections the pool opens.
    pub max_connections: u32,
    /// The connections the pool keeps open when idle.
    pub min_connections: u32,
    /// Seconds a statement waits for a connection, opening one included.
    pub acquire_timeout: u64,
    /// Seconds before an idle connection closes. 0 keeps it open.
    pub idle_timeout: u64,
    /// Seconds before the server cancels a statement. 0 is no limit.
    pub statement_timeout: u64,
    /// Seconds a migration statement waits for a table lock. 0 waits
    /// forever. Only `migrate` sets it.
    pub lock_timeout: u64,
    /// The schemas for a table name with no schema. Empty is the server's
    /// default.
    pub search_path: Vec<String>,
    /// Every transaction is read-only.
    pub read_only: bool,
    /// The name `pg_stat_activity` shows. Empty sends none.
    pub application_name: String,
    /// A PEM bundle to trust, with `verify-full`. `None` keeps the URL's
    /// `sslmode`: the default, `prefer`, verifies no certificate and can
    /// fall back to plain text.
    pub ca_file: Option<String>,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            url: {url:?}.to_string(),
            max_connections: 10,
            min_connections: 0,
            acquire_timeout: 30,
            idle_timeout: 600,
            statement_timeout: 30,
            lock_timeout: 10,
            search_path: Vec::new(),
            read_only: false,
            application_name: {name:?}.to_string(),
            ca_file: None,
        }}
    }}
}}

impl Settings {{
    /// The pool's configuration, from these settings.
    pub fn config(&self) -> DbConfig {{
        let seconds = |secs: u64| (secs > 0).then(|| Duration::from_secs(secs));
        let mut config = DbConfig::new(&self.url);
        config.max_connections = self.max_connections;
        config.min_connections = self.min_connections;
        config.acquire_timeout = Duration::from_secs(self.acquire_timeout);
        config.idle_timeout = seconds(self.idle_timeout);
        config.statement_timeout = seconds(self.statement_timeout);
        config.lock_timeout = seconds(self.lock_timeout);
        config.search_path = self.search_path.clone();
        config.read_only = self.read_only;
        config.application_name =
            Some(self.application_name.clone()).filter(|name| !name.is_empty());
        config.ca_file = self.ca_file.clone().map(Into::into);
        config
    }}
}}

pub struct Database {{
    config: DbConfig,
    /// Open from `before_start` until `shutdown`.
    db: Option<Db>,
}}

impl Plugin for Database {{
    type Config = Settings;
    type Provides = ProvidesContext<Db>;
    const NAME: &'static str = "db";

    fn new(settings: Settings) -> Self {{
        Self {{
            config: settings.config(),
            db: None,
        }}
    }}

    fn configure(&mut self, {lc}: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
{embedded}        if self.config.url.is_empty() {{
            return Err(PluginError::new(
                "[db] url is required; there is no default database\n\
                 help: set `url` under [db] in the config file, or the variable for `db.url`",
            ));
        }}
        Ok(())
    }}

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // The hook is synchronous and connecting is not, so the pool opens
        // lazily and nothing here waits. Waiting would hang a current-thread
        // runtime, where this thread is the one that drives the I/O. The
        // pool spawns its upkeep task, so it needs the runtime entered.
        let runtime = tokio::runtime::Handle::try_current().map_err(|e| {{
            PluginError::new(format!(
                "[db] needs a Tokio runtime at startup to open its pool ({{e}})\n\
                 help: build the application inside the runtime that serves it"
            ))
        }})?;
        let db = {{
            let _entered = runtime.enter();
            Db::connect_lazy(&self.config).map_err(|e| PluginError::new(format!("[db] {{e}}")))?
        }};
        // Ready while `SELECT 1` answers on a pooled connection. `serve`
        // awaits this once before it listens, so a wrong URL, password or
        // certificate stops the start there and not on the first request.
        let probe = db.clone();
        let url = self.config.redacted_url();
        lc.ready_when(move || {{
            let probe = probe.clone();
            let url = url.clone();
            async move {{
                probe.ping().await.map_err(|e| match e {{
                    DbError::Connect {{ .. }} => format!("connecting to {{url}}: {{e}}"),
                    other => other.to_string(),
                }})
            }}
        }});
        // For a plugin registered after this one: the model layer wraps the
        // same pool, so there is one set of connections to size.
        lc.services_mut().insert(db.clone());
        self.db = Some(db);
        Ok(())
    }}

    fn shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {{
        let Some(db) = self.db.take() else {{
            return Ok(());
        }};
        // Without a runtime there is nothing to wait on: the connections
        // close when the pool drops.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {{
            return Ok(());
        }};
        let remaining = sd.remaining();
        // `close` waits for the connections in use to come back, so it
        // waits inside what is left of the grace period and no longer.
        let close = async move {{
            match remaining {{
                Some(left) => tokio::time::timeout(left, db.close()).await.is_ok(),
                None => {{
                    db.close().await;
                    true
                }}
            }}
        }};
        // On a current-thread runtime this thread drives the I/O, so a wait
        // here never ends. The close runs as a task, under the same limit.
        if runtime.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {{
            runtime.spawn(close);
            return Ok(());
        }}
        // On a multi-thread runtime the workers drive it, and a scoped
        // thread waits without blocking one of them.
        std::thread::scope(|scope| {{
            scope
                .spawn(|| runtime.block_on(close))
                .join()
                .map_err(|_| {{
                    PluginError::new(
                        "[db] the closing thread panicked\n\
                         help: read the panic message above; it names the cause",
                    )
                }})
        }})?
        .then_some(())
        .ok_or_else(|| {{
            PluginError::new(
                "[db] connections were still in use at the deadline\n\
                 help: raise `shutdown_grace_secs` under [server], or end the work that holds a connection",
            )
        }})
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `before_start` runs before the server is handed the application,
        // so the pool is here by the time a request builds a context.
        if let Some(db) = &self.db {{
            cx.insert(db.clone());
        }}
    }}
}}
"##
    )
}
