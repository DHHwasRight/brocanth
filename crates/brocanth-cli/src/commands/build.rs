//! `brocanth build` -- compile the application.
//!
//! A brocanth application is a plain Rust binary, so this is cargo with the
//! house output around it. Two things make it worth a subcommand rather than a
//! line in the README.
//!
//! First, **the compile budget is the project's own release-blocking metric**
//! (§6.2), so the number is printed every time rather than measured once in a
//! blog post. Second, the one decision reaches the subprocess: `CARGO_TERM_COLOR`
//! is set from the face, so cargo's diagnostics carry colour exactly when the
//! rest of the transcript does and a redirect gets one plain file.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

use clap::Args;

use crate::commands::Outcome;
use crate::project::Project;
use crate::ui::{self, Cell, Diagnostic, Table, Ui, ACCENT, DIM, STRONG};

#[derive(Args)]
pub struct BuildArgs {
    /// Compile with optimisations. Minutes, not seconds: for a deploy, never
    /// for the edit loop
    #[arg(long)]
    pub release: bool,
    /// Where the pages live, relative to the package root
    #[arg(long, value_name = "DIR")]
    pub pages: Option<String>,
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: BuildArgs) -> Outcome {
    let project = Project::discover(dir, args.pages.as_deref())?;
    ui.heading("build", project.display(&project.manifest_path));

    let steps = ui.steps(&["compile"]);
    steps.begin("compile");
    let started = Instant::now();
    let run = cargo(ui, &project, "build", args.release, true)?;
    let took = started.elapsed();
    steps.done(
        "compile",
        format!(
            "{} in {}{}",
            if args.release { "release" } else { "debug" },
            ui::elapsed(took),
            if run.ok { "" } else { " \u{2014} failed" }
        ),
    );
    steps.end();
    // Cargo's own errors have already been forwarded. Saying "no executable"
    // on top of a compile failure would name the symptom and bury the cause.
    if !run.ok {
        return Err(compile_failed(&project, "build"));
    }

    let executable = executable_in(&run.stdout).ok_or_else(|| {
        vec![
            Diagnostic::error("no-binary", "the package built no executable")
                .at(project.display(&project.manifest_path))
                .help("a brocanth application is a binary: add a [[bin]] target, or a src/main.rs"),
        ]
    })?;

    ui.gap();
    let mut table = Table::new();
    table.row(vec![
        Cell::styled("binary", DIM),
        Cell::styled(project.display(&executable), ACCENT),
    ]);
    table.row(vec![
        Cell::styled("compile", DIM),
        Cell::styled(ui::elapsed(took), STRONG),
    ]);
    ui.table_note(&table);
    ui.gap();
    ui.done(format!("built {}", project.package));
    // §6.2: compile time is the budget every guarantee spends, so it is
    // reported rather than discovered.
    ui.note(format!(
        "  {}",
        crate::ui::paint(
            DIM,
            "compile time is a release-blocking metric; this number is the one being tracked"
        )
    ));
    Ok(ExitCode::SUCCESS)
}

/// What cargo said.
pub struct Run {
    pub stdout: String,
    pub ok: bool,
}

/// Runs one cargo subcommand over the application.
///
/// `records` asks for `--message-format=json-render-diagnostics`, the pairing
/// that matters: cargo still renders its errors as the text a Rust developer
/// reads, on stderr, and the machine-readable artifact records arrive
/// separately on stdout. That is how the binary's path is learned without
/// guessing at `target/debug/<name>`, which is wrong the moment a workspace
/// moves its target directory or a `[[bin]]` is renamed.
pub fn cargo(
    ui: &Ui,
    project: &Project,
    subcommand: &str,
    release: bool,
    records: bool,
) -> Result<Run, Vec<Diagnostic>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(&cargo);
    cmd.arg(subcommand)
        .arg("--manifest-path")
        .arg(&project.manifest_path)
        // The one decision, handed to the subprocess so its diagnostics match
        // the rest of the transcript rather than guessing on their own.
        .env(
            "CARGO_TERM_COLOR",
            // Cargo has two rungs where we have three: it can colour or not,
            // and knows nothing of an alternate screen.
            if ui.face().can_paint() {
                "always"
            } else {
                "never"
            },
        )
        .current_dir(&project.root)
        .stdin(Stdio::null());
    if records {
        cmd.arg("--message-format=json-render-diagnostics");
    }
    if release {
        cmd.arg("--release");
    }
    let output = cmd.output().map_err(|e| {
        vec![
            Diagnostic::error("cargo", format!("{}: {e}", cargo.to_string_lossy())).help(
                "a brocanth application is compiled by cargo; install Rust, or put cargo on PATH",
            ),
        ]
    })?;
    let ok = output.status.success();
    if !ok {
        ui.verbatim(&String::from_utf8_lossy(&output.stderr));
    }
    Ok(Run {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        ok,
    })
}

/// Turns a failed cargo run into the one diagnostic that belongs on top of
/// cargo's own output, which has already been forwarded.
pub fn compile_failed(project: &Project, subcommand: &str) -> Vec<Diagnostic> {
    vec![
        Diagnostic::error("compile", format!("`cargo {subcommand}` did not succeed"))
            .at(project.display(&project.manifest_path))
            .note("the errors above are cargo's, unchanged"),
    ]
}

/// Pulls the last `"executable"` out of cargo's JSON records.
///
/// Hand-rolled rather than through serde: one field is wanted out of a stream
/// whose shape cargo documents as stable, and a JSON dependency here would be
/// a dependency in the shipped binary for a single string.
fn executable_in(stdout: &str) -> Option<PathBuf> {
    let mut found = None;
    for line in stdout.lines() {
        let Some(rest) = line.split("\"executable\":").nth(1) else {
            continue;
        };
        let rest = rest.trim_start();
        // A library or a build script records `"executable":null`.
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        let mut path = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.next() {
                    Some('\\') => path.push('\\'),
                    Some('"') => path.push('"'),
                    Some('n') => path.push('\n'),
                    Some(other) => path.push(other),
                    None => break,
                },
                other => path.push(other),
            }
        }
        if !path.is_empty() {
            found = Some(PathBuf::from(path));
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_binary_is_read_from_cargos_records_not_guessed_at() {
        let stdout = concat!(
            r#"{"reason":"compiler-artifact","executable":null}"#,
            "\n",
            r#"{"reason":"compiler-artifact","executable":"/tmp/t/debug/blog"}"#,
            "\n",
            r#"{"reason":"build-finished","success":true}"#,
            "\n",
        );
        assert_eq!(
            executable_in(stdout),
            Some(PathBuf::from("/tmp/t/debug/blog"))
        );
    }

    #[test]
    fn a_run_that_produced_no_executable_says_none_rather_than_a_guess() {
        assert_eq!(executable_in(r#"{"executable":null}"#), None);
        assert_eq!(executable_in(""), None);
    }
}
