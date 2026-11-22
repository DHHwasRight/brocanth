//! `db-embedded` -- a Postgres server inside the development binary, and
//! `db-embedded-prod` -- the same server, for production too.
//!
//! `db-embedded` is a child of `db`, off until asked for, and development
//! only. `db-embedded-prod` is its production opt-in: no
//! `development_only`, the production rules of `brocanth_db::embedded::Mode`
//! outside `development` and `test`, and backup and restore commands
//! (`docs/embedded-postgres-production.md`). Both write the same plugin
//! file and the same `[db_embedded]` section, so a switch is one `remove`
//! and one `add`.
//!
//! `db-embedded`: Its plugin
//! starts the server from the binary in `configure`, before the `db`
//! plugin, and hands `db` the URL through `brocanth_db::embedded::EmbeddedUrl`.
//! The pool stays `db`'s: this module opens none.
//!
//! What a project gets:
//!
//! - `src/plugins/db_embedded/mod.rs`: the plugin. It refuses to start
//!   outside `development` and `test`, and releases the server on
//!   shutdown.
//! - `brocanth-db` with the feature `embedded`, which installs one pinned
//!   Postgres release at the first start. The build needs no network.
//! - `[db_embedded]` in the config file: the directory, the port, the
//!   database name, the timeout.
//! - `/tmp/postgres` in `.gitignore`.
//!
//! `brocanth check --deploy` refuses it through `development_only`.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{configurable, settings_derive};
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Manifest, Module, Project, Scaffold, Setting, Slot,
};

/// A development Postgres in the binary, as a child of `db` on the menu.
pub struct DbEmbedded;

impl Module for DbEmbedded {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "db-embedded",
            "a Postgres server started from the binary, for development with no Postgres installed; never in production",
        )
        .contributes::<AgentsSection>(Contribution::new("db-embedded", "the database runs in the binary, for development only"), true)
        .category(Category::Enhancer)
        .part_of_but_off("db")
        .because("it starts the server that the `db` pool opens; without the pool nothing connects to it")
        .asked_in("data", "Postgres inside the binary, for development")
        .development_only(
            "the server is shared by every process on a relative directory, connects as the superuser and may download its archive; a deploy needs a real Postgres, or the production module `db-embedded-prod`",
        )
        .krate(
            Dep::brocanth("brocanth-db")
                .feature("embedded")
                .note("a pinned Postgres release, installed at the first start"),
        )
        // No `after`: `db` waits for this module (`after("db-embedded")` in
        // its manifest), so the server runs before the pool reads a URL.
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        contribute(project, s, false);
    }
}

/// Postgres in the binary, in production too: the explicit opt-in.
pub struct DbEmbeddedProd;

impl Module for DbEmbeddedProd {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "db-embedded-prod",
            "a Postgres server started from the binary, in production too: one binary, one data directory, an archive pinned by hash",
        )
        .contributes::<AgentsSection>(Contribution::new("db-embedded-prod", "the database runs in the binary, in production too"), true)
        .category(Category::Enhancer)
        .part_of_but_off("db")
        .because("it starts the server that the `db` pool opens; without the pool nothing connects to it")
        .asked_in("data", "Postgres inside the binary, in production too")
        .conflicts(
            "db-embedded",
            "both start the server of `[db_embedded]`; this one adds the production rules, so keep one",
        )
        .adapts_to(
            "commands",
            "`db_backup` and `db_restore` commands dump and restore the database with the server's own pg_dump and pg_restore",
        )
        .krate(
            Dep::brocanth("brocanth-db")
                .feature("embedded")
                .note("a pinned Postgres release, checked by SHA-256, never downloaded in production"),
        )
        // No `after`: `db` waits for this module (`after("db-embedded-prod")`
        // in its manifest), so the server runs before the pool reads a URL.
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        contribute(project, s, true);
        if project.has("commands") {
            s.file("src/commands/db_backup.rs", DB_BACKUP_RS);
            s.at(Slot::CommandsMod).line("pub mod db_backup;");
            s.file("src/commands/db_restore.rs", DB_RESTORE_RS);
            s.at(Slot::CommandsMod).line("pub mod db_restore;");
        }
    }
}

/// What both modules write. `production` is `db-embedded-prod`.
fn contribute(project: &Project, s: &mut Scaffold, production: bool) {
    s.file(
        "src/plugins/db_embedded/mod.rs",
        plugin_rs(project, production),
    );
    s.at(Slot::PluginsMod).line("pub mod db_embedded;");
    configurable(
        s,
        project,
        "db_embedded",
        "DbEmbedded",
        settings(project, production),
    );
    s.at(Slot::GitIgnore).line("/tmp/postgres");
    let agents = if production { AGENTS_PROD } else { AGENTS };
    s.at(Slot::AgentsSections).block(agents.to_string());
}

/// The database name: the project's name, with `_` for `-`, so `psql`
/// takes it unquoted.
fn database(project: &Project) -> String {
    project.name.replace('-', "_")
}

/// The `[db_embedded]` section. Production adds `archive`.
fn settings(project: &Project, production: bool) -> Vec<Setting> {
    let mut settings = vec![
        Setting::text(
            "dir",
            "tmp/postgres",
            "The directory for the server's binaries, data and password, from the project root.",
        )
        .note("Delete it to start from an empty database."),
        Setting::integer(
            "port",
            0,
            "The port on localhost. 0 takes a free one at each start.",
        )
        .note("Set one to reach the server with psql while the application runs."),
        Setting::text(
            "database",
            &database(project),
            "The database the pool opens, created at the first start.",
        ),
        Setting::integer("timeout", 60, "Seconds initdb or pg_ctl may take, each."),
    ];
    if production {
        settings[0] = Setting::text(
            "dir",
            "tmp/postgres",
            "The directory for the server's binaries, data and passwords. Production needs an absolute path on a volume.",
        )
        .note("Delete it to start from an empty database. In production, pass --db_embedded.dir=/var/lib/<app>.");
        settings.push(
            Setting::text(
                "archive",
                "",
                "A postgresql-<version>-<target>.tar.gz file to install from, checked by SHA-256. Empty: none.",
            )
            .note("Production never downloads: bundle the archive at build time, name it here, or install it once."),
        );
    }
    settings
}

const AGENTS: &str = r#"
## The development database

`src/plugins/db_embedded/mod.rs` starts a Postgres server from the binary
when the application starts, and the `db` pool opens on it: `cargo run`
needs no Postgres on the machine. The data is in `tmp/postgres/data` and
survives a restart; delete `tmp/postgres` for an empty database.

The build needs no network. The first start installs Postgres
`brocanth_db::embedded::POSTGRES_VERSION` from the first of:
`$BROCANTH_POSTGRES_ARCHIVE` (a `postgresql-<version>-<target>.tar.gz`
file, for a machine with no network), the cache in `~/.theseus/postgresql`,
or one download from GitHub (set `GITHUB_TOKEN` behind a rate limit).

Every application started on `tmp/postgres` uses one server: tests that run
in parallel, and a second terminal, join the server that runs. The last one
to stop stops it.

It is for development only. It refuses to start when `$<APP>_ENV` is not
`development` or `test`, and `brocanth check --deploy` reports it. Before a
deploy, run `brocanth remove db-embedded` and set `[db] url`, or switch to
the production module: `brocanth remove db-embedded && brocanth add
db-embedded-prod`.

`migrate` and `dbshell` read `[db] url` and do not start the server. Set
`[db_embedded] port`, run the application, and point `[db] url` at that port
to use them.
"#;

const AGENTS_PROD: &str = r#"
## The embedded database

`src/plugins/db_embedded/mod.rs` starts a Postgres server from the binary
when the application starts, and the `db` pool opens on it: the binary and
one data directory are the whole deploy. The pool connects as a role named
after the database, which owns the database and is not a superuser.

In `development` and `test` it works as a development server: the data is
in `tmp/postgres`, and the tests and a second terminal share the server.

In any other environment it runs in production mode:

- `--db_embedded.dir=<absolute path>` is required: a volume that survives a
  restart. A relative or temporary path is refused at start.
- The start never downloads Postgres. Build with
  `BROCANTH_POSTGRES_BUNDLE=<archive> cargo build --release` to carry the
  archive in the binary, or pass `--db_embedded.archive=<file>`. Every
  archive is checked against the SHA-256 pinned in `brocanth-db`.
- One process owns the server. A second one is refused.
- The server's log goes to stderr, each line prefixed `postgres: `.
- A data directory of another Postgres major version is refused.

Back up with `db_backup <file>` (it reads the running server) and restore
with `db_restore <file>` (the application stopped). Do not run as root:
Postgres refuses it. See `docs/modules/db-embedded-prod.md` in brocanth.
"#;

/// `src/commands/db_backup.rs`.
const DB_BACKUP_RS: &str = r#"//! `db_backup` -- a `pg_dump` of the embedded database, to a file.
//!
//! Reads `[db_embedded]` the way the server does. When the application
//! runs, the dump connects to its server; otherwise it starts the server
//! for the dump and stops it. The file is in pg_dump's custom format, for
//! `db_restore`.
//!
//! Ordinary project code: rename it or delete it, and delete its line in
//! `src/commands/mod.rs`.

use std::path::PathBuf;

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Write a backup of the embedded database to a file
#[cmd]
#[derive(clap::Args)]
pub struct DbBackup {
    /// The file to write, in pg_dump's custom format
    file: PathBuf,
}

impl Command for DbBackup {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        let file = std::path::absolute(&self.file)?;
        // The config files resolve from the project root.
        std::env::set_current_dir(ctx.root())?;
        let config = crate::plugins::db_embedded::config_from_layers()?;
        brocanth_db::embedded::backup(&config, &file)?;
        println!("db_backup: wrote {}", file.display());
        Ok(())
    }
}
"#;

/// `src/commands/db_restore.rs`.
const DB_RESTORE_RS: &str = r#"//! `db_restore` -- the embedded database replaced by a backup file.
//!
//! Stop the application first: the command refuses a running server. It
//! restores into a new database in one transaction and swaps the names
//! only when the restore succeeded, so a failed restore changes nothing.
//!
//! Ordinary project code: rename it or delete it, and delete its line in
//! `src/commands/mod.rs`.

use std::path::PathBuf;

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Replace the embedded database with a backup file
#[cmd]
#[derive(clap::Args)]
pub struct DbRestore {
    /// A file `db_backup` wrote
    file: PathBuf,
}

impl Command for DbRestore {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        let file = std::path::absolute(&self.file)?;
        // The config files resolve from the project root.
        std::env::set_current_dir(ctx.root())?;
        let config = crate::plugins::db_embedded::config_from_layers()?;
        brocanth_db::embedded::restore(&config, &file)?;
        println!("db_restore: restored {}", file.display());
        Ok(())
    }
}
"#;

fn plugin_rs(project: &Project, production: bool) -> String {
    if production {
        return plugin_prod_rs(project);
    }
    let (derive, note) = settings_derive(project, "db_embedded");
    let database = database(project);
    format!(
        r##"//! A Postgres server inside this binary, for development.
//!
//! `configure` starts it, before the `db` plugin, and publishes its URL. The
//! `db` plugin opens its pool on that URL and not on `[db] url`. `shutdown`
//! releases the server after the pool closed; the last user stops it.
//! Applications on the same `dir`, in this process or another, share one
//! server.
//!
//! Never in production: it refuses to start outside `development` and
//! `test`, and `brocanth check --deploy` reports the module. Delete the
//! registration in `src/plugins/mod.rs` and this file (or run `brocanth
//! remove db-embedded`), and the pool goes back to `[db] url`.

use std::time::Duration;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_shutdown::ShutdownCx;
use brocanth_db::embedded::{{EmbeddedConfig, EmbeddedPostgres, EmbeddedUrl}};

/// What this plugin is built from: `[db_embedded]`.
///
/// {note}
{derive}
pub struct Settings {{
    /// The directory for the server's binaries, data and password, from the
    /// project root. Delete it to start from an empty database.
    pub dir: String,
    /// The port on localhost. 0 takes a free one at each start.
    pub port: u16,
    /// The database the pool opens, created at the first start.
    pub database: String,
    /// Seconds `initdb` or `pg_ctl` may take, each.
    pub timeout: u64,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            dir: "tmp/postgres".to_string(),
            port: 0,
            database: {database:?}.to_string(),
            timeout: 60,
        }}
    }}
}}

pub struct DbEmbedded {{
    config: EmbeddedConfig,
    /// Running from `configure` until `shutdown`.
    server: Option<EmbeddedPostgres>,
}}

/// `$<APP>_ENV`, the variable the config plugin reads too.
fn environment() -> String {{
    let var = format!("{{}}_ENV", crate::app::APP.to_uppercase().replace('-', "_"));
    std::env::var(var).unwrap_or_else(|_| "development".to_string())
}}

/// Refuses every environment but `development` and `test`.
fn check_environment(environment: &str) -> Result<(), PluginError> {{
    if matches!(environment, "development" | "test") {{
        return Ok(());
    }}
    Err(PluginError::new(format!(
        "[db_embedded] starts a Postgres server inside this process, and this is \
         `{{environment}}`: it is for development and tests only\n\
         help: before you deploy, run `brocanth remove db-embedded` and set `[db] url` to a \
         real server, or switch to `brocanth add db-embedded-prod`"
    )))
}}

impl Plugin for DbEmbedded {{
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "db_embedded";

    fn new(settings: Settings) -> Self {{
        let mut config = EmbeddedConfig::new(settings.dir, settings.database);
        config.port = settings.port;
        config.timeout = Duration::from_secs(settings.timeout);
        Self {{
            config,
            server: None,
        }}
    }}

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        check_environment(&environment())?;
        // Blocks until the server answers: the first start installs the
        // binaries and runs initdb, a few seconds; a later one starts it, or
        // joins the server another application on `dir` runs.
        let server = EmbeddedPostgres::start(&self.config)
            .map_err(|e| PluginError::new(format!("[db_embedded] {{e}}")))?;
        let version = server.version();
        let url = server.redacted_url();
        crate::plugins::say!(info, "db_embedded: Postgres {{version}} on {{url}}");
        // For the `db` plugin, registered after this one.
        lc.services_mut()
            .insert(EmbeddedUrl(server.url().to_string()));
        self.server = Some(server);
        Ok(())
    }}

    fn shutdown(&mut self, _sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {{
        // `db` closed its pool already: shutdown runs in reverse order.
        let Some(server) = self.server.take() else {{
            return Ok(());
        }};
        server
            .stop()
            .map_err(|e| PluginError::new(format!("[db_embedded] {{e}}")))
    }}
}}

#[cfg(test)]
mod tests {{
    #[test]
    fn only_development_and_test_start_it() {{
        assert!(super::check_environment("development").is_ok());
        assert!(super::check_environment("test").is_ok());
        let refused = super::check_environment("production").expect_err("refused");
        assert!(refused.to_string().contains("help:"), "{{refused}}");
    }}
}}
"##
    )
}

/// The plugin of `db-embedded-prod`: the development server in
/// `development` and `test`, production mode elsewhere.
fn plugin_prod_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "db_embedded");
    let database = database(project);
    // Only the commands read the settings outside the plugin.
    let from_layers = if project.has("commands") {
        r#"
/// The server's config from every settings layer, for `db_backup` and
/// `db_restore`.
pub fn config_from_layers() -> anyhow::Result<EmbeddedConfig> {
    let layers = crate::plugins::layers().map_err(anyhow::Error::msg)?;
    let settings = crate::plugins::db_embedded_settings(&layers).map_err(anyhow::Error::msg)?;
    Ok(config(settings, &environment()))
}
"#
    } else {
        ""
    };
    let anyhow = if project.has("commands") {
        "use brocanth_command::anyhow;\n"
    } else {
        ""
    };
    format!(
        r##"//! A Postgres server inside this binary, for development and production.
//!
//! `configure` starts it, before the `db` plugin, and publishes its URL. The
//! `db` plugin opens its pool on that URL and not on `[db] url`. `shutdown`
//! stops the server after the pool closed (`pg_ctl stop -m fast`, waited
//! for). The pool connects as a role named after the database: it owns the
//! database and is not a superuser.
//!
//! In `development` and `test`, applications on the same `dir` share one
//! server. In any other environment it runs in production mode
//! (`brocanth_db::embedded::Mode::Production`): `dir` must be an absolute
//! path, one process owns the server, the start never downloads Postgres,
//! and the server log goes to stderr.

use std::time::Duration;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_shutdown::ShutdownCx;
{anyhow}use brocanth_db::embedded::{{EmbeddedConfig, EmbeddedPostgres, EmbeddedUrl, Mode}};

/// What this plugin is built from: `[db_embedded]`.
///
/// {note}
{derive}
pub struct Settings {{
    /// The directory for the server's binaries, data and passwords.
    /// Production needs an absolute path on a volume.
    pub dir: String,
    /// The port on localhost. 0 takes a free one at each start.
    pub port: u16,
    /// The database the pool opens, created at the first start. The pool's
    /// role has the same name.
    pub database: String,
    /// Seconds `initdb` or `pg_ctl` may take, each.
    pub timeout: u64,
    /// A `postgresql-<version>-<target>.tar.gz` file to install from,
    /// checked by SHA-256. Empty: none.
    pub archive: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            dir: "tmp/postgres".to_string(),
            port: 0,
            database: {database:?}.to_string(),
            timeout: 60,
            archive: String::new(),
        }}
    }}
}}

pub struct DbEmbedded {{
    settings: Settings,
    /// Running from `configure` until `shutdown`.
    server: Option<EmbeddedPostgres>,
}}

/// `$<APP>_ENV`, the variable the config plugin reads too.
fn environment() -> String {{
    let var = format!("{{}}_ENV", crate::app::APP.to_uppercase().replace('-', "_"));
    std::env::var(var).unwrap_or_else(|_| "development".to_string())
}}

/// Shared in `development` and `test`, owned in every other environment.
fn mode(environment: &str) -> Mode {{
    if matches!(environment, "development" | "test") {{
        Mode::Development
    }} else {{
        Mode::Production
    }}
}}

/// The server's config for `environment`.
fn config(settings: Settings, environment: &str) -> EmbeddedConfig {{
    let mut config = EmbeddedConfig::new(settings.dir, settings.database.clone());
    config.port = settings.port;
    config.timeout = Duration::from_secs(settings.timeout);
    config.mode = mode(environment);
    config.archive = (!settings.archive.is_empty()).then(|| settings.archive.into());
    config.app_role = Some(settings.database);
    config
}}
{from_layers}
impl Plugin for DbEmbedded {{
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "db_embedded";

    fn new(settings: Settings) -> Self {{
        Self {{
            settings,
            server: None,
        }}
    }}

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        let environment = environment();
        let config = config(std::mem::take(&mut self.settings), &environment);
        // Blocks until the server answers: the first start installs the
        // binaries and runs initdb, a few seconds. A bad `dir`, a missing
        // archive or a second owner fails here, at start.
        let server = EmbeddedPostgres::start(&config)
            .map_err(|e| PluginError::new(format!("[db_embedded] {{e}}")))?;
        let version = server.version();
        let url = server.redacted_url();
        let mode = format!("{{:?}}", config.mode).to_lowercase();
        crate::plugins::say!(info, "db_embedded: Postgres {{version}} on {{url}} ({{mode}})");
        // For the `db` plugin, registered after this one.
        lc.services_mut()
            .insert(EmbeddedUrl(server.url().to_string()));
        self.server = Some(server);
        Ok(())
    }}

    fn shutdown(&mut self, _sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {{
        // `db` closed its pool already: shutdown runs in reverse order.
        let Some(server) = self.server.take() else {{
            return Ok(());
        }};
        server
            .stop()
            .map_err(|e| PluginError::new(format!("[db_embedded] {{e}}")))
    }}
}}

#[cfg(test)]
mod tests {{
    use brocanth_db::embedded::Mode;

    #[test]
    fn production_rules_apply_outside_development_and_test() {{
        assert_eq!(super::mode("development"), Mode::Development);
        assert_eq!(super::mode("test"), Mode::Development);
        assert_eq!(super::mode("production"), Mode::Production);
        assert_eq!(super::mode("staging"), Mode::Production);
    }}

    #[test]
    fn the_pool_connects_as_a_role_named_after_the_database() {{
        let config = super::config(super::Settings::default(), "production");
        assert_eq!(config.app_role.as_deref(), Some(config.database.as_str()));
        assert!(config.archive.is_none(), "an empty archive is none");
    }}
}}
"##
    )
}
