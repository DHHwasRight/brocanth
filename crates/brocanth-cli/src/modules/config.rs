//! `config` -- TOML configuration, four sources, merged.

use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Slot};

/// The config plugin.
///
/// The one choice that changes the *source* of every other choice rather
/// than adding to it: with it a plugin's settings derive `Deserialize` and
/// `main` reads a file; without it the struct derives nothing, `main` writes
/// a literal, and no parser is linked.
pub struct Config;

impl Module for Config {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "config",
            "config/<env>.toml, merged; without it nothing links a TOML parser",
        )
        .category(Category::Enhancer)
        .outside_runtime()
        .asked_in(
            "foundations",
            "Settings in config/<env>.toml, layered per environment",
        )
        .adapts_to(
            "commands",
            "a `check` command runs the configuration and deploy checks",
        )
        .krate(Dep::brocanth("brocanth-config").feature_with("commands", "check"))
        .recommends(
            "observability-logs",
            "$<APP>_ENV picks the environment; only observability-logs writes JSON when it is production",
        )
        .absent(
            "brocanth-config",
            "no TOML parser; main builds each plugin from a literal",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        // The files themselves are still written by `create.rs`. The checks
        // are this module's: they read the configuration the way the
        // application does, so they are a command of the project and not of
        // the `brocanth` CLI, which links no configuration loader.
        if project.has("commands") {
            s.file("src/commands/check.rs", CHECK_RS);
            s.at(Slot::CommandsMod).line("pub mod check;");
        }
    }
}

/// `src/commands/check.rs`: `brocanth_config::check`, as a command.
const CHECK_RS: &str = r#"//! `check` -- what is wrong before it is live.
//!
//! Reads the configuration of one environment the way the application does,
//! `brocanth.toml`, and `src/` as text. Nothing is built and nothing
//! connects, so it runs in CI. `--deploy` adds the production checks; the
//! list is in `brocanth_config::check`. `brocanth check` runs this command
//! after the checks only the CLI can make.
//!
//! Ordinary project code, like `serve`: rename it or delete it, and delete
//! its line in `src/commands/mod.rs`.

use brocanth_command::{anyhow, clap, cmd, Command, Context};
use brocanth_config::check::{self, Options};

/// Check the configuration and the source; --deploy adds the production checks.
#[cmd]
#[derive(clap::Args)]
pub struct Check {
    /// Add the production checks: secrets, cookies, CSRF, the admin, the database
    #[arg(long)]
    deploy: bool,
    /// The environment to load [default: production with --deploy, else $<APP>_ENV or development]
    #[arg(long, short, value_name = "ENV")]
    env: Option<String>,
    /// Silence one finding by its id, such as deploy/hsts. Repeat for more
    #[arg(long, value_name = "ID")]
    allow: Vec<String>,
    /// Exit 1 on a warning too
    #[arg(long)]
    strict: bool,
}

impl Command for Check {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        let options = Options {
            deploy: self.deploy,
            env: self.env,
            allow: self.allow,
        };
        let report = check::run(crate::app::APP, ctx.root(), &options)?;
        eprint!("{report}");
        if report.failed(self.strict) {
            std::process::exit(1);
        }
        Ok(())
    }
}
"#;
