//! `brocanth check` and `brocanth check --deploy` -- what is wrong before it
//! is live.
//!
//! Django's `check --deploy` (D23), in two halves, each where its knowledge
//! is:
//!
//! * **The module checks run here.** `check/recommended`: a module in
//!   `brocanth.toml` recommends one the project lacks. Only this tool holds
//!   every module's manifest, so only it can say so. It reads
//!   `brocanth.toml` as text and nothing else.
//! * **The configuration and source checks are the project's.** A project
//!   with `config` and `commands` has a `check` command that loads the
//!   configuration the way the application does (`brocanth_config::check`
//!   holds the list). This command runs it next, with the same arguments.
//!   That builds the project, as every project command does, and connects
//!   to nothing.
//!
//! The CLI does not load the configuration itself: that would link the
//! loader, and the core under it, into a tool whose job is to write files.
//!
//! `--allow <ID>` silences one finding, as Django's
//! `SILENCED_SYSTEM_CHECKS` does. The exit code is 1 when an error remains
//! in either half, and with `--strict` when a warning remains too.

use std::ffi::OsString;
use std::path::Path;
use std::process::ExitCode;

use clap::Args;

use crate::commands::delegate;
use crate::commands::project::Project;
use crate::commands::Outcome;
use crate::ui::{Diagnostic, Ui};

#[derive(Args)]
pub struct CheckArgs {
    /// Add the production checks: secrets, cookies, CSRF, the admin, the database
    #[arg(long)]
    pub deploy: bool,
    /// The environment to load [default: production with --deploy, else $<APP>_ENV or development]
    #[arg(long, short, value_name = "ENV")]
    pub env: Option<String>,
    /// Silence one finding by its id, such as deploy/hsts. Repeat for more
    #[arg(long, value_name = "ID")]
    pub allow: Vec<String>,
    /// Exit 1 on a warning too
    #[arg(long)]
    pub strict: bool,
}

impl CheckArgs {
    /// The same arguments, for the project's own `check`.
    fn forwarded(&self) -> Vec<OsString> {
        let mut out: Vec<OsString> = vec!["check".into()];
        if self.deploy {
            out.push("--deploy".into());
        }
        if let Some(env) = &self.env {
            out.push("--env".into());
            out.push(env.into());
        }
        for id in &self.allow {
            out.push("--allow".into());
            out.push(id.into());
        }
        if self.strict {
            out.push("--strict".into());
        }
        out
    }
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: CheckArgs) -> Outcome {
    let project = Project::find(dir)?;

    let mut findings = modules(&project);
    findings.retain(|d| !d.code.as_ref().is_some_and(|c| args.allow.contains(c)));
    for d in &findings {
        ui.diagnostic(d);
    }
    // Every module finding is a warning: the project builds and runs
    // without a recommended module, and leaving it out is the person's call.
    let mut failed = args.strict && !findings.is_empty();
    if findings.is_empty() {
        ui.done("module checks pass");
    } else {
        ui.note(format!(
            "module checks: {} warning{}",
            findings.len(),
            if findings.len() == 1 { "" } else { "s" }
        ));
    }

    // An empty list is a marker from before the list: try the project.
    let has = |key: &str| project.modules.is_empty() || project.has(key);
    if has("config") && has("commands") {
        let passed = delegate::run_and_wait(&project.root, &args.forwarded())?;
        failed |= !passed;
    } else {
        ui.diagnostic(&skipped(&project));
    }

    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// What the modules recommend and the project lacks.
pub fn modules(project: &Project) -> Vec<Diagnostic> {
    let modules: Vec<&str> = project.modules.iter().map(String::as_str).collect();
    crate::questions::wanted(&modules)
        .into_iter()
        .map(|wanted| {
            wanted
                .diagnostic("check/recommended", &modules)
                .at("brocanth.toml")
        })
        .collect()
}

/// The note for a project that has no `check` command of its own.
fn skipped(project: &Project) -> Diagnostic {
    let missing: Vec<&str> = ["config", "commands"]
        .into_iter()
        .filter(|key| !project.has(key))
        .collect();
    Diagnostic::warning(
        "check/no-project-checks",
        "the configuration and source checks did not run: this project has no `check` command",
    )
    .note("they load the configuration the way the application does, so they run in the project")
    .help(format!("`brocanth add {}` gives it one", missing.join(" ")))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn project(modules: &[&str]) -> Project {
        Project {
            root: PathBuf::from("/p"),
            modules: modules.iter().map(|m| (*m).to_owned()).collect(),
        }
    }

    #[test]
    fn a_recommended_module_that_is_absent_is_a_warning_with_the_command_to_add_it() {
        let found = modules(&project(&["config", "db", "docker"]));
        let ids: Vec<&str> = found.iter().filter_map(|d| d.code.as_deref()).collect();
        assert_eq!(ids, ["check/recommended", "check/recommended"]);
        assert!(found
            .iter()
            .all(|d| d.severity == crate::ui::Severity::Warning));
        let said: String = found.iter().map(Diagnostic::render).collect();
        assert!(said.contains("observability-logs"), "{said}");
        assert!(
            said.contains("brocanth add observability observability-health"),
            "{said}"
        );

        let full = project(&["db", "observability", "observability-health"]);
        assert!(modules(&full).is_empty());
    }

    #[test]
    fn the_project_s_check_gets_the_same_arguments() {
        let args = CheckArgs {
            deploy: true,
            env: Some("staging".to_owned()),
            allow: vec!["deploy/hsts".to_owned(), "check/dialect".to_owned()],
            strict: true,
        };
        let said: Vec<String> = args
            .forwarded()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            said,
            [
                "check",
                "--deploy",
                "--env",
                "staging",
                "--allow",
                "deploy/hsts",
                "--allow",
                "check/dialect",
                "--strict"
            ]
        );
    }

    #[test]
    fn a_project_without_its_own_check_is_told_what_to_add() {
        let d = skipped(&project(&["db"]));
        assert!(
            d.render().contains("brocanth add config commands"),
            "{}",
            d.render()
        );
        let d = skipped(&project(&["config"]));
        assert!(
            d.render().contains("brocanth add commands"),
            "{}",
            d.render()
        );
    }
}
