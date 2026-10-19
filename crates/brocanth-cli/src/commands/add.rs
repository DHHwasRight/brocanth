//! `brocanth add` -- put a module into a project that already exists.
//!
//! `create` was the only way in, so adding a queue to a running application
//! meant doing by hand what the generator does, with the generator's output
//! open in another window as the reference.
//!
//! # One generator, two front doors
//!
//! This command does not write anything `create` does not. It edits the
//! module list in `brocanth.toml` and asks the generator for the project
//! again. What comes back is compared with what the project was before, and
//! the difference is written. That is the whole of it, and it is the whole
//! of it on purpose: this repository has already paid twice for two routes
//! into the scaffold drifting apart.
//!
//! # What it may write
//!
//! Three kinds of file, and [`crate::owned`] decides which is which:
//!
//! * **Owned** -- `src/plugins/mod.rs`, `src/commands/mod.rs`,
//!   `src/main.rs`, `brocanth.toml`. Rewritten whole. Each carries a digest,
//!   and a digest that does not match stops the command: the generator
//!   cannot prevent an edit, so it must never destroy one in silence.
//! * **Shared** -- `Cargo.toml` and `config/<env>.toml`. Both sides write in
//!   them, so this appends and leaves every other byte alone.
//! * **The developer's** -- everything else. Written when it is absent and
//!   never rewritten.
//!
//! # When it refuses
//!
//! A module whose arrival would change a file the developer owns cannot be
//! added by this command. `config` is the clearest case: it changes whether
//! every *other* plugin's settings struct derives `Deserialize`, and those
//! structs are in files that belong to the developer. So the command says
//! which files would have had to change, prints what the generator would
//! have written, and writes nothing.
//!
//! `src/app.rs` is the one exception, and it is the design rather than a
//! hole in it. What lands there is a middleware mount, and a mount is a
//! decision: the pattern is the application's to choose, which is why that
//! file is the developer's in the first place. A project without the mount
//! compiles and runs; it just does not run that layer. So the lines are
//! printed for the developer to place, and the command carries on.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use toml_edit::{DocumentMut, Item};

use crate::commands::create::{self, Answers};
use crate::commands::Outcome;
use crate::owned::{self, Kind};
use crate::ui::{Cell, Diagnostic, Table, Ui, ACCENT, DIM, STRONG};

#[derive(Args)]
pub struct AddArgs {
    /// Modules to add; repeat the argument or separate with commas
    #[arg(
        value_name = "MODULE",
        required = true,
        value_delimiter = ',',
        value_parser = create::offered()
    )]
    pub modules: Vec<String>,
    /// Rewrite a generated file that was edited, discarding the edit
    #[arg(long)]
    pub force: bool,
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: AddArgs) -> Outcome {
    let start = match dir {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir().map_err(|e| {
            vec![Diagnostic::error(
                "cwd",
                format!("cannot read the current directory: {e}"),
            )]
        })?,
    };
    // The same rule the delegating command uses, and the same function. Two
    // answers to "where does this project start" is one answer too many.
    let root = super::delegate::project_root(&start)?;

    let marker = root.join("brocanth.toml");
    let recorded = std::fs::read_to_string(&marker).map_err(|e| read_error(&marker, &e))?;
    if owned::unedited(&recorded) == Some(false) && !args.force {
        return Err(vec![Diagnostic::error(
            "edited",
            "brocanth.toml has been edited since brocanth wrote it",
        )
        .at("brocanth.toml")
        .note("it records what this project is made of, and `add` reads it before it writes")
        .help("undo the edit, or pass --force to let `add` rewrite it")]);
    }
    let project = read_record(&recorded)?;

    let wanted = named(&args.modules)?;
    let already: Vec<&str> = wanted
        .iter()
        .copied()
        .filter(|key| project.enabled.contains(key))
        .collect();
    let adding: Vec<&'static str> = wanted
        .iter()
        .copied()
        .filter(|key| !project.enabled.contains(key))
        .collect();
    if adding.is_empty() {
        ui.done(format!("{} is already in this project", already.join(", ")));
        return Ok(ExitCode::SUCCESS);
    }

    // In the registry's order and not the order they were typed, for the
    // reason `create` gives: the same set of words has to produce the same
    // project.
    let after_picks: Vec<&'static str> = create::all()
        .into_iter()
        .filter(|key| project.enabled.contains(key) || adding.contains(key))
        .collect();
    create::unavailable(&after_picks)?;
    create::orphans(&after_picks)?;
    create::unmet(&after_picks)?;

    // Where this project's brocanth comes from. Read back out of its own
    // manifest rather than recorded in `brocanth.toml`, because the answer
    // is already written down once and an absolute path to somebody's
    // checkout does not belong in a file they commit.
    let manifest_path = root.join("Cargo.toml");
    let manifest_text =
        std::fs::read_to_string(&manifest_path).map_err(|e| read_error(&manifest_path, &e))?;
    let manifest: DocumentMut = manifest_text
        .parse()
        .map_err(|e| vec![Diagnostic::error("Cargo.toml", format!("{e}")).at("Cargo.toml")])?;
    let local = local_checkout(&manifest);

    let answers = |picks: Vec<&'static str>| Answers {
        name: project.name.clone(),
        title: project.title.clone(),
        picks,
        local: local.clone(),
        git: false,
    };
    let before: BTreeMap<String, String> = create::scaffold(&answers(project.enabled.clone()))?
        .into_iter()
        .collect();
    let after = create::scaffold(&answers(after_picks.clone()))?;

    ui.heading("add", adding.join(", "));

    // Nothing is written until every file has been accounted for. A command
    // that half-adds a module leaves a project in a state neither `add` nor
    // a person can reason about.
    let mut writes: Vec<(PathBuf, String, &'static str)> = Vec::new();
    let mut refusals: Vec<Diagnostic> = Vec::new();
    let mut by_hand: Vec<(String, String)> = Vec::new();
    let mut shared: Vec<(String, String, String)> = Vec::new();

    for (path, text) in &after {
        let full = root.join(path);
        let on_disk = std::fs::read_to_string(&full).ok();
        match owned::kind(path) {
            Kind::Owned => {
                if let Some(current) = &on_disk {
                    match owned::unedited(current) {
                        Some(true) => {}
                        // The developer has already been shown what they
                        // are discarding, on the run that refused.
                        _ if args.force => {}
                        Some(false) => {
                            // Against what the generator last wrote, not
                            // against what it would write now: the reader
                            // is looking for their own edit, and mixing
                            // the new module into it hides the thing they
                            // have to decide about.
                            let was = before.get(path).unwrap_or(text);
                            refusals.push(edited(path, current, was));
                        }
                        None => refusals.push(unstamped(path)),
                    }
                }
                if on_disk.as_deref() != Some(text.as_str()) {
                    let what = if on_disk.is_some() {
                        "rewritten"
                    } else {
                        "written"
                    };
                    writes.push((full, text.clone(), what));
                }
            }
            Kind::Shared => match on_disk {
                Some(current) => shared.push((path.clone(), current, text.clone())),
                // A config file for an environment this project does not
                // have yet. Nothing of the developer's is in it, so it
                // arrives whole rather than a section at a time.
                None => writes.push((full, text.clone(), "written")),
            },
            Kind::Theirs => match (&on_disk, before.get(path)) {
                (None, _) => writes.push((full, text.clone(), "written")),
                (Some(_), Some(was)) if was == text => {}
                (Some(_), was) => {
                    let was = was.cloned().unwrap_or_default();
                    if path == "src/app.rs" {
                        by_hand.push((path.clone(), owned::diff(&was, text)));
                    } else {
                        refusals.push(theirs(path, &was, text));
                    }
                }
            },
        }
    }

    if !refusals.is_empty() {
        refusals.push(
            Diagnostic::warning("nothing written", "the project is exactly as it was")
                .note("`add` regenerates the project and writes only what it owns"),
        );
        return Err(refusals);
    }

    // The two shared files, appended to.
    for (path, current, generated) in &shared {
        let merged = if path == "Cargo.toml" {
            merge_manifest(path, current, generated)?
        } else {
            merge_config(path, current, generated)?
        };
        if merged != *current {
            writes.push((root.join(path), merged, "appended to"));
        }
    }

    let mut table = Table::new();
    for (full, text, what) in &writes {
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| write_error(parent, &e))?;
        }
        std::fs::write(full, text).map_err(|e| write_error(full, &e))?;
        let shown = full.strip_prefix(&root).unwrap_or(full);
        table.row(vec![
            Cell::styled(shown.display().to_string(), ACCENT),
            Cell::styled((*what).to_string(), DIM).right(),
        ]);
    }
    ui.table_note(&table);

    for (path, patch) in &by_hand {
        ui.gap();
        ui.warn(format!("{path} is yours, and one line belongs in it"));
        ui.note(format!(
            "  {}",
            crate::ui::paint(
                DIM,
                "a mount is a decision, so the generator does not make it"
            )
        ));
        ui.gap();
        for line in patch.lines() {
            let style = if line.starts_with('+') { ACCENT } else { DIM };
            ui.note(format!("    {}", crate::ui::paint(style, line)));
        }
    }

    ui.gap();
    let mut contents = Table::new().indent(4);
    for key in &after_picks {
        let style = if adding.contains(key) { ACCENT } else { DIM };
        contents.row(vec![
            Cell::styled(*key, style),
            Cell::styled(create::about(key).gist, DIM),
        ]);
    }
    ui.table_note(&contents);

    ui.gap();
    ui.done(format!(
        "{} {}",
        crate::ui::paint(crate::ui::PLAIN, "added"),
        crate::ui::paint(ACCENT, adding.join(", "))
    ));
    ui.gap();
    let mut next = Table::new().indent(4);
    next.row(vec![
        Cell::styled("cargo build", STRONG),
        Cell::styled("the new dependency is fetched here", DIM),
    ]);
    ui.table_note(&next);
    Ok(ExitCode::SUCCESS)
}

// -- what the project says about itself -------------------------------------

/// What `brocanth.toml` records.
#[derive(Debug)]
struct Record {
    name: String,
    title: String,
    enabled: Vec<&'static str>,
}

fn read_record(text: &str) -> Result<Record, Vec<Diagnostic>> {
    let doc: DocumentMut = text.parse().map_err(|e| {
        vec![Diagnostic::error("brocanth.toml", format!("{e}")).at("brocanth.toml")]
    })?;
    let name = doc
        .get("project")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| {
            vec![Diagnostic::error("brocanth.toml", "no `[project] name`").at("brocanth.toml")]
        })?
        .to_string();

    let Some(listed) = doc
        .get("modules")
        .and_then(|m| m.get("enabled"))
        .and_then(|e| e.as_array())
    else {
        // A project scaffolded before the marker held the module list. The
        // list cannot be worked out from the source without a parser
        // guessing at a decision somebody already made, so it is asked for.
        return Err(vec![Diagnostic::error(
            "brocanth.toml",
            "this project has no `[modules] enabled` list",
        )
        .at("brocanth.toml")
        .note("it was scaffolded before `brocanth add` existed, so nothing recorded what it is made of")
        .help("add the list by hand -- `[modules]` then `enabled = [\"logging\", ..]` -- naming what `brocanth create` was asked for")]);
    };

    let mut enabled = Vec::new();
    let mut unknown = Vec::new();
    for value in listed.iter() {
        let Some(key) = value.as_str() else { continue };
        match create::all().into_iter().find(|k| *k == key) {
            Some(key) => enabled.push(key),
            None => unknown.push(key.to_string()),
        }
    }
    if !unknown.is_empty() {
        return Err(vec![Diagnostic::error(
            "brocanth.toml",
            format!(
                "no module in this brocanth is called {}",
                unknown.join(", ")
            ),
        )
        .at("brocanth.toml")
        .note("the project names a module this build of the command line does not have")
        .help(
            "install the brocanth that made this project, or the crate that module lives in",
        )]);
    }

    let title = doc
        .get("project")
        .and_then(|p| p.get("title"))
        .and_then(|t| t.as_str())
        .map_or_else(|| crate::ui::title_case(&name), str::to_string);
    Ok(Record {
        name,
        title,
        enabled,
    })
}

/// The modules named on the command line, as the registry spells them.
fn named(words: &[String]) -> Result<Vec<&'static str>, Vec<Diagnostic>> {
    let mut out = Vec::new();
    let mut wrong = Vec::new();
    for word in words {
        match create::all().into_iter().find(|key| key == word) {
            Some(key) if !out.contains(&key) => out.push(key),
            Some(_) => {}
            None => wrong.push(
                Diagnostic::error(word.clone(), format!("`{word}` is not a module"))
                    .help("`brocanth add --help` lists what this build can add"),
            ),
        }
    }
    if wrong.is_empty() {
        Ok(out)
    } else {
        Err(wrong)
    }
}

/// `--brocanth-path`, read back out of the project's own manifest.
fn local_checkout(manifest: &DocumentMut) -> Option<String> {
    manifest
        .get("dependencies")?
        .get("brocanth")?
        .get("path")?
        .as_str()
        .map(str::to_string)
}

// -- the two files both sides write in --------------------------------------

/// The project's `Cargo.toml` with the new dependencies in it, and every
/// other byte where it was.
///
/// Key by key rather than table by table: the item is moved across with the
/// decoration it was generated with, so a dependency keeps the trailing
/// comment saying why it is there.
fn merge_manifest(path: &str, current: &str, generated: &str) -> Result<String, Vec<Diagnostic>> {
    let mut doc: DocumentMut = current.parse().map_err(|e| toml_error(path, &e))?;
    let want: DocumentMut = generated.parse().map_err(|e| toml_error(path, &e))?;

    if let Some(deps) = want.get("dependencies").and_then(Item::as_table) {
        let have = doc
            .entry("dependencies")
            .or_insert(Item::Table(Default::default()));
        if let Some(have) = have.as_table_mut() {
            for (key, item) in deps.iter() {
                if have.contains_key(key) {
                    continue;
                }
                have.insert(key, item.clone());
            }
            // The receipt above `[dependencies]` names every capability the
            // project does *not* have, so it goes stale the moment one
            // arrives. It is generated prose with nothing of the
            // developer's in it, so it is replaced rather than left lying.
            if let Some(fresh) = deps.decor().prefix().and_then(|p| p.as_str()) {
                have.decor_mut().set_prefix(fresh.to_string());
            }
        }
    }

    // `cargo run` with two binaries and no default cannot choose, so a
    // module that brings a second one brings this with it.
    if let Some(run) = want.get("package").and_then(|p| p.get("default-run")) {
        if let Some(package) = doc.get_mut("package").and_then(Item::as_table_mut) {
            if !package.contains_key("default-run") {
                package.insert("default-run", run.clone());
            }
        }
    }

    // A second compile target a module asked for.
    if let Some(bins) = want.get("bin").and_then(Item::as_array_of_tables) {
        let named = |doc: &DocumentMut, name: &str| {
            doc.get("bin")
                .and_then(Item::as_array_of_tables)
                .is_some_and(|all| {
                    all.iter()
                        .any(|t| t.get("name").and_then(|n| n.as_str()) == Some(name))
                })
        };
        for table in bins.iter() {
            let Some(name) = table.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            if named(&doc, name) {
                continue;
            }
            let entry = doc
                .entry("bin")
                .or_insert(Item::ArrayOfTables(toml_edit::ArrayOfTables::new()));
            if let Some(all) = entry.as_array_of_tables_mut() {
                all.push(table.clone());
            }
        }
    }
    Ok(doc.to_string())
}

/// The project's `config/<env>.toml` with the new sections appended.
///
/// A section the file already has is left exactly as it is, comments,
/// values and all: the developer has had a chance to set the URL in it, and
/// a generator that reset it to `postgres://localhost/development` would be
/// a generator nobody runs twice.
fn merge_config(path: &str, current: &str, generated: &str) -> Result<String, Vec<Diagnostic>> {
    let mut doc: DocumentMut = current.parse().map_err(|e| toml_error(path, &e))?;
    let want: DocumentMut = generated.parse().map_err(|e| toml_error(path, &e))?;
    for (key, item) in want.iter() {
        if doc.contains_key(key) {
            continue;
        }
        doc.insert(key, item.clone());
    }
    Ok(doc.to_string())
}

// -- the refusals -----------------------------------------------------------

fn edited(path: &str, current: &str, was: &str) -> Diagnostic {
    let body = owned::unstamp(current).map_or_else(|| current.to_string(), |(body, _)| body);
    let was = owned::unstamp(was).map_or_else(|| was.to_string(), |(body, _)| body);
    Diagnostic::error("edited", format!("{path} has been edited"))
        .at(path)
        .note("`brocanth add` rewrites this file, and the edit would go with it")
        .note(owned::diff(&was, &body))
        .help("move the edit into a file of your own, or pass --force to discard it")
}

fn unstamped(path: &str) -> Diagnostic {
    Diagnostic::error("no digest", format!("{path} carries no digest"))
        .at(path)
        .note("`brocanth add` rewrites this file and cannot tell whether anybody changed it")
        .help("check it reads the way brocanth wrote it, then pass --force")
}

fn theirs(path: &str, was: &str, now: &str) -> Diagnostic {
    Diagnostic::error("yours", format!("{path} would have to change, and it is yours"))
        .at(path)
        .note("`brocanth add` writes a file of yours once and never rewrites it")
        .note(owned::diff(was, now))
        .help("apply this by hand and run the command again, or scaffold a new project with `brocanth create`")
}

fn toml_error(path: &str, e: &toml_edit::TomlError) -> Vec<Diagnostic> {
    vec![Diagnostic::error("toml", format!("{e}")).at(path.to_string())]
}

fn read_error(path: &Path, e: &std::io::Error) -> Vec<Diagnostic> {
    vec![Diagnostic::error("read", format!("{e}")).at(path.display().to_string())]
}

fn write_error(path: &Path, e: &std::io::Error) -> Vec<Diagnostic> {
    vec![Diagnostic::error("write", format!("{e}")).at(path.display().to_string())]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> Ui {
        Ui::new(crate::ui::Face::Plain, true, false)
    }

    /// A project on disk, made the way `brocanth create` makes one.
    fn project(name: &str, picks: &[&'static str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "brocanth-add-{}-{name}-{:?}",
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

    fn add(root: &Path, module: &str, force: bool) -> Outcome {
        run(
            &ui(),
            Some(root),
            AddArgs {
                modules: vec![module.to_string()],
                force,
            },
        )
    }

    fn read(root: &Path, path: &str) -> String {
        std::fs::read_to_string(root.join(path)).unwrap_or_default()
    }

    /// Whether the manifest *depends* on a crate. Not whether it mentions
    /// one: the receipt above `[dependencies]` names every crate the
    /// project does not have, which is the point of it.
    fn depends_on(root: &Path, name: &str) -> bool {
        read(root, "Cargo.toml")
            .lines()
            .any(|line| line.starts_with(&format!("{name} = ")))
    }

    #[test]
    fn adding_a_module_writes_what_creating_with_it_would_have_written() {
        // The claim the whole command rests on: one generator, and `add` is
        // the same generator with one more entry on the list. Checked file
        // by file against a project `create` made with both modules, which
        // is the thing a developer would otherwise do by hand.
        let root = project("worky", &["logging", "config"]);
        add(&root, "cache", false).expect("cache goes in");

        let both = Answers {
            name: "worky".to_string(),
            title: "Worky".to_string(),
            picks: vec!["logging", "config", "cache"],
            local: None,
            git: false,
        };
        for (path, want) in create::scaffold(&both).expect("the modules agree") {
            // `Cargo.toml` and the config are appended to rather than
            // rewritten, so they hold the same facts in a different order.
            // Every other file is byte for byte what `create` would write.
            if owned::kind(&path) == Kind::Shared {
                continue;
            }
            assert_eq!(read(&root, &path), want, "{path}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_appended_manifest_holds_the_new_crate_and_nothing_else_moved() {
        let root = project("worky", &["logging", "config"]);
        let before = read(&root, "Cargo.toml");
        add(&root, "cache", false).expect("cache goes in");
        let after = read(&root, "Cargo.toml");

        assert!(depends_on(&root, "brocanth-cache"), "{after}");
        // Every line the manifest had is still there, except the receipt's
        // own line about the cache being absent -- which is the one line
        // that stopped being true.
        for line in before.lines() {
            if line.contains("brocanth-cache") {
                continue;
            }
            assert!(after.contains(line), "{line:?} went missing:\n{after}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_config_section_the_developer_has_already_set_is_left_alone() {
        let root = project("worky", &["logging", "config"]);
        let path = root.join("config/development.toml");
        let edited = read(&root, "config/development.toml").replace("debug", "warn");
        std::fs::write(&path, &edited).expect("write");

        add(&root, "cache", false).expect("cache goes in");
        let after = read(&root, "config/development.toml");
        assert!(after.contains("level = \"warn\""), "{after}");
        assert!(after.contains("[cache]"), "{after}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_edited_generated_file_stops_the_command_and_nothing_is_written() {
        let root = project("worky", &["logging", "config"]);
        let path = root.join("src/plugins/mod.rs");
        let edited = format!("{}pub mod mine;\n", read(&root, "src/plugins/mod.rs"));
        std::fs::write(&path, &edited).expect("write");

        let refused = add(&root, "cache", false).expect_err("the edit is in the way");
        let said = refused
            .iter()
            .map(crate::ui::Diagnostic::render)
            .collect::<String>();
        assert!(said.contains("has been edited"), "{said}");
        assert!(
            said.contains("pub mod mine"),
            "the edit is not shown: {said}"
        );
        // Nothing at all: not the plugin file, not the manifest.
        assert_eq!(read(&root, "src/plugins/mod.rs"), edited);
        assert!(!root.join("src/plugins/cache").exists());
        assert!(!depends_on(&root, "brocanth-cache"));

        add(&root, "cache", true).expect("--force discards it");
        assert!(!read(&root, "src/plugins/mod.rs").contains("pub mod mine"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_module_that_would_rewrite_a_file_of_the_developers_is_refused() {
        // `config` changes whether every *other* plugin's settings struct
        // derives `Deserialize`, and those structs are in files that
        // belong to the developer. The command says which, and shows the
        // change it would have made.
        let root = project("worky", &["logging"]);
        let refused = add(&root, "config", false).expect_err("logging would have to change");
        let said = refused
            .iter()
            .map(crate::ui::Diagnostic::render)
            .collect::<String>();
        assert!(said.contains("src/plugins/logging/mod.rs"), "{said}");
        assert!(said.contains("serde::Deserialize"), "{said}");
        assert!(!depends_on(&root, "brocanth-config"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_project_with_no_recorded_module_list_is_asked_rather_than_guessed_at() {
        let older = "[project]\nname = \"worky\"\n";
        let refused = read_record(older).expect_err("nothing says what it is made of");
        let said = refused[0].render();
        assert!(said.contains("[modules] enabled"), "{said}");
    }

    #[test]
    fn the_record_reads_the_list_back_in_the_registry_s_own_words() {
        let text = "[project]\nname = \"worky\"\ntitle = \"Worky\"\n\
                    \n[modules]\nenabled = [\"config\", \"logging\"]\n";
        let record = read_record(text).expect("a module list");
        assert_eq!(record.name, "worky");
        assert_eq!(record.title, "Worky");
        assert_eq!(record.enabled, ["config", "logging"]);
    }

    #[test]
    fn a_module_this_build_does_not_have_is_named_rather_than_skipped() {
        let text = "[project]\nname = \"worky\"\n\
                    \n[modules]\nenabled = [\"logging\", \"stripe\"]\n";
        let refused = read_record(text).expect_err("`stripe` is nobody's module here");
        let said = refused[0].render();
        assert!(said.contains("stripe"), "{said}");
    }

    /// A project, a module added to it, and cargo.
    ///
    /// Ignored because it runs cargo and wants the registry. It is the test
    /// that matters most all the same -- green tests that bypass the
    /// production path are not evidence -- so it is a command rather than a
    /// paragraph: `cargo test -p brocanth-cli -- --ignored`.
    #[test]
    #[ignore = "runs cargo build over a generated application"]
    fn a_project_with_a_module_added_to_it_compiles() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/");
        let brocanth = crates
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-add-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let answers = Answers {
            name: "added".to_string(),
            title: "Added".to_string(),
            picks: vec!["logging", "config", "commands"],
            local: Some(brocanth.display().to_string()),
            git: false,
        };
        for (path, text) in create::scaffold(&answers).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().expect("a directory")).expect("mkdir");
            std::fs::write(&full, text).expect("write");
        }
        add(&root, "sessions", false).expect("sessions goes in");

        let out =
            std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .arg("build")
                .current_dir(&root)
                .output()
                .expect("run cargo");
        assert!(
            out.status.success(),
            "the project did not compile after `add`:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn adding_a_module_that_is_already_there_changes_nothing() {
        let root = project("worky", &["logging", "config"]);
        let before = read(&root, "brocanth.toml");
        add(&root, "logging", false).expect("already there is not an error");
        assert_eq!(read(&root, "brocanth.toml"), before);
        let _ = std::fs::remove_dir_all(&root);
    }
}
