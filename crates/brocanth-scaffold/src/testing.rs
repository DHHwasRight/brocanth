//! Tests for a module crate: format a generated project, compare it with a
//! golden file, and build and test it.
//!
//! On under the `testing` feature. A module crate takes it as a
//! dev-dependency:
//!
//! ```toml
//! [dev-dependencies]
//! brocanth-scaffold = { version = "0.1", features = ["testing"] }
//! brocanth-cli = { version = "0.1" }
//! ```
//!
//! The generator lives in the CLI, so the project comes from
//! `brocanth_cli::testing::generate`, which takes the module under test as a
//! `&dyn Module` beside the keys the CLI has. This module holds the checks,
//! so they need nothing but `std`, `rustfmt` and `cargo`:
//!
//! ```ignore
//! use brocanth_scaffold::testing;
//!
//! #[test]
//! fn the_module_writes_this_project() {
//!     let files = brocanth_cli::testing::generate(
//!         "my-blog",
//!         &["config", "robots"],
//!         &[&brocanth_robots::scaffold::Robots],
//!     )
//!     .expect("the project generates");
//!     testing::assert_formatted(&files);
//!     testing::golden(
//!         concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/robots.txt"),
//!         &testing::snapshot(&files),
//!     );
//!     if testing::building() {
//!         // Path dependencies on the checkout, so it builds the code under
//!         // test.
//!         let files = brocanth_cli::testing::generate_for_build(
//!             "my-blog",
//!             &["config", "robots"],
//!             &[&brocanth_robots::scaffold::Robots],
//!         )
//!         .expect("the project generates");
//!         testing::build(&files).expect("the project builds and passes its tests");
//!     }
//! }
//! ```
//!
//! Two environment variables steer it:
//!
//! * `BROCANTH_BLESS=1` writes the golden file instead of comparing.
//! * `BROCANTH_BUILD=1` makes [`building`] true, so a test builds the
//!   project and runs its own tests. Off by default: a build fetches and
//!   compiles the framework, which takes minutes.
//! * `BROCANTH_BUILD_TARGET=<dir>` gives every build one target directory,
//!   so the framework compiles once for all of them. Unset, each project
//!   has its own, deleted with it.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A generated project: `(path, contents)` per file, in the order the
/// generator wrote them.
pub type Files = [(String, String)];

/// One project as one string: every file, with its path above it.
///
/// The same shape as the CLI's own goldens, so a diff of one file shows a
/// line that moves between two generated files as one change.
pub fn snapshot(files: &Files) -> String {
    let mut out = String::new();
    for (path, contents) in files {
        out.push_str(&format!("===== {path} =====\n{contents}"));
        if !contents.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Whether this run blesses goldens: `BROCANTH_BLESS` is set and not empty.
pub fn blessing() -> bool {
    std::env::var("BROCANTH_BLESS").is_ok_and(|v| !v.is_empty())
}

/// Whether this run builds and tests generated projects: `BROCANTH_BUILD`
/// is set and not empty.
pub fn building() -> bool {
    std::env::var("BROCANTH_BUILD").is_ok_and(|v| !v.is_empty())
}

/// Compares `actual` with the golden file at `path`, or writes it when
/// [`blessing`].
///
/// # Panics
///
/// When the file is missing or differs. The message names the first line
/// that differs and says how to bless.
pub fn golden(path: impl AsRef<Path>, actual: &str) {
    let path = path.as_ref();
    if blessing() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .unwrap_or_else(|e| panic!("cannot create {}: {e}", dir.display()));
        }
        std::fs::write(path, actual)
            .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        return;
    }
    let Ok(expected) = std::fs::read_to_string(path) else {
        panic!(
            "error: the golden file {} does not exist\n\
             help: run the test again with BROCANTH_BLESS=1, then read the file",
            path.display()
        );
    };
    if expected == actual {
        return;
    }
    let (line, want, got) = expected
        .lines()
        .map(Some)
        .chain(std::iter::repeat(None))
        .zip(actual.lines().map(Some).chain(std::iter::repeat(None)))
        .take_while(|(a, b)| a.is_some() || b.is_some())
        .enumerate()
        .find(|(_, (a, b))| a != b)
        .map(|(at, (a, b))| (at + 1, a.unwrap_or("<end>"), b.unwrap_or("<end>")))
        .unwrap_or((0, "", ""));
    panic!(
        "error: the generated project differs from {} at line {line}\n\
         \x20 golden:    {want}\n\
         \x20 generated: {got}\n\
         help: if the change is intended, run with BROCANTH_BLESS=1 and read the diff \
         before you commit it",
        path.display()
    );
}

/// The Rust files in `files` that rustfmt would change, or cannot parse.
///
/// `None` when rustfmt is not on `PATH`.
///
/// Through `--emit stdout` and a comparison, not `--check`: `--check` exits
/// 0 with an empty output for a file it cannot parse, which reads as a
/// formatted file.
pub fn unformatted(files: &Files) -> Option<Vec<String>> {
    Command::new("rustfmt").arg("--version").output().ok()?;
    let mut wrong = Vec::new();
    for (path, source) in files {
        if !path.ends_with(".rs") {
            continue;
        }
        match rustfmt(source) {
            Ok(formatted) if formatted == *source => {}
            Ok(_) => wrong.push(path.clone()),
            Err(()) => wrong.push(format!("{path} (rustfmt cannot parse it)")),
        }
    }
    Some(wrong)
}

/// `source` through rustfmt, with the edition the generated projects use.
fn rustfmt(source: &str) -> Result<String, ()> {
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2021", "--emit", "stdout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ())?;
    child
        .stdin
        .take()
        .ok_or(())?
        .write_all(source.as_bytes())
        .map_err(|_| ())?;
    let done = child.wait_with_output().map_err(|_| ())?;
    if !done.status.success() {
        return Err(());
    }
    let out = String::from_utf8_lossy(&done.stdout);
    // Some rustfmt builds label the stream with `<stdin>:` first.
    Ok(out.strip_prefix("<stdin>:\n").unwrap_or(&out).to_string())
}

/// Checks that every Rust file in `files` passes its first `cargo fmt`
/// unchanged.
///
/// Prints a line and passes when rustfmt is not on `PATH`, so a machine
/// without it still runs the other checks.
///
/// # Panics
///
/// When a file is not formatted. The message names each file.
pub fn assert_formatted(files: &Files) {
    match unformatted(files) {
        None => eprintln!("skipped: rustfmt is not on PATH"),
        Some(wrong) if wrong.is_empty() => {}
        Some(wrong) => panic!(
            "error: rustfmt changes these generated files:\n  {}\n\
             help: write the text in the shape rustfmt gives it; for a hooking \
             plugin, use `snippet::hooking_line`",
            wrong.join("\n  ")
        ),
    }
}

/// Writes `files` under `dir`, creating the directories they need.
pub fn write(dir: &Path, files: &Files) -> std::io::Result<()> {
    for (path, contents) in files {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(full, contents)?;
    }
    Ok(())
}

/// Writes the project into a fresh directory under the system's temporary
/// directory and runs `cargo test` in it: cargo builds the project, then
/// runs the tests the generator wrote into it. A build alone proves the
/// code compiles, not that the project passes its own tests. The directory
/// is deleted when the tests pass, and kept for a look when they fail.
///
/// The project must name its brocanth crates by path or by a source cargo
/// can reach. `brocanth_cli::testing::generate_for_build` writes path
/// dependencies on the checkout it was compiled in.
///
/// # Errors
///
/// When a file cannot be written, the project does not build, or a test
/// fails. The error holds cargo's output.
pub fn build(files: &Files) -> Result<(), String> {
    let name = files
        .iter()
        .find(|(path, _)| path == "Cargo.toml")
        .and_then(|(_, text)| {
            text.lines()
                .find_map(|line| line.strip_prefix("name = \""))
                .and_then(|rest| rest.strip_suffix('"'))
                .map(str::to_string)
        })
        .unwrap_or_else(|| "project".to_string());
    // Tests run in parallel in one process, so the process id alone names
    // one directory for all of them: a counter keeps each call apart.
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "brocanth-testing-{name}-{}-{}",
        std::process::id(),
        CALLS.fetch_add(1, Ordering::Relaxed)
    ));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    write(&dir, files).map_err(|e| format!("{}: {e}", dir.display()))?;
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    // Not the target directory of the test that runs it, so the build does
    // not wait on that lock: the project's own, or `BROCANTH_BUILD_TARGET`.
    let mut command = Command::new(cargo);
    command
        .args(["test", "--quiet"])
        .current_dir(&dir)
        .env_remove("CARGO_TARGET_DIR");
    if let Some(target) = std::env::var_os("BROCANTH_BUILD_TARGET").filter(|t| !t.is_empty()) {
        command.env("CARGO_TARGET_DIR", target);
    }
    let out = command.output().map_err(|e| format!("cargo: {e}"))?;
    if out.status.success() {
        // A target directory of a few gigabytes per project.
        let _ = std::fs::remove_dir_all(&dir);
        Ok(())
    } else {
        Err(format!(
            "error: the generated project in {} does not build or fails its tests\n{}{}\n\
             help: open the directory and run `cargo test` to read the error in place",
            dir.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn a_snapshot_puts_each_path_above_its_file() {
        let text = snapshot(&files(&[("a.rs", "fn a() {}"), ("b.toml", "x = 1\n")]));
        assert_eq!(
            text,
            "===== a.rs =====\nfn a() {}\n===== b.toml =====\nx = 1\n"
        );
    }

    #[test]
    fn the_formatting_check_names_a_file_rustfmt_changes_or_cannot_read() {
        let Some(wrong) = unformatted(&files(&[
            ("good.rs", "fn main() {}\n"),
            ("bad.rs", "fn main(){}\n"),
            ("broken.rs", "fn main() { }}\n"),
            ("not-rust.toml", "fn main(){}\n"),
        ])) else {
            eprintln!("skipped: rustfmt is not on PATH");
            return;
        };
        assert_eq!(wrong, ["bad.rs", "broken.rs (rustfmt cannot parse it)"]);
    }

    #[test]
    fn every_hooking_line_shape_is_what_rustfmt_writes() {
        // The statement lands inside a function body at 8 columns, so it is
        // checked there. A short type takes the folded form, a long one
        // the broken form, and a chain keeps its own breaks.
        for (ty, config) in [
            ("a::B", "a::BConfig::default()"),
            (
                "brocanth_correlation::Correlation",
                "brocanth_correlation::CorrelationConfig::default()",
            ),
            (
                "brocanth_correlation::Correlation",
                "brocanth_correlation::CorrelationConfig::default()\n    \
                 .header(\"x-request-id-from-the-proxy\")\n    \
                 .generate(true)",
            ),
        ] {
            let source = format!(
                "fn f() {{\n    {{\n{}    }}\n}}\n",
                crate::snippet::hooking_line(ty, config)
            );
            if let Some(wrong) = unformatted(&files(&[("f.rs", &source)])) {
                assert!(wrong.is_empty(), "rustfmt changes:\n{source}");
            }
        }
    }
}
