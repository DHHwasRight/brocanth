//! The one decision, asserted at the binary rather than at the layer.
//!
//! `src/ui.rs` has unit tests over a `Ui` built on an in-memory writer, which
//! proves the renderers behave. These run the real executable with no terminal
//! on either descriptor -- which is what CI, a pipe and a redirect all look
//! like -- and prove that nothing downstream of the decision reintroduces an
//! escape sequence or changes a single character of layout.
//!
//! The fixture is built by `brocanth create`, so the scaffold is exercised end to
//! end on every run: the commands below read an application this tool wrote.
//! Only commands that need no compiler are used, so the suite stays in
//! milliseconds -- `cargo check` over the fixture is the `#[ignore]`d test in
//! `src/commands/new.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

const BROCANTH: &str = env!("CARGO_BIN_EXE_brocanth");

struct Run {
    stdout: String,
    stderr: String,
    code: i32,
}

impl Run {
    /// Both streams in the order a reader would have met them. `routes` writes
    /// its table to stdout and its commentary to stderr, so comparing only one
    /// of them would miss half the transcript.
    fn whole(&self) -> String {
        format!("{}{}", self.stderr, self.stdout)
    }
}

/// Runs the tool with no terminal on either descriptor.
fn run_in(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
    let mut cmd = Command::new(BROCANTH);
    cmd.arg("-C").arg(dir).args(args);
    // A clean slate: the host's own NO_COLOR must not decide the outcome of a
    // test about NO_COLOR.
    cmd.env_remove("NO_COLOR").env_remove("CLICOLOR_FORCE");
    for (key, value) in env {
        cmd.env(key, value);
    }
    let out = cmd.output().expect("run brocanth");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code().unwrap_or(-1),
    }
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brocanth-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The brocanth checkout this test binary was built from, so the generated
/// application depends on a path rather than on the network.
fn checkout() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .join("brocanth")
        .canonicalize()
        .expect("the framework is beside the cli")
        .display()
        .to_string()
}

/// An application on disk, written by `brocanth create` itself.
fn fixture(name: &str) -> PathBuf {
    let parent = temp(name);
    let checkout = checkout();
    let run = run_in(
        &parent,
        &[
            "create",
            "app",
            "--title",
            "Fixture",
            "--with",
            "logging,config,sessions",
            "--no-git",
            "--brocanth-path",
            &checkout,
        ],
        &[],
    );
    assert_eq!(run.code, 0, "new failed:\n{}", run.whole());
    parent.join("app")
}

fn strip(text: &str) -> String {
    anstream::adapter::strip_str(text).to_string()
}

/// Blanks out the one thing in a transcript that is allowed to differ between
/// two runs: how long the run took. Everything else must match byte for byte.
fn steady(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(|c: char| c.is_ascii_digit()) {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let end = tail
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(tail.len());
        let unit = tail[end..].starts_with("ms") || tail[end..].starts_with('s');
        if unit {
            out.push('#');
        } else {
            out.push_str(&tail[..end]);
        }
        rest = &tail[end..];
    }
    out.push_str(rest);
    out
}

/// Every command that needs no compiler, over the generated application.
const READ_ONLY: [&[&str]; 5] = [
    &["routes"],
    &["routes", "--no-middleware"],
    &["routes", "session"],
    &["check", "--no-compile"],
    &["dev"],
];

#[test]
fn the_coloured_transcript_is_the_plain_one_with_colour_added() {
    // The claim the whole layer rests on: one renderer, two transcripts, and
    // the difference between them is exactly the escape sequences. If a command
    // ever branched on the face to lay something out differently, this is the
    // test that would catch it.
    let root = fixture("same");
    for args in READ_ONLY {
        let plain = run_in(&root, args, &[]);
        let coloured = run_in(&root, args, &[("CLICOLOR_FORCE", "1")]);
        assert!(
            coloured.whole().contains('\u{1b}'),
            "{args:?} should have coloured under CLICOLOR_FORCE"
        );
        assert_eq!(
            steady(&strip(&coloured.whole())),
            steady(&plain.whole()),
            "{args:?} differs by more than colour"
        );
        assert_eq!(coloured.code, plain.code, "{args:?} exit code differs");
    }
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn the_scaffold_writes_the_same_transcript_coloured_and_plain() {
    // `new` is the renderer with the most to get wrong -- a file table, the
    // contents receipt and a next-steps block -- so it gets its own pass.
    // `--force` puts both runs in one directory, so the only difference that
    // can survive is the colour.
    let parent = temp("scaffold-face");
    let checkout = checkout();
    let args = [
        "create",
        "app",
        "--title",
        "Fixture",
        "--with",
        "logging,config",
        "--no-git",
        "--force",
        "--brocanth-path",
        &checkout,
    ];
    let plain = run_in(&parent, &args, &[]);
    let coloured = run_in(&parent, &args, &[("CLICOLOR_FORCE", "1")]);
    assert_eq!(plain.code, 0, "{}", plain.whole());
    assert!(coloured.whole().contains('\u{1b}'));
    assert_eq!(strip(&coloured.whole()), plain.whole());
    std::fs::remove_dir_all(&parent).unwrap();
}

#[test]
fn a_missing_name_names_the_argument_rather_than_waiting_for_one() {
    // The whole reason every prompt has a flag. With no terminal this must fail
    // immediately; a prompt here would hang a CI job until it timed out.
    let dir = temp("noname");
    let run = run_in(&dir, &["create"], &[]);
    assert_eq!(run.code, 1);
    assert!(
        run.stderr
            .contains("no name, and no terminal to ask for one"),
        "{}",
        run.stderr
    );
    assert!(
        run.stderr
            .contains("= help: pass it: `brocanth create <NAME>`"),
        "{}",
        run.stderr
    );
    assert!(run.stderr.contains("--with"), "{}", run.stderr);
    assert!(!run.stderr.contains('\u{1b}'), "{:?}", run.stderr);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn colour_alone_does_not_buy_a_chooser() {
    // The rung the two-way decision could not express. CLICOLOR_FORCE says
    // paint, and painting is a property of stdout; asking is a property of
    // stdin. A tool that conflated them would open a full-screen view onto a
    // descriptor that can never answer it, and wait there.
    let dir = temp("forced");
    let run = run_in(&dir, &["create"], &[("CLICOLOR_FORCE", "1")]);
    assert_eq!(run.code, 1, "it must fail rather than wait for a key");
    assert!(
        run.stderr
            .contains("no name, and no terminal to ask for one"),
        "{}",
        run.stderr
    );
    // And it is still the coloured transcript, because colour was asked for.
    assert!(run.stderr.contains('\u{1b}'), "{:?}", run.stderr);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn new_still_reaches_create() {
    // The command was renamed, not replaced. An alias costs one line and saves
    // every script and every pair of hands that learned the old name.
    let parent = temp("alias");
    let checkout = checkout();
    let run = run_in(
        &parent,
        &[
            "new",
            "aliased",
            "--core-only",
            "--no-git",
            "--brocanth-path",
            &checkout,
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}", run.whole());
    assert!(parent.join("aliased/Cargo.toml").exists());
    std::fs::remove_dir_all(&parent).unwrap();
}

#[test]
fn the_model_layer_is_the_only_thing_that_puts_an_orm_in_the_manifest() {
    let parent = temp("model");
    let checkout = checkout();
    let run = run_in(
        &parent,
        &[
            "create",
            "shop",
            "--with",
            "model,config",
            "--no-git",
            "--brocanth-path",
            &checkout,
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}", run.whole());
    let manifest = std::fs::read_to_string(parent.join("shop/Cargo.toml")).unwrap();
    assert!(manifest.contains("brocanth-model = {"), "{manifest}");
    let config = std::fs::read_to_string(parent.join("shop/config/development.toml")).unwrap();
    assert!(config.contains("[model]"), "{config}");
    assert!(!config.contains("[logging]"), "{config}");
    std::fs::remove_dir_all(&parent).unwrap();
}

#[test]
#[ignore = "runs cargo build over a generated project; minutes, not milliseconds"]
fn a_scaffolded_project_builds_and_its_tasks_run() {
    // The end-to-end claim: `create` writes a project, cargo compiles it, and
    // the command the scaffold put in `src/commands/` is a working subcommand of
    // the `manage` binary. Every other test here stops short of a compiler,
    // so this is the only one that would notice a scaffold that is merely
    // well-formed text.
    let parent = temp("scaffold-builds");
    let checkout = checkout();
    let run = run_in(
        &parent,
        &[
            "create",
            "built",
            "--with",
            "logging,config,commands",
            "--no-git",
            "--brocanth-path",
            &checkout,
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}", run.whole());
    let root = parent.join("built");

    // What a project with a command line has.
    for path in [
        "brocanth.toml",
        "src/app.rs",
        "src/commands/mod.rs",
        "src/commands/serve.rs",
    ] {
        assert!(root.join(path).is_file(), "{path} is missing");
    }
    // And what it deliberately does not: no second binary, and no cargo
    // alias. A project's command line is a framework concern; cargo is the
    // build tool, and hooking one into the other conflates them.
    for path in [".cargo/config.toml", "src/bin/manage.rs"] {
        assert!(!root.join(path).exists(), "{path} should be gone");
    }

    let built = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--quiet"])
        .current_dir(&root)
        .output()
        .expect("cargo build");
    assert!(
        built.status.success(),
        "the scaffold does not compile:\n{}",
        String::from_utf8_lossy(&built.stderr)
    );

    // And the commands actually registered, through the slice, in a project
    // this tool wrote from scratch.
    let listed = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["run", "--quiet", "--", "commands"])
        .current_dir(&root)
        .output()
        .expect("cargo run");
    let listed = String::from_utf8_lossy(&listed.stdout);
    assert!(
        listed.contains("serve"),
        "the one built-in command is absent:\n{listed}"
    );
    assert!(listed.contains("commands"), "{listed}");

    // `serve --help`, which proves the command is wired and its flags parsed
    // without binding a socket in a test.
    let ran = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["run", "--quiet", "--", "serve", "--help"])
        .current_dir(&root)
        .output()
        .expect("cargo run");
    assert!(
        ran.status.success(),
        "`serve --help` does not run:\n{}",
        String::from_utf8_lossy(&ran.stderr)
    );

    // PLAN.md §4.8: "if scaffolding produces code you would not write by
    // hand, the framework below it is wrong." Nobody writes an unindented
    // struct body, and a template built with `\n\` continuations loses the
    // next line's leading whitespace -- which is exactly how `src/commands/mod.rs`
    // and `src/bin/manage.rs` shipped flat. rustfmt is the cheapest judge of
    // "would you write this", so it judges.
    let fmt = Command::new("cargo")
        .args(["fmt", "--check"])
        .current_dir(&root)
        .output()
        .expect("cargo fmt");
    assert!(
        fmt.status.success(),
        "a freshly scaffolded project is not fmt-clean:\n{}{}",
        String::from_utf8_lossy(&fmt.stdout),
        String::from_utf8_lossy(&fmt.stderr)
    );

    std::fs::remove_dir_all(&parent).unwrap();
}
