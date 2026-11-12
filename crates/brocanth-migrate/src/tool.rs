//! The `migrate` tool: `ids`, `make` and `sync` over a project on disk.
//!
//! A project with the `model` module has a second binary,
//! `src/bin/migrate.rs`, whose `main` is [`main`]. It is a development
//! tool: it reads `src/models/` as text and writes Rust source, so it never
//! ships and never touches a database. Applying a migration is the
//! application's own `migrate up`, because the migrations are compiled
//! into the application's binary.
//!
//! ```text
//! cargo run --bin migrate -- ids
//! cargo run --bin migrate -- make add_posts
//! cargo run --bin migrate -- sync
//! ```
//!
//! It was a subcommand of the global `brocanth` CLI. It moved here so the
//! CLI does not link a Rust parser for a command only a project with an
//! ORM can use.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::migrator::{migrator_file, unedited, MIGRATOR};
use crate::render::render;
use crate::scan::{scan_source, schema_of, Model};
use crate::stamp::stamp;
use crate::{diff, Problem, Schema};

/// Where the models are, from the project root.
pub const MODELS: &str = "src/models";
/// Where the migrations and the snapshot go, from the project root.
pub const MIGRATIONS: &str = "migrations";
/// The snapshot, in [`MIGRATIONS`].
pub const SNAPSHOT: &str = "schema.json";
/// The file that says "a project starts here".
pub const PROJECT_MARKER: &str = "brocanth.toml";

const USAGE: &str = "\
Write migrations from the models in src/models, and the ids that make a rename visible

Usage: migrate <COMMAND>

Commands:
  ids                          Write an id on each model and field that has none
  make <NAME> [--snapshot-only]  Write a migration for what changed since the last one
  sync                         List every migrations/m*.rs file in migrations/mod.rs

`make` applies nothing. Read the file it writes, then run the application's
`migrate up`.";

/// One finding, printed as `error[migrate/<code>]: <message>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// The id after `migrate/`.
    pub code: String,
    /// What is wrong.
    pub message: String,
    /// Where, as `path` or `path:line:col`.
    pub at: Option<String>,
    /// A sentence of context.
    pub note: Option<String>,
    /// What to do next.
    pub help: Option<String>,
}

impl Diagnostic {
    fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            at: None,
            note: None,
            help: None,
        }
    }

    fn at(mut self, at: impl Into<String>) -> Self {
        self.at = Some(at.into());
        self
    }

    fn help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error[migrate/{}]: {}", self.code, self.message)?;
        if let Some(at) = &self.at {
            write!(f, "\n  --> {at}")?;
        }
        if let Some(note) = &self.note {
            write!(f, "\nnote: {note}")?;
        }
        if let Some(help) = &self.help {
            write!(f, "\nhelp: {help}")?;
        }
        Ok(())
    }
}

/// What a run prints, in order. The binary writes it; a test reads it.
#[derive(Debug, Default)]
pub struct Report {
    /// Lines for stdout.
    pub lines: Vec<String>,
    /// Warnings, for stderr, each already prefixed with `warning: `.
    pub warnings: Vec<String>,
}

impl Report {
    fn say(&mut self, line: impl Into<String>) {
        self.lines.push(line.into());
    }

    fn warn(&mut self, line: impl Into<String>) {
        self.warnings.push(format!("warning: {}", line.into()));
    }
}

/// `main` for `src/bin/migrate.rs`: reads the arguments, finds the project
/// root from the current directory, runs, prints, and exits 0, 1 or 2.
pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = match std::env::current_dir()
        .map_err(|e| {
            vec![Diagnostic::new(
                "cwd",
                format!("cannot read the current directory: {e}"),
            )
            .help("run the tool from a directory that exists and that you can read, inside the project")]
        })
        .and_then(|cwd| project_root(&cwd))
    {
        Ok(root) => root,
        Err(problems) => return fail(&problems),
    };
    let mut report = Report::default();
    let result = run(&root, &args, &mut report);
    for line in &report.warnings {
        eprintln!("{line}");
    }
    for line in &report.lines {
        println!("{line}");
    }
    match result {
        Ok(code) => code,
        Err(problems) => fail(&problems),
    }
}

fn fail(problems: &[Diagnostic]) -> ExitCode {
    for problem in problems {
        eprintln!("{problem}");
    }
    ExitCode::from(1)
}

/// The nearest ancestor of `start` holding [`PROJECT_MARKER`].
pub fn project_root(start: &Path) -> Result<PathBuf, Vec<Diagnostic>> {
    start
        .ancestors()
        .find(|dir| dir.join(PROJECT_MARKER).is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            vec![Diagnostic::new(
                "no-project",
                format!("no `{PROJECT_MARKER}` here or in any parent directory"),
            )
            .help("run this inside a brocanth project")]
        })
}

/// One run of the tool over the project at `root`.
pub fn run(root: &Path, args: &[String], report: &mut Report) -> Result<ExitCode, Vec<Diagnostic>> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["ids"] => ids(root, report),
        ["sync"] => sync(root, report).map(|()| {
            report.say("migrations/mod.rs lists every migration file.");
            ExitCode::SUCCESS
        }),
        ["make", rest @ ..] => {
            let snapshot_only = rest.contains(&"--snapshot-only");
            let names: Vec<&&str> = rest.iter().filter(|a| !a.starts_with("--")).collect();
            match (names.as_slice(), rest.iter().find(|a| a.starts_with("--") && **a != "--snapshot-only")) {
                (_, Some(flag)) => Err(vec![Diagnostic::new(
                    "usage",
                    format!("`make` takes no option `{flag}`."),
                )
                .help("migrate make <NAME> [--snapshot-only]")]),
                ([name], None) => make(root, name, snapshot_only, report),
                _ => Err(vec![Diagnostic::new("usage", "`make` takes one name.")
                    .help("migrate make <NAME> [--snapshot-only], for example `migrate make add_posts`")]),
            }
        }
        [] | ["-h"] | ["--help"] | ["help"] => {
            report.say(USAGE);
            Ok(if args.is_empty() {
                ExitCode::from(2)
            } else {
                ExitCode::SUCCESS
            })
        }
        [other, ..] => Err(vec![Diagnostic::new(
            "usage",
            format!("`{other}` is not a command of this tool."),
        )
        .help("the commands are `ids`, `make <NAME>` and `sync`; `up`, `down` and `status` are the application's own `migrate` command")]),
    }
}

/// The migration files in [`MIGRATIONS`], as module names, oldest first.
///
/// A name starts with its UTC time, so sorting the names sorts by age.
pub fn stems(root: &Path) -> Result<Vec<String>, Vec<Diagnostic>> {
    let dir = root.join(MIGRATIONS);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&dir).map_err(|e| io_error(root, &dir, &e))?;
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".rs")?;
            let ident = stem.starts_with('m')
                && stem
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
            (ident && stem != "mod").then(|| stem.to_string())
        })
        .collect();
    out.sort();
    Ok(out)
}

/// Lists every migration file in `migrations/mod.rs`.
///
/// Rewritten whole while it still carries the digest the tool wrote. A
/// file somebody edited is left alone, with a warning that names the lines
/// it lacks: an edit here is a decision, and the tool does not overrule one
/// in silence.
pub fn sync(root: &Path, report: &mut Report) -> Result<(), Vec<Diagnostic>> {
    let stems = stems(root)?;
    let path = root.join(MIGRATOR);
    let fresh = migrator_file(&stems);
    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(root.join(MIGRATIONS))
                .map_err(|e| io_error(root, &root.join(MIGRATIONS), &e))?;
            write(root, &path, &fresh)
        }
        Err(e) => Err(io_error(root, &path, &e)),
        Ok(text) if text == fresh => Ok(()),
        Ok(text) if unedited(&text) == Some(true) => write(root, &path, &fresh),
        Ok(text) => {
            let missing: Vec<&str> = stems
                .iter()
                .filter(|stem| !text.contains(&format!("{stem}::Migration")))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                report.warn(format!(
                    "{MIGRATOR} was edited, so it was not rewritten. It does not list: {}",
                    missing.join(", ")
                ));
                report.warn(
                    "add `mod <name>;` and `Box::new(<name>::Migration),` for each, \
                     or delete the file and run `migrate sync`",
                );
            }
            Ok(())
        }
    }
}

fn make(
    root: &Path,
    name: &str,
    snapshot_only: bool,
    report: &mut Report,
) -> Result<ExitCode, Vec<Diagnostic>> {
    if !brocanth_db::stamp::is_snake_case(name) {
        return Err(vec![Diagnostic::new(
            "name",
            format!(
                "The name `{name}` is not snake case. Use lower-case letters, digits and `_`, \
                 and start with a letter."
            ),
        )
        .help(
            "write the name in snake case, for example `migrate make add_posts`",
        )]);
    }
    let models = read_models(root)?;
    let new = schema_of(&models).map_err(diagnostics)?;

    let snapshot = root.join(MIGRATIONS).join(SNAPSHOT);
    let old = match std::fs::read_to_string(&snapshot) {
        Ok(text) => Schema::from_json(&text).map_err(|e| {
            vec![
                Diagnostic::new("snapshot", format!("The snapshot cannot be read: {e}."))
                    .at(relative(root, &snapshot))
                    .help(format!(
                        "restore {MIGRATIONS}/{SNAPSHOT} from version control. If the \
                         migrations already match the models, delete it and run \
                         `migrate make <name> --snapshot-only` to write it again"
                    )),
            ]
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Schema::default(),
        Err(e) => return Err(io_error(root, &snapshot, &e)),
    };

    let changes = diff(&old, &new);
    if changes.is_empty() {
        report.say("No change since the last migration. No file is written.");
        return Ok(ExitCode::SUCCESS);
    }

    let dir = root.join(MIGRATIONS);
    std::fs::create_dir_all(&dir).map_err(|e| io_error(root, &dir, &e))?;

    if snapshot_only {
        write(root, &snapshot, &new.to_json())?;
        report.say(format!(
            "wrote {} and no migration",
            relative(root, &snapshot)
        ));
        return Ok(ExitCode::SUCCESS);
    }

    let stem = format!("m{}_{name}", brocanth_db::stamp::now());
    let file = dir.join(format!("{stem}.rs"));
    if file.exists() {
        return Err(vec![Diagnostic::new(
            "exists",
            format!(
                "The file {} exists. Use a different name.",
                relative(root, &file)
            ),
        )
        .help(
            "run `migrate make` again with another name, or wait one second",
        )]);
    }
    write(root, &file, &render(&stem, &changes))?;
    write(root, &snapshot, &new.to_json())?;

    for change in &changes {
        if change.destructive() {
            report.warn(change.describe());
        } else {
            report.say(format!("  {}", change.describe()));
        }
    }
    sync(root, report)?;
    report.say(format!("wrote {}", relative(root, &file)));
    report.say(format!(
        "  Read it, then run the application's `migrate up`. {MIGRATOR} lists it, and \
         {MIGRATIONS}/{SNAPSHOT} now records the models."
    ));
    Ok(ExitCode::SUCCESS)
}

fn ids(root: &Path, report: &mut Report) -> Result<ExitCode, Vec<Diagnostic>> {
    let mut problems = Vec::new();
    let mut total = 0;
    for path in model_files(root)? {
        let label = relative(root, &path);
        let text = std::fs::read_to_string(&path).map_err(|e| io_error(root, &path, &e))?;
        match stamp(&label, &text, crate::id::new_id) {
            Ok(stamped) => {
                if stamped.text != text {
                    write(root, &path, &stamped.text)?;
                }
                if !stamped.written.is_empty() {
                    total += stamped.written.len();
                    report.say(format!("  {label}  {}", stamped.written.join(", ")));
                }
            }
            Err(mut p) => problems.append(&mut p),
        }
    }
    if !problems.is_empty() {
        return Err(diagnostics(problems));
    }
    if total == 0 {
        report.say("Each model and field has an id. No file is changed.");
    } else {
        report.say(format!(
            "wrote {total} id{}",
            if total == 1 { "" } else { "s" }
        ));
    }
    Ok(ExitCode::SUCCESS)
}

/// Every model under [`MODELS`], or why there is none to read.
fn read_models(root: &Path) -> Result<Vec<Model>, Vec<Diagnostic>> {
    let mut models = Vec::new();
    let mut problems = Vec::new();
    for path in model_files(root)? {
        let label = relative(root, &path);
        let text = std::fs::read_to_string(&path).map_err(|e| io_error(root, &path, &e))?;
        match scan_source(&label, &text) {
            Ok(mut found) => models.append(&mut found),
            Err(mut p) => problems.append(&mut p),
        }
    }
    if !problems.is_empty() {
        return Err(diagnostics(problems));
    }
    Ok(models)
}

/// The `.rs` files under [`MODELS`], sorted, so two runs read in one order.
fn model_files(root: &Path) -> Result<Vec<PathBuf>, Vec<Diagnostic>> {
    let dir = root.join(MODELS);
    if !dir.is_dir() {
        return Err(vec![Diagnostic::new(
            "no-models",
            format!("The directory {MODELS} does not exist."),
        )
        .help(format!(
            "write a model in {MODELS}/, as the `brocanth_model` documentation shows"
        ))]);
    }
    let mut files = Vec::new();
    let mut stack = vec![dir];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| io_error(root, &dir, &e))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

fn diagnostics(problems: Vec<Problem>) -> Vec<Diagnostic> {
    let mut noted = false;
    problems
        .into_iter()
        .map(|p| {
            let mut d = Diagnostic::new(p.code, p.message).at(p.at);
            if d.code == "no-id" && !noted {
                noted = true;
                d.note = Some(
                    "the diff finds a field by its id. Without an id, a rename and a drop \
                     plus an add look the same"
                        .to_string(),
                );
            }
            d.help = Some(problem_help(&d.code).to_string());
            d
        })
        .collect()
}

/// What to do about a problem the scanner or the id stamper found.
fn problem_help(code: &str) -> &'static str {
    match code {
        "no-id" => "run `migrate ids` first",
        "duplicate-id" => "remove the copied `#[brocanth(id = ..)]` and run `migrate ids`",
        "bad-id" => "remove the attribute and run `migrate ids` to write a new id",
        "duplicate-table" => "give each model its own `table_name`, or delete the copy",
        "no-column-type" => {
            "give the field a type the scanner knows, or a `column_type` in `#[sea_orm(..)]`"
        }
        "shared-line" => "put the item on a line of its own, then run `migrate ids` again",
        "parse" => "fix the Rust syntax at that place, then run the command again",
        "attribute" => "write the attribute as `#[brocanth(id = \"..\")]`, with no other key",
        _ => "fix the source at that place, then run the command again",
    }
}

fn write(root: &Path, path: &Path, text: &str) -> Result<(), Vec<Diagnostic>> {
    std::fs::write(path, text).map_err(|e| io_error(root, path, &e))
}

fn io_error(root: &Path, path: &Path, e: &std::io::Error) -> Vec<Diagnostic> {
    vec![Diagnostic::new("io", format!("{e}"))
        .at(relative(root, path))
        .help("check that the path exists and that you can read and write it, then run the command again")]
}

/// `path` from `root`, with `/`, for messages and file labels.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_problem_has_a_help_line() {
        let problems = ["no-id", "no-id", "parse", "duplicate-id", "something-new"]
            .into_iter()
            .map(|code| Problem {
                code,
                at: "src/models/post.rs:1:1".to_string(),
                message: "m".to_string(),
            })
            .collect();
        for d in diagnostics(problems) {
            assert!(d.to_string().contains("\nhelp: "), "{d}");
        }
    }
}
