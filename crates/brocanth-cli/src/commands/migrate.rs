//! `brocanth migrate` -- stable field ids, a rename-aware migration diff,
//! and the commands that apply the result.
//!
//! Two subcommands read and write text, and touch no database:
//!
//! * `ids` writes `#[brocanth(id = "..")]` on every model and column field
//!   under `src/models/` that has none. It is the only writer of ids. A field
//!   keeps its id for life, so a rename keeps it too.
//! * `make <NAME>` compares the models with `migrations/schema.json` by id,
//!   writes `migrations/m<UTC time>_<NAME>.rs` and a new snapshot, and lists
//!   the file in `migrations/mod.rs`. It applies nothing: the generator
//!   proposes and the developer commits (G7).
//!
//! Three apply it, through the project's own binary, the way every project
//! command runs: `up`, `down [N]` and `status`. The global `brocanth` cannot
//! know a project's migrations; they are compiled into its binary. Each of
//! the three first lists every `migrations/m*.rs` in `migrations/mod.rs`, so
//! a file written by hand is applied too.
//!
//! A model or a field with no id stops `make`. Without the id, a rename and a
//! drop plus an add look the same, and a guess here loses a column of data.

use std::ffi::OsString;
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
    /// Apply each pending migration in migrations/, oldest first
    ///
    /// Runs `migrate up` in this project's own binary, which reads `[model]
    /// url` the way the server does. Needs the `model` and `commands`
    /// modules.
    Up,
    /// Roll back the newest N applied migrations
    Down {
        /// How many migrations to roll back
        #[arg(value_name = "N", default_value_t = 1)]
        n: u32,
    },
    /// List each migration in migrations/ as applied or pending
    Status,
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
        Action::Up => apply(ui, &root, &["up"]),
        Action::Down { n } => apply(ui, &root, &["down", &n.to_string()]),
        Action::Status => apply(ui, &root, &["status"]),
    }
}

/// `up`, `down` and `status`: list the files, then hand over to the
/// project's binary.
fn apply(ui: &Ui, root: &Path, args: &[&str]) -> Outcome {
    if super::delegate::has_module(root, "model") == Some(false) {
        return Err(vec![Diagnostic::error(
            "migrate/no-model",
            "This project has no `model` module, so it has no database to migrate.",
        )
        .help("`brocanth add model` adds one")]);
    }
    sync(ui, root)?;
    let mut handed: Vec<OsString> = vec!["migrate".into()];
    handed.extend(args.iter().map(OsString::from));
    super::delegate::run(ui, Some(root), handed)
}

/// The migration files in [`MIGRATIONS`], as module names, oldest first.
///
/// A name starts with its UTC time, so sorting the names sorts by age.
fn stems(root: &Path) -> Result<Vec<String>, Vec<Diagnostic>> {
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
/// Rewritten whole while it still carries the digest the generator wrote.
/// A file somebody edited is left alone, with a warning that names the
/// lines it lacks: an edit here is a decision, and the generator does not
/// overrule one in silence.
fn sync(ui: &Ui, root: &Path) -> Result<(), Vec<Diagnostic>> {
    let stems = stems(root)?;
    let path = root.join(crate::owned::MIGRATOR);
    let fresh = migrator_file(&stems);
    match std::fs::read_to_string(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(root.join(MIGRATIONS))
                .map_err(|e| io_error(root, &root.join(MIGRATIONS), &e))?;
            write(root, &path, &fresh)
        }
        Err(e) => Err(io_error(root, &path, &e)),
        Ok(text) if text == fresh => Ok(()),
        Ok(text) if crate::owned::unedited(&text) == Some(true) => write(root, &path, &fresh),
        Ok(text) => {
            let missing: Vec<&String> = stems
                .iter()
                .filter(|stem| !text.contains(&format!("{stem}::Migration")))
                .collect();
            if !missing.is_empty() {
                ui.warn(format!(
                    "{} was edited, so it was not rewritten. It does not list: {}",
                    crate::owned::MIGRATOR,
                    missing
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                ui.note(paint(
                    DIM,
                    "  Add `mod <name>;` and `Box::new(<name>::Migration),` for each, \
                     or delete the file and run this command again.",
                ));
            }
            Ok(())
        }
    }
}

/// `migrations/mod.rs`, stamped, for these migration files.
pub fn migrator_file(stems: &[String]) -> String {
    crate::owned::stamp(crate::owned::MIGRATOR, &migrator_rs(stems))
}

/// `migrations/mod.rs`, before the stamp.
///
/// Written in a shape rustfmt keeps, so a project's own `cargo fmt` leaves
/// the file and its digest alone.
fn migrator_rs(stems: &[String]) -> String {
    let mods: String = stems.iter().map(|stem| format!("mod {stem};\n")).collect();
    let items: Vec<String> = stems
        .iter()
        .map(|stem| format!("Box::new({stem}::Migration)"))
        .collect();
    // One element per line, under a comment. rustfmt keeps a list with a
    // comment in it vertical, so the shape does not depend on how long the
    // names are, and each new migration is a one-line diff.
    let list = if items.is_empty() {
        "        vec![]\n".to_string()
    } else {
        let mut out = String::from("        vec![\n            // Oldest first.\n");
        for item in &items {
            out.push_str(&format!("            {item},\n"));
        }
        out.push_str("        ]\n");
        out
    };
    format!(
        "//! The migrations in this directory, oldest first.\n\
         //!\n\
         //! A file name starts with its UTC time, so the order of the names is the\n\
         //! order the migrations were written. `src/commands/migrate.rs` applies this\n\
         //! list, and `src/main.rs` reaches this file through `#[path]`.\n\
         \n\
         use brocanth_model::sea_orm_migration::prelude::*;\n\
         \n\
         {mods}{gap}\
         /// Every migration this binary applies, oldest first.\n\
         pub struct Migrator;\n\
         \n\
         #[async_trait::async_trait]\n\
         impl MigratorTrait for Migrator {{\n\
        \x20   fn migrations() -> Vec<Box<dyn MigrationTrait>> {{\n\
         {list}\
        \x20   }}\n\
         }}\n",
        gap = if stems.is_empty() { "" } else { "\n" },
    )
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
    sync(ui, root)?;
    ui.gap();
    ui.done(format!("wrote {}", paint(ACCENT, relative(root, &file))));
    ui.note(paint(
        DIM,
        format!(
            "  Read it, then run `brocanth migrate up`. {} lists it, and \
             {MIGRATIONS}/{SNAPSHOT} now records the models.",
            crate::owned::MIGRATOR
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

    /// The list in every shape it takes, through rustfmt: empty, one short
    /// name, one long name, and several. A project's own `cargo fmt` must
    /// leave the file alone, or the next `make` finds it "edited".
    #[test]
    fn the_migration_list_is_already_formatted() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        if Command::new("rustfmt").arg("--version").output().is_err() {
            eprintln!("SKIPPED: rustfmt is not on PATH");
            return;
        }
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for stems in [
            names(&[]),
            names(&["m20261030_120000_a"]),
            names(&["m20261030_120000_create_posts_and_comments"]),
            names(&["m20261030_120000_create_posts_and_comments_and_tags"]),
            names(&[
                "m20261030_120000_create_posts",
                "m20261031_090000_rename_title",
                "m20261101_100000_add_tags",
            ]),
        ] {
            let text = migrator_rs(&stems);
            let mut child = Command::new("rustfmt")
                .args(["--edition", "2021", "--emit", "stdout"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("rustfmt");
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&out.stdout), text, "{stems:?}");
        }
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
