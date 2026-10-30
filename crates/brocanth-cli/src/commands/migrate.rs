//! `brocanth migrate` -- stable field ids and a rename-aware migration diff.
//!
//! Two subcommands, and neither touches a database:
//!
//! * `ids` writes `#[brocanth(id = "..")]` on every model and column field
//!   under `src/models/` that has none. It is the only writer of ids. A field
//!   keeps its id for life, so a rename keeps it too.
//! * `make <NAME>` compares the models with `migrations/schema.json` by id,
//!   writes `migrations/m<UTC time>_<NAME>.rs` and a new snapshot. It applies
//!   nothing: the generator proposes and the developer commits (G7).
//!
//! A model or a field with no id stops `make`. Without the id, a rename and a
//! drop plus an add look the same, and a guess here loses a column of data.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use brocanth_migrate::render::render;
use brocanth_migrate::scan::{scan_source, schema_of, Model};
use brocanth_migrate::stamp::stamp;
use brocanth_migrate::{diff, Problem, Schema};
use clap::{Args, Subcommand};

use crate::commands::Outcome;
use crate::ui::{paint, Diagnostic, Ui, ACCENT, DIM};

/// Where the models are, from the project root.
pub const MODELS: &str = "src/models";
/// Where the migrations and the snapshot go, from the project root.
pub const MIGRATIONS: &str = "migrations";
/// The snapshot, in [`MIGRATIONS`].
pub const SNAPSHOT: &str = "schema.json";

#[derive(Args)]
pub struct MigrateArgs {
    #[command(subcommand)]
    pub action: Action,
}

#[derive(Subcommand)]
pub enum Action {
    /// Write a migration for what changed in src/models since the last one
    ///
    /// Compares each model and field with migrations/schema.json by its id,
    /// so a rename is written as a rename. Writes the migration file and the
    /// new snapshot, and applies nothing.
    Make(MakeArgs),
    /// Write an id on each model and field in src/models that has none
    ///
    /// An id is written one time and never edited. Keep it on the field when
    /// you rename the field: that is how `make` sees a rename.
    Ids,
}

#[derive(Args)]
pub struct MakeArgs {
    /// The name of the migration, in snake case: add_posts, rename_title
    #[arg(value_name = "NAME")]
    pub name: String,
    /// Write the snapshot and no migration, for models that a hand-written
    /// migration already created
    #[arg(long)]
    pub snapshot_only: bool,
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: MigrateArgs) -> Outcome {
    let start = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().map_err(|e| {
            vec![Diagnostic::error(
                "cwd",
                format!("cannot read the current directory: {e}"),
            )]
        })?,
    };
    let root = super::delegate::project_root(&start)?;
    match args.action {
        Action::Make(make_args) => make(ui, &root, &make_args),
        Action::Ids => ids(ui, &root),
    }
}

fn make(ui: &Ui, root: &Path, args: &MakeArgs) -> Outcome {
    if !snake_case(&args.name) {
        return Err(vec![Diagnostic::error(
            "migrate/name",
            format!(
                "The name `{}` is not snake case. Use lower-case letters, digits and `_`, \
                 and start with a letter.",
                args.name
            ),
        )]);
    }
    let models = read_models(root)?;
    let new = schema_of(&models).map_err(diagnostics)?;

    let snapshot = root.join(MIGRATIONS).join(SNAPSHOT);
    let old = match std::fs::read_to_string(&snapshot) {
        Ok(text) => Schema::from_json(&text).map_err(|e| {
            vec![Diagnostic::error(
                "migrate/snapshot",
                format!("The snapshot cannot be read: {e}."),
            )
            .at(relative(root, &snapshot))]
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Schema::default(),
        Err(e) => return Err(io_error(root, &snapshot, &e)),
    };

    let changes = diff(&old, &new);
    if changes.is_empty() {
        ui.done("No change since the last migration. No file is written.");
        return Ok(ExitCode::SUCCESS);
    }

    let dir = root.join(MIGRATIONS);
    std::fs::create_dir_all(&dir).map_err(|e| io_error(root, &dir, &e))?;

    if args.snapshot_only {
        write(root, &snapshot, &new.to_json())?;
        ui.done(format!(
            "wrote {} and no migration",
            paint(ACCENT, relative(root, &snapshot))
        ));
        return Ok(ExitCode::SUCCESS);
    }

    let stem = format!("m{}_{}", utc_stamp(), args.name);
    let file = dir.join(format!("{stem}.rs"));
    if file.exists() {
        return Err(vec![Diagnostic::error(
            "migrate/exists",
            format!(
                "The file {} exists. Use a different name.",
                relative(root, &file)
            ),
        )]);
    }
    write(root, &file, &render(&stem, &changes))?;
    write(root, &snapshot, &new.to_json())?;

    for change in &changes {
        let line = format!("  {}", change.describe());
        if change.destructive() {
            ui.warn(line.trim_start());
        } else {
            ui.note(line);
        }
    }
    ui.gap();
    ui.done(format!("wrote {}", paint(ACCENT, relative(root, &file))));
    ui.note(paint(
        DIM,
        format!(
            "  Read it, then add `{stem}::Migration` to your Migrator. \
             {MIGRATIONS}/{SNAPSHOT} now records the models."
        ),
    ));
    Ok(ExitCode::SUCCESS)
}

fn ids(ui: &Ui, root: &Path) -> Outcome {
    let mut problems = Vec::new();
    let mut total = 0;
    for path in model_files(root)? {
        let label = relative(root, &path);
        let text = std::fs::read_to_string(&path).map_err(|e| io_error(root, &path, &e))?;
        match stamp(&label, &text, brocanth_migrate::id::new_id) {
            Ok(stamped) => {
                if stamped.text != text {
                    write(root, &path, &stamped.text)?;
                }
                if !stamped.written.is_empty() {
                    total += stamped.written.len();
                    ui.note(format!(
                        "  {}  {}",
                        paint(ACCENT, &label),
                        stamped.written.join(", ")
                    ));
                }
            }
            Err(mut p) => problems.append(&mut p),
        }
    }
    if !problems.is_empty() {
        return Err(diagnostics(problems));
    }
    if total == 0 {
        ui.done("Each model and field has an id. No file is changed.");
    } else {
        ui.done(format!(
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
        return Err(vec![Diagnostic::error(
            "migrate/no-models",
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
            let d = Diagnostic::error(format!("migrate/{}", p.code), p.message).at(p.at);
            if p.code == "no-id" && !noted {
                noted = true;
                d.note(
                    "the diff finds a field by its id. Without an id, a rename and a drop \
                     plus an add look the same",
                )
            } else {
                d
            }
        })
        .collect()
}

fn write(root: &Path, path: &Path, text: &str) -> Result<(), Vec<Diagnostic>> {
    std::fs::write(path, text).map_err(|e| io_error(root, path, &e))
}

fn io_error(root: &Path, path: &Path, e: &std::io::Error) -> Vec<Diagnostic> {
    vec![Diagnostic::error("io", format!("{e}")).at(relative(root, path))]
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

fn snake_case(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `YYYYMMDD_HHMMSS` in UTC, the stem SeaORM's own generator uses.
fn utc_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    stamp_of(secs)
}

fn stamp_of(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}_{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_utc_in_seaorms_shape() {
        assert_eq!(stamp_of(0), "19700101_000000");
        // 2026-10-31T14:05:09Z
        assert_eq!(stamp_of(1_793_455_509), "20261031_140509");
        // 2024-02-29T23:59:59Z, a leap day.
        assert_eq!(stamp_of(1_709_251_199), "20240229_235959");
    }

    #[test]
    fn a_name_is_snake_case() {
        assert!(snake_case("rename_title"));
        assert!(snake_case("add_posts2"));
        assert!(!snake_case("RenameTitle"));
        assert!(!snake_case("2fast"));
        assert!(!snake_case("rename-title"));
        assert!(!snake_case(""));
    }
}
