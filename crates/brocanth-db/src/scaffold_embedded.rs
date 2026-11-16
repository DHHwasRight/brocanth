//! `db-embedded` -- a Postgres server inside the development binary.
//!
//! A child of `db`, off until asked for, and development only. Its plugin
//! starts the server from the binary in `configure`, before the `db`
//! plugin, and hands `db` the URL through `brocanth_db::embedded::EmbeddedUrl`.
//! The pool stays `db`'s: this module opens none.
//!
//! What a project gets:
//!
//! - `src/plugins/db_embedded/mod.rs`: the plugin. It refuses to start
//!   outside `development` and `test`, and stops the server on shutdown.
//! - `brocanth-db` with the feature `embedded`, which bundles the server
//!   archive into the binary at build time.
//! - `[db_embedded]` in the config file: the directory, the port, the
//!   database name, the timeout.
//! - `/tmp/postgres` in `.gitignore`.
//!
//! `brocanth check --deploy` refuses it through `development_only`.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{configurable, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// A development Postgres in the binary, as a child of `db` on the menu.
pub struct DbEmbedded;

impl Module for DbEmbedded {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "db-embedded",
            "a Postgres server started from the binary, for development with no Postgres installed; never in production",
        )
        .category(Category::Enhancer)
        .part_of_but_off("db")
        .because("it starts the server that the `db` pool opens; without the pool nothing connects to it")
        .asked_in("data", "Postgres inside the binary, for development")
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .development_only(
            "the server runs inside one process on one disk, with a generated password in a file beside its data; a deploy needs a managed or a real Postgres",
        )
        .krate(
            Dep::brocanth("brocanth-db")
                .feature("embedded")
                .note("the Postgres archive, bundled at build time"),
        )
        // No `after`: `db` waits for this module (`after("db-embedded")` in
        // its manifest), so the server runs before the pool reads a URL. An
        // `after("config")` here would push `db`, and every module after
        // it, one step later in every project that has a config file.
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/db_embedded/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod db_embedded;");
        configurable(s, project, "db_embedded", "DbEmbedded", settings(project));
        if !project.has("config") {
            // The literal form calls `Plugin::new`, a trait method, so the
            // trait has to be in scope.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        s.at(Slot::GitIgnore).line("/tmp/postgres");
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The database name: the project's name, with `_` for `-`, so `psql`
/// takes it unquoted.
fn database(project: &Project) -> String {
    project.name.replace('-', "_")
}

/// The `[db_embedded]` section.
fn settings(project: &Project) -> Vec<Setting> {
    vec![
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
    ]
}

const AGENTS: &str = r#"
## The development database

`src/plugins/db_embedded/mod.rs` starts a Postgres server from the binary
when the application starts, and the `db` pool opens on it: `cargo run`
needs no Postgres on the machine. The data is in `tmp/postgres/data` and
survives a restart; delete `tmp/postgres` for an empty database.

It is for development only. It refuses to start when `$<APP>_ENV` is not
`development` or `test`, and `brocanth check --deploy` reports it. Before a
deploy, run `brocanth remove db-embedded` and set `[db] url`.

`migrate` and `dbshell` read `[db] url` and do not start the server. Set
`[db_embedded] port`, run the application, and point `[db] url` at that port
to use them.
"#;

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "db_embedded");
    let database = database(project);
    format!(
        r##"//! A Postgres server inside this binary, for development.
//!
//! `configure` starts it from the archive the `embedded` feature of
//! `brocanth-db` bundled, before the `db` plugin, and publishes its URL. The
//! `db` plugin opens its pool on that URL and not on `[db] url`. `shutdown`
//! stops the server after the pool closed.
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
         help: run `brocanth remove db-embedded` and set `[db] url` to a real server \
         before you deploy"
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
        // Blocks until the server answers: the first start extracts the
        // binaries and runs initdb, a few seconds; a later one starts it.
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
