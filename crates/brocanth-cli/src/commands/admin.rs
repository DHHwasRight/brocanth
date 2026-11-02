//! `brocanth admin eject` -- a shipped admin screen, copied into the project
//! and installed over the default.
//!
//! Laravel's `vendor:publish`, for one screen. Level 8 of
//! `docs/admin-customization.md`: when wrapping a screen (level 4) is not
//! enough and writing one from an empty file is too much.
//!
//! ```text
//! brocanth admin eject list                  # every model's list
//! brocanth admin eject list --model invoice  # the invoice list only
//! brocanth admin eject layout                # the shell around every screen
//! ```
//!
//! # What it writes
//!
//! | Path | Owner |
//! |---|---|
//! | `src/plugins/admin/screens/<screen>.rsx` | you, from the moment it is written |
//! | `src/plugins/admin/screens/models/<model>/<screen>.rsx` | you |
//! | `src/plugins/admin/ejected.rs` | this command: written again on every eject |
//!
//! And three edits, each made once and only when missing: `rsx` in
//! `[dependencies]` (an ejected screen is a template the project compiles),
//! `mod ejected;` in `src/plugins/admin/mod.rs`, and
//! `let config = ejected::apply(config);` before `Admin::new` in `mount`.
//!
//! `ejected.rs` is rebuilt from the `.rsx` files on disk, so the files are
//! the record of what was ejected. Delete one and run any eject again to
//! drop it from the wiring.
//!
//! # What it refuses
//!
//! An existing copy is never overwritten: it is yours, and the shipped
//! screen is one `rm` away. A screen that shows no one model (`layout`,
//! `index`, `error`) cannot take `--model`. A model key that is not a Rust
//! module name cannot name the directory `rsx` turns into a module.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use brocanth_admin::eject::{self, Ejected};
use clap::{Args, Subcommand};
use toml_edit::{DocumentMut, InlineTable, Item, Value};

use crate::commands::project::Project;
use crate::commands::Outcome;
use crate::ui::{paint, Diagnostic, Ui, ACCENT, DIM};

#[derive(Args)]
pub struct AdminArgs {
    #[command(subcommand)]
    pub command: AdminCommand,
}

#[derive(Subcommand)]
pub enum AdminCommand {
    /// Copy a shipped screen into the project and install it over the default
    ///
    /// The copy renders what the shipped screen does until you edit it. It
    /// is yours: an upgrade of brocanth-admin does not change it.
    Eject(EjectArgs),
}

#[derive(Args)]
pub struct EjectArgs {
    /// The screen to copy
    #[arg(value_parser = clap::builder::PossibleValuesParser::new(eject::names()))]
    pub screen: String,
    /// Install the copy for this model only. list, detail, form and delete take one
    #[arg(long, value_name = "KEY")]
    pub model: Option<String>,
}

/// Where the plugin and the copies live, relative to the package root.
const PLUGIN: &str = "src/plugins/admin/mod.rs";
const SCREENS: &str = "src/plugins/admin/screens";
const WIRING: &str = "src/plugins/admin/ejected.rs";

/// The line in the generated `mount` the install goes in front of.
const ANCHOR: &str = "let admin = Arc::new(Admin::new(";
const INSTALL: &str = "let config = ejected::apply(config);";

pub fn run(ui: &Ui, dir: Option<&Path>, args: AdminArgs) -> Outcome {
    match args.command {
        AdminCommand::Eject(args) => eject(ui, dir, args),
    }
}

fn eject(ui: &Ui, dir: Option<&Path>, args: EjectArgs) -> Outcome {
    let project = Project::find(dir)?;
    // An empty list is a marker from before the list, which says nothing.
    if !project.modules.is_empty() && !project.has("admin") {
        return Err(vec![Diagnostic::error(
            "admin/no-admin",
            "this project has no admin, so it has no screen to eject",
        )
        .help("`brocanth add admin` mounts it at /admin first")]);
    }
    let Some(screen) = eject::screen(&args.screen) else {
        // clap checked the name against the same list.
        return Err(vec![Diagnostic::error(
            "admin/screen",
            format!("there is no admin screen named `{}`", args.screen),
        )
        .help(format!("one of: {}", eject::names().join(", ")))]);
    };
    if let Some(model) = &args.model {
        if screen.one_model.is_none() {
            return Err(vec![Diagnostic::error(
                "admin/not-per-model",
                format!(
                    "`{}` shows no one model, so it cannot be ejected for `{model}`",
                    screen.name
                ),
            )
            .help(format!(
                "drop --model to eject it for every model; per model: {}",
                per_model().join(", ")
            ))]);
        }
        if !eject::model_key_is_a_module_name(model) {
            return Err(vec![Diagnostic::error(
                "admin/model-key",
                format!("`{model}` cannot name a template directory"),
            )
            .note("rsx turns each directory into a Rust module, so the key must be one")
            .help(
                "eject it for every model and branch on `ctx.model.key` inside the copy",
            )]);
        }
    }

    let root = &project.root;
    let plugin = root.join(PLUGIN);
    let plugin_text = std::fs::read_to_string(&plugin).map_err(|e| {
        vec![
            Diagnostic::error("admin/no-plugin", format!("cannot read {PLUGIN}: {e}"))
                .help("`brocanth add admin` writes it; an ejected screen is installed there"),
        ]
    })?;

    let wanted = Ejected {
        screen: screen.name,
        model: args.model.clone(),
    };
    let copy = root.join(SCREENS).join(wanted.file());
    if copy.exists() {
        return Err(vec![Diagnostic::error(
            "admin/exists",
            format!("{} is already ejected", describe(&wanted)),
        )
        .at(relative(root, &copy))
        .help(
            "edit that file, or delete it and run this again to start from the shipped screen",
        )]);
    }

    write(&copy, &screen.source())?;
    let all = on_disk(&root.join(SCREENS));
    write(&root.join(WIRING), &eject::wiring(SCREENS, &all))?;
    let mut changed = vec![relative(root, &copy), WIRING.to_string()];

    if add_rsx(&root.join("Cargo.toml"))? {
        changed.push("Cargo.toml".to_string());
    }
    let (wired, installed) = wire(&plugin_text);
    if wired != plugin_text {
        write(&plugin, &wired)?;
        changed.push(PLUGIN.to_string());
    }

    ui.done(format!("ejected {}", paint(ACCENT, describe(&wanted))));
    for path in &changed {
        ui.note(format!("  {}", paint(DIM, path)));
    }
    if !installed {
        ui.diagnostic(
            &Diagnostic::warning(
                "admin/not-installed",
                "the copy is written, and nothing installs it yet",
            )
            .at(PLUGIN)
            .note(format!("`mount` no longer has the line `{ANCHOR}`"))
            .help(format!(
                "add `{INSTALL}` before the admin is built from `config`"
            )),
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The screens that take `--model`.
fn per_model() -> Vec<&'static str> {
    eject::SCREENS
        .iter()
        .filter(|s| s.one_model.is_some())
        .map(|s| s.name)
        .collect()
}

/// `` `list` `` or `` `list` for `invoice` ``.
fn describe(e: &Ejected) -> String {
    match &e.model {
        None => format!("`{}`", e.screen),
        Some(model) => format!("`{}` for `{model}`", e.screen),
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn write(path: &Path, text: &str) -> Result<(), Vec<Diagnostic>> {
    let fail = |e: std::io::Error| {
        vec![Diagnostic::error("write", format!("cannot write: {e}")).at(path.display().to_string())]
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(fail)?;
    }
    std::fs::write(path, text).map_err(fail)
}

/// Every copy under the screens directory that names a screen, for every
/// model or for one. Anything else there is the developer's and is left
/// out of the wiring.
fn on_disk(screens: &Path) -> Vec<Ejected> {
    let stems = |dir: &Path| -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".rsx").map(str::to_owned)
            })
            .collect()
    };
    let mut found = Vec::new();
    for stem in stems(screens) {
        if let Some(s) = eject::screen(&stem) {
            found.push(Ejected {
                screen: s.name,
                model: None,
            });
        }
    }
    let models = screens.join("models");
    for entry in std::fs::read_dir(&models).into_iter().flatten().flatten() {
        let model = entry.file_name().to_string_lossy().into_owned();
        if !entry.path().is_dir() || !eject::model_key_is_a_module_name(&model) {
            continue;
        }
        for stem in stems(&entry.path()) {
            if let Some(s) = eject::screen(&stem).filter(|s| s.one_model.is_some()) {
                found.push(Ejected {
                    screen: s.name,
                    model: Some(model.clone()),
                });
            }
        }
    }
    found.sort();
    found
}

/// Adds `rsx` to `[dependencies]` when it is missing. True when it did.
///
/// An edit, not a rewrite: the developer writes in this file too.
fn add_rsx(path: &PathBuf) -> Result<bool, Vec<Diagnostic>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        vec![
            Diagnostic::error("manifest", format!("cannot read Cargo.toml: {e}"))
                .at(path.display().to_string()),
        ]
    })?;
    let mut doc = text.parse::<DocumentMut>().map_err(|e| {
        vec![
            Diagnostic::error("manifest", format!("Cargo.toml is not TOML: {e}"))
                .at(path.display().to_string()),
        ]
    })?;
    let deps = doc
        .entry("dependencies")
        .or_insert(Item::Table(toml_edit::Table::new()));
    let Some(deps) = deps.as_table_like_mut() else {
        return Err(vec![Diagnostic::error(
            "manifest",
            "`dependencies` in Cargo.toml is not a table",
        )
        .at(path.display().to_string())]);
    };
    if deps.contains_key("rsx") {
        return Ok(false);
    }
    let mut source = InlineTable::new();
    source.insert("git", Value::from(crate::modules::rsx::REPOSITORY));
    source.insert("branch", Value::from("main"));
    deps.insert("rsx", Item::Value(Value::InlineTable(source)));
    write(path, &doc.to_string())?;
    Ok(true)
}

/// The plugin with `mod ejected;` and the install line in it, and whether
/// the install line is there now.
///
/// Each edit is made once. `mod ejected;` goes above the first `use`, and
/// the install line in front of [`ANCHOR`], at its indentation. A `mount`
/// the developer rewrote has no anchor: the caller says what to add.
fn wire(text: &str) -> (String, bool) {
    let mut out = text.to_string();
    if !out.lines().any(|l| l.trim() == "mod ejected;") {
        out = match out.find("\nuse ") {
            Some(at) => format!("{}mod ejected;\n\n{}", &out[..=at], &out[at + 1..]),
            None => format!("{out}\nmod ejected;\n"),
        };
    }
    if out.contains("ejected::apply(") {
        return (out, true);
    }
    let Some(at) = out.find(ANCHOR) else {
        return (out, false);
    };
    let line_start = out[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent = out[line_start..at].to_string();
    out.insert_str(line_start, &format!("{indent}{INSTALL}\n"));
    (out, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::create::{self, Answers};

    fn ui() -> Ui {
        Ui::new(crate::ui::Face::Plain, true, false)
    }

    fn project(name: &str, picks: &[&'static str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "brocanth-eject-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let answers = Answers {
            name: name.to_string(),
            title: crate::ui::title_case(name),
            picks: picks.to_vec(),
            local: None,
            git: false,
        };
        for (path, text) in create::scaffold(&answers).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().expect("a directory")).expect("mkdir");
            std::fs::write(&full, text).expect("write");
        }
        root
    }

    fn eject_in(root: &Path, screen: &str, model: Option<&str>) -> Outcome {
        run(
            &ui(),
            Some(root),
            AdminArgs {
                command: AdminCommand::Eject(EjectArgs {
                    screen: screen.to_string(),
                    model: model.map(str::to_string),
                }),
            },
        )
    }

    fn read(root: &Path, path: &str) -> String {
        std::fs::read_to_string(root.join(path)).unwrap_or_default()
    }

    fn refusal(outcome: Outcome) -> Diagnostic {
        match outcome {
            Ok(_) => panic!("it was not refused"),
            Err(mut d) => d.remove(0),
        }
    }

    #[test]
    fn ejecting_writes_the_copy_and_installs_it_once() {
        let root = project("ejecty", &["config", "admin"]);
        eject_in(&root, "list", None).expect("list ejects");

        let copy = read(&root, "src/plugins/admin/screens/list.rsx");
        assert!(copy.starts_with("// Ejected from brocanth-admin"));
        assert!(copy.contains("pub fn List(ctx: &ListCtx) -> Element"));
        assert!(
            !copy.contains("crate::"),
            "the copy names the crate's privates"
        );

        let wiring = read(&root, WIRING);
        assert!(wiring.contains("views.with_list("));

        let plugin = read(&root, PLUGIN);
        assert_eq!(plugin.matches("mod ejected;").count(), 1);
        assert_eq!(plugin.matches(INSTALL).count(), 1);
        let install = plugin.find(INSTALL).expect("installed");
        assert!(install < plugin.find(ANCHOR).expect("the anchor"));

        let manifest = read(&root, "Cargo.toml");
        assert!(manifest.lines().any(|l| l.starts_with("rsx = ")));

        // A second screen: the wiring holds both, nothing else is added twice.
        eject_in(&root, "layout", None).expect("layout ejects");
        eject_in(&root, "form", Some("invoice")).expect("one model's form ejects");
        let wiring = read(&root, WIRING);
        assert!(wiring.contains("views.with_list("));
        assert!(wiring.contains("views.with_layout("));
        assert!(wiring.contains("config.model(\"invoice\", |m| {"));
        assert!(
            read(&root, "src/plugins/admin/screens/models/invoice/form.rsx")
                .contains("pub fn FormPage")
        );
        let plugin = read(&root, PLUGIN);
        assert_eq!(plugin.matches("mod ejected;").count(), 1);
        assert_eq!(plugin.matches(INSTALL).count(), 1);
        let manifest = read(&root, "Cargo.toml");
        assert_eq!(
            manifest.lines().filter(|l| l.starts_with("rsx = ")).count(),
            1
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_copy_that_exists_is_never_overwritten() {
        let root = project("twice", &["admin"]);
        eject_in(&root, "detail", None).expect("detail ejects");
        let path = root.join("src/plugins/admin/screens/detail.rsx");
        std::fs::write(&path, "// mine\n").unwrap();

        let d = refusal(eject_in(&root, "detail", None));
        assert_eq!(d.code.as_deref(), Some("admin/exists"));
        assert!(d.notes.iter().any(|(kind, _)| *kind == "help"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "// mine\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_screen_of_no_one_model_takes_no_model() {
        let root = project("nomodel", &["admin"]);
        for screen in ["layout", "index", "error"] {
            let d = refusal(eject_in(&root, screen, Some("invoice")));
            assert_eq!(d.code.as_deref(), Some("admin/not-per-model"), "{screen}");
        }
        assert!(!root.join(SCREENS).exists(), "a refusal wrote a file");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_model_key_that_is_no_module_name_is_refused() {
        let root = project("badkey", &["admin"]);
        let d = refusal(eject_in(&root, "list", Some("blog-post")));
        assert_eq!(d.code.as_deref(), Some("admin/model-key"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_project_without_the_admin_is_refused() {
        let root = project("noadmin", &["config"]);
        let d = refusal(eject_in(&root, "list", None));
        assert_eq!(d.code.as_deref(), Some("admin/no-admin"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_project_with_rsx_keeps_its_own_line() {
        let root = project("withrsx", &["admin", "rsx"]);
        let before = read(&root, "Cargo.toml");
        eject_in(&root, "index", None).expect("index ejects");
        assert_eq!(read(&root, "Cargo.toml"), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_rewritten_mount_gets_the_copy_and_a_warning_not_an_edit() {
        let (text, installed) = wire("use a::b;\n\nfn mount() {\n    build(config);\n}\n");
        assert!(!installed);
        assert!(text.contains("mod ejected;"));
        assert!(!text.contains(INSTALL));
    }

    #[test]
    fn every_written_file_is_already_formatted() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        if Command::new("rustfmt").arg("--version").output().is_err() {
            eprintln!("skipped: rustfmt is not on PATH");
            return;
        }
        let root = project("fmt", &["config", "admin", "middleware", "middleware-csrf"]);
        eject_in(&root, "layout", None).expect("layout ejects");
        eject_in(&root, "list", Some("post")).expect("post's list ejects");
        for path in [PLUGIN, WIRING] {
            let source = read(&root, path);
            let mut child = Command::new("rustfmt")
                .args(["--edition", "2021", "--emit", "stdout"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("rustfmt runs");
            child
                .stdin
                .take()
                .expect("a pipe")
                .write_all(source.as_bytes())
                .expect("rustfmt reads");
            let done = child.wait_with_output().expect("rustfmt finishes");
            assert!(done.status.success(), "rustfmt could not read {path}");
            assert_eq!(String::from_utf8_lossy(&done.stdout), source, "{path}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A generated project with ejected screens builds, passes its own
    /// tests, and is still formatted.
    ///
    /// `cargo test -p brocanth-cli -- --ignored`.
    #[test]
    #[ignore = "runs cargo test over a generated application"]
    fn a_project_with_ejected_screens_compiles_and_passes_its_tests() {
        use std::process::Command;

        let brocanth = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-ejected-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let answers = Answers {
            name: "ejected-test".into(),
            title: "Ejected Test".into(),
            picks: vec!["config", "admin", "middleware", "middleware-csrf"],
            local: Some(brocanth.display().to_string()),
            git: false,
        };
        for (path, text) in create::scaffold(&answers).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, text).unwrap();
        }
        // Every screen for every model, and one model's four: the generated
        // test then renders the locked admin's 403 through the ejected
        // error screen and layout.
        for s in eject::SCREENS {
            eject_in(&root, s.name, None).expect("ejects");
        }
        for s in per_model() {
            eject_in(&root, s, Some("post")).expect("ejects for one model");
        }
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        for args in [&["test", "--quiet"][..], &["fmt", "--check"][..]] {
            let output = Command::new(&cargo)
                .args(args)
                .current_dir(&root)
                .output()
                .expect("run cargo");
            assert!(
                output.status.success(),
                "`cargo {}` failed in the ejected project:\n{}{}",
                args.join(" "),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
