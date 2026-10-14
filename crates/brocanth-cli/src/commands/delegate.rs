//! Everything that is not a built-in: handed to the project's own binary.
//!
//! `brocanth` is installed once, globally. The commands it can run are defined in
//! a project and compiled into that project's binary, so this command's whole
//! job is to find that binary, make sure it is current, and get out of the
//! way. It is the same shape as `rails` handing off to `bin/rails`.
//!
//! Four steps, and the order matters:
//!
//! 1. Walk up for `brocanth.toml`. Without it there is no project, and
//!    guessing would run the wrong one.
//! 2. `cargo build`, with cargo's stderr straight through. A command that was
//!    edited a second ago must be the one that runs, and cargo's progress and
//!    errors are better than anything this could paraphrase.
//! 3. Ask `cargo metadata` where the binary is. Guessing `target/debug/<name>`
//!    is wrong under `CARGO_TARGET_DIR`, in a workspace, and on any profile
//!    but the default.
//! 4. Replace this process with it, so signals, exit codes and the terminal
//!    all belong to the command rather than to a wrapper holding its hand.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::commands::Outcome;
use crate::ui::{Diagnostic, Ui, ACCENT};

/// The marker that says "a project starts here".
const MARKER: &str = "brocanth.toml";

pub fn run(ui: &Ui, dir: Option<&Path>, args: Vec<OsString>) -> Outcome {
    let start = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().map_err(|e| {
            vec![Diagnostic::error(
                "cwd",
                format!("cannot read the current directory: {e}"),
            )]
        })?,
    };

    let root = project_root(&start)?;
    ui.note(format!(
        "{} {}",
        crate::ui::paint(ACCENT, root.display()),
        crate::ui::paint(crate::ui::DIM, "(brocanth.toml)")
    ));

    build(&root)?;
    let binary = binary_path(&root)?;
    ui.debug(format!(
        "exec {} {}",
        binary.display(),
        args.iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
    ));
    handoff(&binary, &args)
}

/// The nearest ancestor of `start` holding [`MARKER`].
fn project_root(start: &Path) -> Result<PathBuf, Vec<Diagnostic>> {
    start
        .ancestors()
        .find(|dir| dir.join(MARKER).is_file())
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            vec![Diagnostic::error(
                "no-project",
                format!("no `{MARKER}` here or in any parent directory"),
            )
            .note(format!("looked upward from {}", start.display()))
            .help("`brocanth create <NAME>` makes a project, or cd into one")]
        })
}

/// Builds the project, with cargo's own output passed through.
fn build(root: &Path) -> Result<(), Vec<Diagnostic>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let status = Command::new(&cargo)
        .args(["build", "--quiet"])
        .current_dir(root)
        // Inherited rather than captured: a compile error belongs on the
        // screen as cargo wrote it, with its colours and its spans, not
        // summarised by a second tool.
        .stderr(Stdio::inherit())
        .stdout(Stdio::inherit())
        .status()
        .map_err(|e| {
            vec![Diagnostic::error("cargo", format!("cannot run cargo: {e}"))
                .help("cargo has to be on PATH, or $CARGO has to name it")]
        })?;

    if !status.success() {
        // No message: cargo has already said everything useful, on the stderr
        // this command just inherited. Repeating it would only bury it.
        return Err(vec![Diagnostic::error(
            "build",
            "the project does not compile, so no command was run",
        )]);
    }
    Ok(())
}

/// Where cargo actually put the binary.
fn binary_path(root: &Path) -> Result<PathBuf, Vec<Diagnostic>> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .no_deps()
        .exec()
        .map_err(|e| {
            vec![Diagnostic::error(
                "metadata",
                format!("cannot read cargo metadata: {e}"),
            )]
        })?;

    // By manifest path, not `root_package`: that one means "the workspace
    // root", and a project inside a workspace is a member, so it would come
    // back empty for exactly the layout the examples use.
    let manifest = root.join("Cargo.toml");
    let package = metadata
        .packages
        .iter()
        .find(|pkg| Path::new(pkg.manifest_path.as_str()) == manifest)
        .ok_or_else(|| {
            vec![Diagnostic::error(
                "no-package",
                format!("{MARKER} is here, but its directory is not a cargo package"),
            )
            .note("a brocanth project is a cargo package whose binary is the CLI")]
        })?;

    // `manage` first, then whatever single binary there is. A project built
    // by `brocanth create` has two -- the application and the command runner --
    // and `cargo metadata` lists them in manifest order, which is not a
    // promise. Picking by name makes the choice ours rather than cargo's.
    let bins: Vec<_> = package.targets.iter().filter(|t| t.is_bin()).collect();
    let bin = bins
        .iter()
        .find(|t| t.name == "manage")
        .or_else(|| bins.first())
        .ok_or_else(|| {
            vec![Diagnostic::error(
                "no-binary",
                format!("`{}` has no binary target", package.name),
            )
            .help("a brocanth project needs a `src/main.rs` calling `brocanth_command::main`")]
        })?;

    // From metadata, so `CARGO_TARGET_DIR`, a workspace root and a shared
    // target directory are all already accounted for.
    let path = metadata.target_directory.join("debug").join(&bin.name);
    let path = PathBuf::from(path);
    if !path.is_file() {
        return Err(vec![Diagnostic::error(
            "missing-binary",
            format!("{} was not produced by the build", path.display()),
        )]);
    }
    Ok(path)
}

/// Hands over to the project binary.
///
/// On Unix this replaces the process, so the command owns the terminal, its exit
/// code is the one the shell sees, and Ctrl-C reaches it rather than reaching
/// a wrapper that has to decide what to do about it. `exec` only returns on
/// failure, which is why the error path is after the call.
#[cfg(unix)]
fn handoff(binary: &Path, args: &[OsString]) -> Outcome {
    use std::os::unix::process::CommandExt;

    let error = Command::new(binary).args(args).exec();
    Err(vec![Diagnostic::error(
        "exec",
        format!("cannot run {}: {error}", binary.display()),
    )])
}

/// The fallback where `exec` does not exist: run it and copy the exit code.
#[cfg(not(unix))]
fn handoff(binary: &Path, args: &[OsString]) -> Outcome {
    let status = Command::new(binary).args(args).status().map_err(|e| {
        vec![Diagnostic::error(
            "spawn",
            format!("cannot run {}: {e}", binary.display()),
        )]
    })?;
    match status.code() {
        Some(code) => Ok(std::process::ExitCode::from(
            u8::try_from(code).unwrap_or(1),
        )),
        // Killed by a signal. There is no exit code to copy, and reporting
        // success would be a lie.
        None => Ok(std::process::ExitCode::FAILURE),
    }
}

/// The commands a project defines, for `brocanth --help` to list.
///
/// Read out of the project binary's own `--help`, because that is the only
/// place the list exists -- the commands are compiled into that binary and
/// this tool cannot know them. There is deliberately no dedicated listing
/// command to ask instead: it would be a second way to see what `--help`
/// already shows, and a name a project could not then use for its own.
///
/// This does **not** build. A `--help` that compiles a project is a `--help`
/// nobody runs twice, and the list is only decoration: every command still
/// works through [`run`], which does build. An unbuilt project gets the plain
/// help, and a command added since the last build runs anyway.
pub fn listed(dir: Option<&Path>) -> Option<Vec<(String, String)>> {
    let start = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().ok()?,
    };
    let root = project_root(&start).ok()?;
    let binary = binary_path(&root).ok()?;

    let out = Command::new(&binary)
        .arg("--help")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let listed = parse_help(&String::from_utf8_lossy(&out.stdout));
    (!listed.is_empty()).then_some(listed)
}

/// The `Commands:` block of a clap help page, as name and description.
///
/// Split out so it can be tested against a recorded page rather than against
/// whatever happens to be built.
fn parse_help(help: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in help.lines() {
        if line.trim_end() == "Commands:" {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        // The block ends at the first line that is not indented. A blank line
        // does not end it: clap separates long entries with one.
        if !line.starts_with("  ") {
            if line.trim().is_empty() {
                continue;
            }
            break;
        }
        let Some((name, about)) = line.trim().split_once("  ") else {
            continue;
        };
        let name = name.trim();
        // `help` is clap's, not the project's, and `brocanth` has its own.
        if name.is_empty() || name == "help" {
            continue;
        }
        out.push((name.to_string(), about.trim().to_string()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real page, pasted, so the parser is tested against clap's output
    /// rather than against an idea of it.
    const HELP: &str = "\
A brocanth application.

Usage: blog [OPTIONS] <COMMAND>

Commands:
  serve  Serve the application over HTTP
  ping   Answer pong
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
";

    #[test]
    fn the_command_block_is_read_and_nothing_else_is() {
        let listed = parse_help(HELP);
        assert_eq!(
            listed,
            vec![
                (
                    "serve".to_string(),
                    "Serve the application over HTTP".to_string()
                ),
                ("ping".to_string(), "Answer pong".to_string()),
            ],
            "`help` is clap's own and the options block is not a command list"
        );
    }

    #[test]
    fn a_help_page_with_no_commands_yields_nothing() {
        let listed = parse_help("Usage: blog [OPTIONS]\n\nOptions:\n  -h, --help  Print help\n");
        assert!(listed.is_empty(), "{listed:?}");
    }

    #[test]
    fn the_root_is_the_nearest_ancestor_holding_the_marker() {
        let base = std::env::temp_dir().join(format!("brocanth-delegate-{}", std::process::id()));
        let deep = base.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(base.join(MARKER), "[project]\n").unwrap();

        let found = project_root(&deep).expect("walks up to the marker");
        assert_eq!(
            found.canonicalize().unwrap(),
            base.canonicalize().unwrap(),
            "it should stop at the directory holding the marker"
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn no_marker_anywhere_says_so_and_says_what_to_do() {
        // The root directory has no brocanth.toml, and walking up from it
        // terminates immediately.
        let error = project_root(Path::new("/")).expect_err("there is no project at /");
        let rendered = error[0].render();
        assert!(rendered.contains("no `brocanth.toml`"), "{rendered}");
        assert!(rendered.contains("brocanth create"), "{rendered}");
    }
}
