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
//!   has its own, deleted with it. The builds of one process then run one
//!   at a time; do not point two processes at the same directory.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

/// The manifest rules every module must keep, checked on `module`: what
/// `brocanth create` would refuse, and what it cannot see.
///
/// It runs the module twice, once in a project of every key in `known`
/// (pass the keys the CLI has, and the module's own) and once with only the
/// module, what it needs and its parent, so each `has` branch is reached.
/// Then it checks that the manifest names a category, that each slot written
/// is open to one ([`crate::Scaffold::check_slots`]), that each `has` asked
/// is declared with `adapts_to`, `needs` or `parent`, and that each
/// `adapts_to` is asked. The same two-way rule holds for Kinds: each
/// `project.provider::<K>()` asked is declared with `uses` or `requires`,
/// and each `uses` is asked. In the first run every Kind the module names
/// is bound; in the second only what it requires. Each returned line is one
/// broken rule; empty means none.
///
/// ```ignore
/// let known: Vec<&str> = brocanth_cli::testing::modules()
///     .iter()
///     .map(|m| m.manifest())
///     .filter(|m| m.blocked.is_none())
///     .map(|m| m.key)
///     .collect();
/// let wrong = brocanth_scaffold::testing::check_module(&MyModule, &known);
/// assert!(wrong.is_empty(), "{}", wrong.join("\n"));
/// ```
pub fn check_module(module: &dyn crate::Module, known: &[&str]) -> Vec<String> {
    let m = module.manifest();
    let mut wrong = Vec::new();
    if m.categories.is_empty() {
        wrong.push(format!(
            "`{}` has no category: add `.category(Category::..)` to its manifest",
            m.key
        ));
    }
    if m.blocked.is_some() {
        return wrong;
    }
    let mut everything: Vec<&str> = known.to_vec();
    if !everything.contains(&m.key) {
        everything.push(m.key);
    }
    let mut alone = vec![m.key];
    alone.extend(m.needs.iter().map(|(need, _)| *need));
    alone.extend(m.parent);

    wrong.extend(kind_declarations(&m, &everything));
    // The provider's key is not known here, and a module must not depend on
    // it: it asks whether the Kind is bound, not who binds it.
    const SOME_PROVIDER: &str = "(provider)";
    let mut all_bound: Vec<(&str, &str)> = m
        .may_ask_kinds()
        .into_iter()
        .map(|kind| (kind, SOME_PROVIDER))
        .collect();
    let mut required: Vec<(&str, &str)> = m
        .requirements(&alone)
        .map(|r| (r.kind.name, SOME_PROVIDER))
        .collect();
    // The scaffold reads the file layer for every module, declared or not:
    // bound in the first run and not in the second, so both registrations
    // are checked.
    {
        use crate::kind::Kind as _;
        if !all_bound
            .iter()
            .any(|(kind, _)| *kind == crate::kind::ConfigSource::NAME)
        {
            all_bound.push((crate::kind::ConfigSource::NAME, SOME_PROVIDER));
        }
    }
    for (kind, _) in &m.provides {
        all_bound.push((kind.name, m.key));
        required.push((kind.name, m.key));
    }

    let mut asked: Vec<String> = Vec::new();
    let mut asked_kinds: Vec<&str> = Vec::new();
    for (on, bound) in [(&everything, &all_bound), (&alone, &required)] {
        let project =
            crate::Project::new("my-blog", "My Blog", on, &everything).with_bindings(bound);
        let mut s = crate::Scaffold::new();
        s.now_writing(m.key);
        module.contribute(&project, &mut s);
        s.check_slots(m.key, &m.categories);
        wrong.extend(s.problems().iter().cloned());
        asked.extend(project.asked_keys());
        asked_kinds.extend(project.asked_kinds());
    }
    asked_kinds.sort_unstable();
    asked_kinds.dedup();
    let may_kinds = m.may_ask_kinds();
    for kind in &asked_kinds {
        if !may_kinds.contains(kind) && !m.gives(kind) {
            wrong.push(format!(
                "`{}` asks `project.provider::<{kind}>()` and does not declare it: \
                 add `.uses::<{kind}>(\"<what changes>\")` to its manifest",
                m.key
            ));
        }
    }
    for (kind, _) in &m.uses {
        if !asked_kinds.contains(&kind.name) {
            wrong.push(format!(
                "`{}` declares `uses::<{kind}>()` and never asks `project.provider::<{kind}>()`",
                m.key
            ));
        }
    }
    asked.sort();
    asked.dedup();
    let may = m.may_ask();
    for key in &asked {
        if key != m.key && !may.contains(&key.as_str()) {
            wrong.push(format!(
                "`{}` asks `project.has(\"{key}\")` and does not declare it: \
                 add `.adapts_to(\"{key}\", \"<what changes>\")` to its manifest",
                m.key
            ));
        }
    }
    for (key, _) in &m.adapts_to {
        if !asked.iter().any(|k| k == key) {
            wrong.push(format!(
                "`{}` declares `adapts_to(\"{key}\")` and never asks about it",
                m.key
            ));
        }
    }
    wrong.dedup();
    wrong
}

/// The rules on a manifest's Kinds that need no run: each reason is
/// written, a Kind is declared once per module, and a requirement is lifted
/// only by a key that exists.
fn kind_declarations(m: &crate::Manifest, known: &[&str]) -> Vec<String> {
    let mut wrong = Vec::new();
    let mut named: Vec<(&str, &str)> = Vec::new();
    let declared = m
        .provides
        .iter()
        .map(|(k, why)| ("provides", k.name, *why))
        .chain(m.requires.iter().map(|r| ("requires", r.kind.name, r.why)))
        .chain(m.uses.iter().map(|(k, why)| ("uses", k.name, *why)));
    for (how, kind, why) in declared {
        if why.is_empty() {
            wrong.push(format!(
                "`{}` declares `{how}::<{kind}>` and does not say why",
                m.key
            ));
        }
        if let Some((first, _)) = named.iter().find(|(_, k)| *k == kind) {
            wrong.push(format!(
                "`{}` declares `{first}::<{kind}>` and `{how}::<{kind}>`: keep one",
                m.key
            ));
        }
        named.push((how, kind));
    }
    for r in &m.requires {
        for key in r.unless {
            if !known.contains(key) {
                wrong.push(format!(
                    "`{}` requires {} unless `{key}`, which no module declares",
                    m.key, r.kind
                ));
            }
        }
    }
    wrong
}

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
    panic!("{}", mismatch(path, &expected, actual));
}

/// The message for a golden file that differs from what was generated.
fn mismatch(path: &Path, expected: &str, actual: &str) -> String {
    let first = expected
        .lines()
        .map(Some)
        .chain(std::iter::repeat(None))
        .zip(actual.lines().map(Some).chain(std::iter::repeat(None)))
        .take_while(|(a, b)| a.is_some() || b.is_some())
        .enumerate()
        .find(|(_, (a, b))| a != b)
        .map(|(at, (a, b))| (at + 1, a.unwrap_or("<end>"), b.unwrap_or("<end>")));
    // `lines()` hides a final newline and a `\r`, so two texts can differ
    // with every line equal. Say so, rather than "line 0" and two blanks.
    let Some((line, want, got)) = first else {
        let what = if expected.contains('\r') != actual.contains('\r') {
            "the line endings (CRLF against LF)"
        } else {
            "the newline at the end of the file"
        };
        return format!(
            "error: the generated project differs from {} only in {what}\n\
             \x20 golden:    {} bytes\n\
             \x20 generated: {} bytes\n\
             help: if the change is intended, run with BROCANTH_BLESS=1 and read the diff \
             before you commit it",
            path.display(),
            expected.len(),
            actual.len()
        );
    };
    format!(
        "error: the generated project differs from {} at line {line}\n\
         \x20 golden:    {want}\n\
         \x20 generated: {got}\n\
         help: if the change is intended, run with BROCANTH_BLESS=1 and read the diff \
         before you commit it",
        path.display()
    )
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
    // Cargo leaves a root package's path out of its hash, so two projects
    // named alike share one test binary in a shared target directory, and
    // cargo judges it fresh by the mtimes of the sources. One shared build
    // at a time, with the files written inside the turn: files written
    // while another build ran are older than its binary, and cargo would
    // run that binary for this project.
    static SHARED: Mutex<()> = Mutex::new(());
    let shared = std::env::var_os("BROCANTH_BUILD_TARGET").filter(|t| !t.is_empty());
    let _turn = shared
        .is_some()
        .then(|| SHARED.lock().unwrap_or_else(PoisonError::into_inner));
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
    if let Some(target) = shared {
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
    use crate::{Category, Manifest, Module, PgPool, Project, Scaffold};

    /// A module for the Kind rules: a manifest, and a `contribute` that asks
    /// for the pool or not.
    struct Asks {
        manifest: fn() -> Manifest,
        asks: bool,
    }

    impl Module for Asks {
        fn manifest(&self) -> Manifest {
            (self.manifest)()
        }
        fn contribute(&self, project: &Project, _: &mut Scaffold) {
            if self.asks {
                let _ = project.provider::<PgPool>();
            }
        }
    }

    fn base() -> Manifest {
        Manifest::new("thing", "a test module").category(Category::Enhancer)
    }

    fn check(manifest: fn() -> Manifest, asks: bool) -> Vec<String> {
        check_module(&Asks { manifest, asks }, &["thing", "thing-memory"])
    }

    #[test]
    fn a_kind_asked_must_be_declared_and_a_kind_used_must_be_asked() {
        let wrong = check(base, true);
        assert!(
            wrong[0].contains("asks `project.provider::<PgPool>()` and does not declare it"),
            "{wrong:?}"
        );
        let wrong = check(|| base().uses::<PgPool>("rows"), false);
        assert!(wrong[0].contains("never asks"), "{wrong:?}");
        assert!(check(|| base().uses::<PgPool>("rows"), true).is_empty());
        // A requirement may be asked, and need not be.
        assert!(check(|| base().requires::<PgPool>("rows"), true).is_empty());
        assert!(check(|| base().requires::<PgPool>("rows"), false).is_empty());
    }

    #[test]
    fn a_kind_is_declared_once_with_a_reason_and_lifted_by_a_known_key() {
        let wrong = check(
            || base().requires::<PgPool>("rows").uses::<PgPool>("rows"),
            true,
        );
        assert!(wrong.iter().any(|w| w.contains("keep one")), "{wrong:?}");
        let wrong = check(|| base().requires::<PgPool>(""), false);
        assert!(
            wrong.iter().any(|w| w.contains("does not say why")),
            "{wrong:?}"
        );
        let wrong = check(
            || base().requires_unless::<PgPool>(&["nope"], "rows"),
            false,
        );
        assert!(
            wrong.iter().any(|w| w.contains("unless `nope`")),
            "{wrong:?}"
        );
        assert!(check(
            || base().requires_unless::<PgPool>(&["thing-memory"], "rows"),
            false
        )
        .is_empty());
    }

    /// The pool is bound in the second run only when it is required there,
    /// so a module sees both branches of a `uses`.
    #[test]
    fn a_used_kind_is_seen_bound_and_unbound() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static BOUND: AtomicUsize = AtomicUsize::new(0);
        static UNBOUND: AtomicUsize = AtomicUsize::new(0);
        struct Counts;
        impl Module for Counts {
            fn manifest(&self) -> Manifest {
                base().uses::<PgPool>("rows")
            }
            fn contribute(&self, project: &Project, _: &mut Scaffold) {
                match project.provider::<PgPool>() {
                    Some(_) => BOUND.fetch_add(1, Ordering::SeqCst),
                    None => UNBOUND.fetch_add(1, Ordering::SeqCst),
                };
            }
        }
        assert!(check_module(&Counts, &["thing"]).is_empty());
        assert_eq!(BOUND.load(Ordering::SeqCst), 1);
        assert_eq!(UNBOUND.load(Ordering::SeqCst), 1);
    }

    fn files(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(p, c)| (p.to_string(), c.to_string()))
            .collect()
    }

    #[test]
    fn a_mismatch_in_the_final_newline_or_line_endings_is_named() {
        let path = Path::new("g.txt");
        let newline = mismatch(path, "a\nb\n", "a\nb");
        assert!(newline.contains("newline at the end"), "{newline}");
        assert!(
            newline.contains("4 bytes") && newline.contains("3 bytes"),
            "{newline}"
        );
        let crlf = mismatch(path, "a\nb\n", "a\r\nb\r\n");
        assert!(crlf.contains("CRLF"), "{crlf}");
        assert!(mismatch(path, "a\n", "b\n").contains("at line 1"));
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

    #[test]
    fn every_plugin_registration_shape_is_what_rustfmt_writes() {
        // The one registration form, for a short key, a long one, and a
        // plugin with no declared setting.
        for (key, ty, settings) in [
            ("db", "Database", true),
            ("oauth_client", "OauthClient", true),
            ("observability", "Observe", true),
            ("model", "Model", false),
        ] {
            let mut s = crate::Scaffold::new();
            s.now_writing(key);
            let everything = [key];
            let project = crate::Project::new("my-blog", "My Blog", &everything, &everything);
            let declared = if settings {
                vec![crate::Setting::bool("on", true, "On.")]
            } else {
                Vec::new()
            };
            crate::snippet::configurable(&mut s, &project, key, ty, declared);
            let source = format!(
                "fn f() {{\n    {{\n{}    }}\n}}\n",
                s.slot(crate::Slot::AppPlugins).collect::<String>()
            );
            if let Some(wrong) = unformatted(&files(&[("f.rs", &source)])) {
                assert!(wrong.is_empty(), "rustfmt changes:\n{source}");
            }
        }
    }
}
