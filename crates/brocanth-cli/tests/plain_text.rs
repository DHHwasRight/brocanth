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
fn a_pipe_receives_no_escape_sequence_at_all() {
    let root = fixture("plain");
    for args in READ_ONLY {
        let run = run_in(&root, args, &[]);
        let whole = run.whole();
        assert!(
            !whole.contains('\u{1b}'),
            "{args:?} wrote an escape sequence to a pipe:\n{whole:?}"
        );
    }
    // `generate` prints a plan and writes nothing, so it is safe to run here.
    let run = run_in(&root, &["generate", "model", "Post"], &[]);
    assert!(!run.whole().contains('\u{1b}'), "{:?}", run.whole());
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

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
fn no_color_the_flag_and_the_variable_agree() {
    let root = fixture("nocolor");
    let flag = run_in(&root, &["routes", "--no-color"], &[("CLICOLOR_FORCE", "1")]);
    let variable = run_in(
        &root,
        &["routes"],
        &[("NO_COLOR", "1"), ("CLICOLOR_FORCE", "1")],
    );
    let piped = run_in(&root, &["routes"], &[]);
    assert_eq!(flag.whole(), piped.whole());
    assert_eq!(variable.whole(), piped.whole());
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn quiet_keeps_the_table_and_drops_the_commentary() {
    let root = fixture("quiet");
    let loud = run_in(&root, &["routes"], &[]);
    let quiet = run_in(&root, &["routes", "--quiet"], &[]);
    assert_eq!(quiet.stderr, "", "--quiet should say nothing on stderr");
    assert_eq!(
        quiet.stdout, loud.stdout,
        "--quiet must not change what was asked for"
    );
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn routes_prints_the_table_the_file_convention_produces() {
    let root = fixture("table");
    let run = run_in(&root, &["routes"], &[]);
    assert_eq!(run.code, 0, "routes reports, it does not judge");
    let stdout = run.stdout;
    for line in [
        "GET     /              index.rs#handle         request_log",
        "GET     /about         about.rs#handle         request_log",
        "GET     /users/{id}    users/[id].rs#handle         request_log, session",
        "POST    /users/{id}    users/[id].rs#handle_post    request_log, session",
    ] {
        // Compare on words rather than on the exact run of spaces, so the
        // column widths stay free to change without rewriting the test.
        let wanted: Vec<&str> = line.split_whitespace().collect();
        assert!(
            stdout
                .lines()
                .any(|l| l.split_whitespace().collect::<Vec<_>>() == wanted),
            "missing {wanted:?} in:\n{stdout}"
        );
    }
    // The mount table says where each layer runs and who offered it.
    assert!(stdout.contains("request_log"), "{stdout}");
    assert!(stdout.contains("src/plugins/sessions.rs"), "{stdout}");
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn a_filter_keeps_the_rows_it_names_and_drops_the_rest() {
    let root = fixture("filter");
    let all = run_in(&root, &["routes"], &[]);
    let post = run_in(&root, &["routes", "post"], &[]);
    assert!(post.stdout.contains("handle_post"), "{}", post.stdout);
    assert!(!post.stdout.contains("about.rs"), "{}", post.stdout);
    assert!(all.stdout.lines().count() > post.stdout.lines().count());
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn a_typo_in_a_mount_is_an_error_in_rustcs_shape() {
    // The finding §8 makes possible: a plugin offers, only the application
    // mounts, so the name can be misspelt in exactly one place.
    let root = fixture("typo");
    let main = root.join("src/main.rs");
    let source = std::fs::read_to_string(&main).unwrap();
    std::fs::write(&main, source.replace("\"session\")", "\"sesion\")")).unwrap();

    let run = run_in(&root, &["check", "--no-compile"], &[]);
    assert_eq!(run.code, 1, "a mounted name nobody offers must fail");
    for line in [
        "error[middleware/unknown]: `sesion` is mounted, but no registered plugin offers it",
        " --> src/main.rs:",
        "= help: did you mean `session`?",
    ] {
        assert!(
            run.stderr.contains(line),
            "missing {line:?} in:\n{}",
            run.stderr
        );
    }
    // The caret run covers the name, quotes included, and nothing else.
    assert!(run.stderr.contains("^^^^^^^^\n"), "{}", run.stderr);
    assert!(!run.stderr.contains('\u{1b}'), "{:?}", run.stderr);
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn a_page_that_answers_no_verb_is_a_warning_and_still_passes() {
    // A page someone is still writing must not fail the command: a tool that
    // refused to pass until every draft was finished would be turned off.
    let root = fixture("draft");
    std::fs::write(
        root.join("src/pages/draft.rs"),
        "//! a page nobody has finished\n",
    )
    .unwrap();
    let run = run_in(&root, &["check", "--no-compile"], &[]);
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stderr
            .contains("warning[route/no-handler]: `/draft` answers no verb"),
        "{}",
        run.stderr
    );
    // And `routes` says so too, without pretending to know what to do about it.
    let routes = run_in(&root, &["routes"], &[]);
    assert!(
        routes.stderr.contains("that answer nothing"),
        "{}",
        routes.stderr
    );
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
}

#[test]
fn a_middleware_pattern_that_matches_nothing_is_reported() {
    let root = fixture("ghost");
    let main = root.join("src/main.rs");
    let source = std::fs::read_to_string(&main).unwrap();
    std::fs::write(
        &main,
        source.replace("\"/users/{*rest}\"", "\"/admin/{*rest}\""),
    )
    .unwrap();
    let run = run_in(&root, &["check", "--no-compile"], &[]);
    assert_eq!(run.code, 0, "it is dead weight, not a failure");
    assert!(
        run.stderr.contains(
            "warning[middleware/no-match]: `/admin/{*rest}` matches no route, so `session` never runs"
        ),
        "{}",
        run.stderr
    );
    std::fs::remove_dir_all(root.parent().unwrap()).unwrap();
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
fn the_core_alone_is_one_dependency_and_no_plugin_directory() {
    // §8, end to end: what was not asked for is absent from the source, the
    // manifest and therefore the lock file.
    let parent = temp("coreonly");
    let checkout = checkout();
    let run = run_in(
        &parent,
        &[
            "create",
            "bare",
            "--core-only",
            "--no-git",
            "--brocanth-path",
            &checkout,
        ],
        &[],
    );
    assert_eq!(run.code, 0, "{}", run.whole());
    let root = parent.join("bare");
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(manifest.contains("brocanth = {"), "{manifest}");
    for absent in ["serde", "brocanth-config", "brocanth-model", "toml"] {
        assert!(
            !manifest.contains(&format!("\n{absent} = ")),
            "{absent} is in a manifest nobody asked for it in:\n{manifest}"
        );
    }
    assert!(!root.join("src/plugins").exists());
    assert!(!root.join("config").exists());
    // And the tool reads its own output: no plugins, no mounts, three routes.
    let routes = run_in(&root, &["routes"], &[]);
    assert_eq!(routes.code, 0, "{}", routes.whole());
    assert!(routes.stderr.contains("3 routes"), "{}", routes.stderr);
    assert!(routes.stderr.contains("0 mounts"), "{}", routes.stderr);
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
fn a_directory_that_is_not_an_application_says_which_flag_to_pass() {
    let dir = temp("notanapp");
    let run = run_in(&dir, &["routes"], &[]);
    assert_eq!(run.code, 1);
    assert!(run.stderr.contains("error[no-package]:"), "{}", run.stderr);
    assert!(
        run.stderr
            .contains("run `brocanth create <NAME>` to make one"),
        "{}",
        run.stderr
    );
    std::fs::remove_dir_all(&dir).unwrap();
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

    // The four files commands add, and the alias that makes them reachable.
    for path in [
        "brocanth.toml",
        ".cargo/config.toml",
        "src/bin/manage.rs",
        "src/commands/mod.rs",
    ] {
        assert!(root.join(path).is_file(), "{path} is missing");
    }
    let alias = std::fs::read_to_string(root.join(".cargo/config.toml")).unwrap();
    assert!(alias.contains("manage = "), "{alias}");

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

    // And the command actually registered, through the slice, in a project this
    // tool wrote from scratch.
    let manage = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["run", "--quiet", "--bin", "manage", "--", "commands"])
        .current_dir(&root)
        .output()
        .expect("cargo run");
    let listed = String::from_utf8_lossy(&manage.stdout);
    assert!(
        listed.contains("greet"),
        "the example command is absent:\n{listed}"
    );
    assert!(listed.contains("commands"), "{listed}");

    // `cargo run` with no `--bin`, which is what somebody types first. With
    // two binaries cargo refuses to guess, so the manifest has to name one.
    let ran = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["run", "--quiet"])
        .current_dir(&root)
        .output()
        .expect("cargo run");
    assert!(
        ran.status.success(),
        "`cargo run` does not pick a binary:\n{}",
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
