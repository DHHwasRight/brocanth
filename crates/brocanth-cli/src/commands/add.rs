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
//!   `src/commands/mod.rs` ends with a part of the developer's, below
//!   [`owned::YOURS`], which the digest does not cover and the rewrite
//!   keeps.
//! * **Shared** -- `Cargo.toml` and `config/<env>.toml`. Both sides write in
//!   them, so this appends and leaves every other byte alone: comments,
//!   and the spec of a dependency already there.
//! * **The developer's** -- everything else. Written when it is new with
//!   this add; a file the generator wrote before and the developer has
//!   since deleted stays deleted.
//!   Rewritten only while it still holds, byte for byte, what the
//!   generator wrote for the modules recorded before this add: then
//!   nobody has edited it, and rewriting it loses nothing.
//!
//! Every module is generated again, not only the new one. So a module
//! that adapts to the one being added (`admin` to `commands`, `ci-github`
//! to `jobs`) writes its adaptation too, as `create` would have.
//!
//! # When it refuses
//!
//! A module whose arrival would change a file the developer has edited.
//! The change is a wiring line the new module needs (a `mod` line, a
//! route, a derive), so a project without it does not compile or does not
//! run the module, and half an add is worse than none. The command prints
//! the change and writes nothing. When the developer has made the change
//! by hand, around their own lines, the next run sees every added line in
//! place and goes through.
//!
//! `src/app.rs` is the one exception, and it is the design rather than a
//! hole in it. What lands there is a middleware mount, and a mount is a
//! decision: the pattern is the application's to choose, which is why that
//! file is the developer's in the first place. A project without the mount
//! compiles and runs; it just does not run that layer. So when the file
//! was edited, the lines are printed for the developer to place, and the
//! command carries on.

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
        required_unless_present = "questions",
        value_delimiter = ',',
        value_parser = create::offered()
    )]
    pub modules: Vec<String>,
    /// Rewrite a generated file that was edited, discarding the edit
    #[arg(long)]
    pub force: bool,
    /// Print the chooser's questions, with what this project has, and add nothing
    #[arg(long, conflicts_with = "modules")]
    pub questions: bool,
    /// With --questions: print JSON instead of text
    #[arg(long, requires = "questions")]
    pub json: bool,
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
    if args.questions {
        let project = read_record(&recorded)?;
        crate::questions::print(ui, args.json, &project.enabled, Some(&project.enabled));
        return Ok(ExitCode::SUCCESS);
    }
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
    create::selection(&after_picks, create::Door::Add)?;

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

    let mut kept_deleted: Vec<&str> = Vec::new();
    for (path, text) in &after {
        let full = root.join(path);
        let on_disk = std::fs::read_to_string(&full).ok();
        match owned::kind(path) {
            Kind::Owned => {
                // The lines below `owned::YOURS` are the developer's, and
                // they come along into the rewritten file.
                let text = &owned::keep_yours(path, on_disk.as_deref().unwrap_or(""), text);
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
                // The generator wrote it before this add, and it is gone:
                // the developer deleted it. A deleted example page is a
                // decision, and an add that brought it back would undo it.
                (None, Some(_)) => kept_deleted.push(path),
                (None, None) => writes.push((full, text.clone(), "written")),
                // Already what the generator writes: unchanged by this
                // module, or changed by hand to match it.
                (Some(current), _) if current == text => {}
                (Some(_), Some(was)) if was == text => {}
                // Byte for byte what the generator wrote before this add.
                // Nobody has edited it, so rewriting it loses nothing.
                (Some(current), Some(was)) if current == was => {
                    writes.push((full, text.clone(), "rewritten"));
                }
                // Edited, and the developer has already made this module's
                // change in it, around their own lines.
                (Some(current), Some(was)) if applied(current, was, text) => {}
                // Kept by an earlier `brocanth remove`: the migrations are
                // the database's history and the `migrate` tool's list, and
                // the flags file holds the developer's renames. Both are
                // theirs, and this add writes around them.
                (Some(_), None) if path.starts_with("migrations/") || path == "src/flags.rs" => {}
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

    // Renames `brocanth remove` parked, for flags that are back.
    let flags = root.join("src/flags.rs");
    if let Ok(current) = std::fs::read_to_string(&flags) {
        let present = create::setting_paths(&answers(after_picks.clone()))?;
        let restored = crate::commands::remove::unpark(&current, &present);
        if restored != current {
            writes.push((flags, restored, "renames restored"));
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
    // Screens ejected before a `brocanth remove admin`, installed again.
    if adding.contains(&"admin") {
        for path in crate::commands::admin::rewire(&root)? {
            table.row(vec![
                Cell::styled(path, ACCENT),
                Cell::styled("ejected screens installed".to_string(), DIM).right(),
            ]);
        }
    }
    ui.table_note(&table);
    if !kept_deleted.is_empty() {
        ui.note(format!(
            "  {}",
            crate::ui::paint(
                DIM,
                format!(
                    "not written again, because you deleted it: {}",
                    kept_deleted.join(", ")
                )
            )
        ));
    }

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

    // `add` asks nobody, so what the modules now in the project recommend
    // is said here, as `create` says it after a run driven by flags.
    crate::questions::warn_wanted(ui, &after_picks);

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
pub(crate) struct Record {
    pub(crate) name: String,
    pub(crate) title: String,
    pub(crate) enabled: Vec<&'static str>,
}

pub(crate) fn read_record(text: &str) -> Result<Record, Vec<Diagnostic>> {
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
pub(crate) fn named(words: &[String]) -> Result<Vec<&'static str>, Vec<Diagnostic>> {
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
pub(crate) fn local_checkout(manifest: &DocumentMut) -> Option<String> {
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
                if let Some(old) = have.get_mut(key) {
                    // The developer's spec stays: a `branch`, a `rev`, a
                    // version they pinned. Only a feature the new module
                    // needs is added to it.
                    add_features(old, item);
                    continue;
                }
                let item = same_source(have, item);
                have.insert(key, item);
            }
            // The receipt above `[dependencies]` names every capability the
            // project does *not* have, so a line of it goes stale the
            // moment that capability arrives. Only those lines go: the
            // developer may have written comments of their own around it.
            if let Some(fresh) = deps.decor().prefix().and_then(|p| p.as_str()) {
                let current = have
                    .decor()
                    .prefix()
                    .and_then(|p| p.as_str())
                    .unwrap_or("")
                    .to_string();
                have.decor_mut().set_prefix(receipt(&current, fresh));
            }
        }
    }

    // What a module's `build.rs` uses. No receipt here: the receipt is
    // about `[dependencies]`.
    if let Some(deps) = want.get("build-dependencies").and_then(Item::as_table) {
        // A pinned brocanth source is pinned in `[dependencies]`.
        let pins = doc
            .get("dependencies")
            .and_then(Item::as_table)
            .cloned()
            .unwrap_or_default();
        let have = doc
            .entry("build-dependencies")
            .or_insert(Item::Table(Default::default()));
        if let Some(have) = have.as_table_mut() {
            for (key, item) in deps.iter() {
                if let Some(old) = have.get_mut(key) {
                    add_features(old, item);
                    continue;
                }
                have.insert(key, same_source(&pins, item));
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

/// The receipt in `current` with the lines `fresh` no longer has taken out,
/// and every other line, the developer's included, left where it was.
///
/// A receipt line is one in the block under `# Not here`, indented by
/// three spaces. Outside that block nothing is touched.
pub(crate) fn receipt(current: &str, fresh: &str) -> String {
    let mut out = String::with_capacity(current.len());
    let mut in_block = false;
    for line in current.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if bare.starts_with("# Not here") {
            in_block = true;
        } else if in_block && bare.starts_with("#   ") {
            if !fresh.lines().any(|l| l == bare) {
                continue;
            }
        } else {
            in_block = false;
        }
        out.push_str(line);
    }
    out
}

/// Add to `old` every feature `new` asks for that it does not have.
pub(crate) fn add_features(old: &mut Item, new: &Item) {
    let Some(wanted) = new
        .as_table_like()
        .and_then(|t| t.get("features"))
        .and_then(Item::as_array)
    else {
        return;
    };
    let Some(old) = old.as_table_like_mut() else {
        return;
    };
    if old.get("features").is_none() {
        old.insert("features", toml_edit::value(toml_edit::Array::new()));
    }
    let Some(have) = old.get_mut("features").and_then(Item::as_array_mut) else {
        return;
    };
    for feature in wanted.iter() {
        let Some(name) = feature.as_str() else {
            continue;
        };
        if !have.iter().any(|f| f.as_str() == Some(name)) {
            have.push(name);
        }
    }
}

/// `item`, a new git dependency, from the same `branch`, `rev` or `tag` as
/// a dependency the project already takes from that repository.
///
/// Cargo treats one repository at two refs as two sources, so a new
/// brocanth crate from the default branch beside a `branch = "main"` core
/// is a second copy of the core, and the types do not meet.
fn same_source(deps: &toml_edit::Table, item: &Item) -> Item {
    let mut item = item.clone();
    let Some(url) = item
        .as_table_like()
        .and_then(|t| t.get("git"))
        .and_then(Item::as_str)
        .map(str::to_string)
    else {
        return item;
    };
    let Some(pinned) = deps.iter().find_map(|(_, dep)| {
        let dep = dep.as_table_like()?;
        (dep.get("git")?.as_str()? == url).then(|| {
            ["branch", "rev", "tag"]
                .into_iter()
                .filter_map(|key| {
                    dep.get(key)
                        .and_then(Item::as_value)
                        .map(|v| (key, v.clone()))
                })
                .collect::<Vec<_>>()
        })
    }) else {
        return item;
    };
    if let Some(inline) = item.as_inline_table_mut() {
        // Rebuilt in order, so the ref sits beside the `git` it qualifies.
        let mut out = toml_edit::InlineTable::new();
        for (key, value) in inline.iter() {
            out.insert(key, value.clone());
            if key == "git" {
                for (k, v) in &pinned {
                    out.insert(*k, v.clone().decorated(" ", ""));
                }
            }
        }
        *out.decor_mut() = inline.decor().clone();
        *inline = out;
    } else if let Some(table) = item.as_table_like_mut() {
        for (k, v) in pinned {
            table.insert(k, Item::Value(v));
        }
    }
    item
}

/// The project's `config/<env>.toml` with the new sections appended.
///
/// A section the file already has is left exactly as it is, comments,
/// values and all: the developer has had a chance to set the URL in it, and
/// a generator that reset it to `postgres://localhost/development` would be
/// a generator nobody runs twice.
///
/// Appended as text at the end of the file, after a blank line. Inserted
/// through the document instead, a section lands before the file's
/// trailing comments, which belong to the section above it: the new
/// header would cut that section's comments in two.
fn merge_config(path: &str, current: &str, generated: &str) -> Result<String, Vec<Diagnostic>> {
    let doc: DocumentMut = current.parse().map_err(|e| toml_error(path, &e))?;
    let want: DocumentMut = generated.parse().map_err(|e| toml_error(path, &e))?;
    let mut out = current.to_string();
    for (key, item) in want.iter() {
        if doc.contains_key(key) {
            continue;
        }
        let mut one = DocumentMut::new();
        one.insert(key, item.clone());
        let text = one.to_string();
        if !out.is_empty() {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            if !out.ends_with("\n\n") {
                out.push('\n');
            }
        }
        out.push_str(text.trim_start_matches(['\n', '\r']));
    }
    // Still a document, and still holding every key it held.
    out.parse::<DocumentMut>()
        .map_err(|e| toml_error(path, &e))?;
    Ok(out)
}

// -- the refusals -----------------------------------------------------------

pub(crate) fn edited(path: &str, current: &str, was: &str) -> Diagnostic {
    let body = owned::unstamp(current).map_or_else(|| current.to_string(), |(body, _)| body);
    let was = owned::unstamp(was).map_or_else(|| was.to_string(), |(body, _)| body);
    let help = if path == "src/commands/mod.rs" {
        format!(
            "move your `pub mod` lines below the line `{}`, at the end of the file; `add` keeps them there",
            owned::YOURS
        )
    } else {
        "move the edit into a file of your own, or pass --force to discard it".to_string()
    };
    Diagnostic::error("edited", format!("{path} has been edited"))
        .at(path)
        .note("`brocanth add` rewrites this file, and the edit would go with it")
        .note(owned::diff(&was, &body))
        .help(help)
}

pub(crate) fn unstamped(path: &str) -> Diagnostic {
    Diagnostic::error("no digest", format!("{path} carries no digest"))
        .at(path)
        .note("`brocanth add` rewrites this file and cannot tell whether anybody changed it")
        .help("check it reads the way brocanth wrote it, then pass --force")
}

/// Whether an edited file already holds the change from `was` to `now`:
/// every line the generator adds is in it, and no line it removes is.
///
/// Lines, not a patch: the developer applied the change by hand, around
/// lines of their own, so its position is theirs to choose.
pub(crate) fn applied(current: &str, was: &str, now: &str) -> bool {
    let has = |text: &str, line: &str| text.lines().any(|l| l == line);
    now.lines()
        .filter(|line| !has(was, line))
        .all(|line| has(current, line))
        && was
            .lines()
            .filter(|line| !has(now, line) && !line.trim().is_empty())
            .all(|line| !has(current, line))
}

fn theirs(path: &str, was: &str, now: &str) -> Diagnostic {
    Diagnostic::error(
        "yours",
        format!("{path} would have to change, and it is yours"),
    )
    .at(path)
    .note("you have edited it, and `brocanth add` never rewrites an edit of yours")
    .note(owned::diff(was, now))
    .help("make this change by hand, keeping your own lines, and run the command again")
}

pub(crate) fn toml_error(path: &str, e: &toml_edit::TomlError) -> Vec<Diagnostic> {
    vec![Diagnostic::error("toml", format!("{e}")).at(path.to_string())]
}

pub(crate) fn read_error(path: &Path, e: &std::io::Error) -> Vec<Diagnostic> {
    vec![Diagnostic::error("read", format!("{e}")).at(path.display().to_string())]
}

pub(crate) fn write_error(path: &Path, e: &std::io::Error) -> Vec<Diagnostic> {
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
                questions: false,
                json: false,
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
        // belong to the developer. Once the developer has edited one, the
        // command says which, and shows the change it would have made.
        let root = project("worky", &["logging"]);
        let logging = root.join("src/plugins/logging/mod.rs");
        let edited = format!("{}// mine\n", read(&root, "src/plugins/logging/mod.rs"));
        std::fs::write(&logging, &edited).expect("write");
        let refused = add(&root, "config", false).expect_err("logging would have to change");
        let said = refused
            .iter()
            .map(crate::ui::Diagnostic::render)
            .collect::<String>();
        assert!(said.contains("src/plugins/logging/mod.rs"), "{said}");
        assert!(said.contains("serde::Deserialize"), "{said}");
        assert!(!depends_on(&root, "brocanth-config"));
        assert_eq!(read(&root, "src/plugins/logging/mod.rs"), edited);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn config_goes_into_a_project_whose_plugins_are_as_created() {
        // The same change as above, into files nobody has touched: the
        // generator wrote them, so it may write them again.
        let root = project("worky", &["logging"]);
        if let Err(e) = add(&root, "config", false) {
            panic!("{}", said(&e));
        }
        assert!(read(&root, "src/plugins/logging/mod.rs").contains("serde::Deserialize"));
        assert!(depends_on(&root, "brocanth-config"));
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

    /// What `create` makes when nobody answers a question.
    const DEFAULT: &[&str] = &[
        "observability",
        "observability-logs",
        "observability-health",
        "observability-metrics",
        "config",
        "commands",
        "docker",
        "ci",
        "ci-github",
        "middleware",
        "middleware-security-headers",
    ];

    fn said(diagnostics: &[Diagnostic]) -> String {
        diagnostics.iter().map(Diagnostic::render).collect()
    }

    /// What `create` would have written with these modules, for every file
    /// `add` writes whole: the shared files are appended to instead, so
    /// they hold the same facts in another order.
    fn as_created(name: &str, picks: &[&'static str]) -> Vec<(String, String)> {
        let answers = Answers {
            name: name.to_string(),
            title: crate::ui::title_case(name),
            picks: create::all()
                .into_iter()
                .filter(|key| picks.contains(key))
                .collect(),
            local: None,
            git: false,
        };
        create::scaffold(&answers)
            .expect("the modules agree")
            .into_iter()
            .filter(|(path, _)| owned::kind(path) != Kind::Shared)
            .collect()
    }

    fn with(picks: &[&'static str], more: &'static str) -> Vec<&'static str> {
        let mut all = picks.to_vec();
        all.push(more);
        all
    }

    #[test]
    fn a_page_module_goes_into_a_project_whose_pages_are_as_created() {
        // `src/pages/mod.rs` is the developer's, but nobody has touched it:
        // it holds what the generator wrote. Rewriting it loses nothing.
        for module in ["admin", "openapi", "resource"] {
            let name = format!("pages{module}");
            let root = project(&name, DEFAULT);
            if let Err(e) = add(&root, module, false) {
                panic!("{module}: {}", said(&e));
            }
            for (path, want) in as_created(&name, &with(DEFAULT, module)) {
                assert_eq!(read(&root, &path), want, "{module}: {path}");
            }
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn jobs_goes_into_a_default_project_and_the_workflow_follows() {
        let root = project("queue", DEFAULT);
        if let Err(e) = add(&root, "jobs", false) {
            panic!("{}", said(&e));
        }
        for (path, want) in as_created("queue", &with(DEFAULT, "jobs")) {
            assert_eq!(read(&root, &path), want, "{path}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn adding_a_module_reapplies_what_the_others_adapt_to() {
        // `admin` adapts to `commands`: with both, the project gets an
        // `admin eject` subcommand. Adding `commands` later writes it, as
        // `create` with both would have.
        let root = project("later", &["config", "admin"]);
        if let Err(e) = add(&root, "commands", false) {
            panic!("{}", said(&e));
        }
        assert!(root.join("src/commands/admin.rs").exists());
        for (path, want) in as_created("later", &["config", "admin", "commands"]) {
            assert_eq!(read(&root, &path), want, "{path}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_edited_file_of_the_developers_stops_the_add_until_the_change_is_made() {
        let root = project("touched", DEFAULT);
        let pages = root.join("src/pages/mod.rs");
        let edited = format!("{}// mine\n", read(&root, "src/pages/mod.rs"));
        std::fs::write(&pages, &edited).expect("write");

        let refused = said(&add(&root, "admin", false).expect_err("the edit is in the way"));
        assert!(refused.contains("src/pages/mod.rs"), "{refused}");
        assert!(refused.contains("help:"), "{refused}");
        assert_eq!(read(&root, "src/pages/mod.rs"), edited, "an edit was lost");
        assert!(!root.join("src/plugins/admin").exists(), "half an add");

        // The developer makes the change the refusal showed, keeps their
        // own line, and runs the command again.
        let want = as_created("touched", &with(DEFAULT, "admin"))
            .into_iter()
            .find(|(path, _)| path == "src/pages/mod.rs")
            .expect("the pages module")
            .1;
        std::fs::write(&pages, format!("{want}// mine\n")).expect("write");
        if let Err(e) = add(&root, "admin", false) {
            panic!("the change is in place: {}", said(&e));
        }
        assert!(read(&root, "src/pages/mod.rs").ends_with("// mine\n"));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn add_many(root: &Path, modules: &[&str]) -> Outcome {
        run(
            &ui(),
            Some(root),
            AddArgs {
                modules: modules.iter().map(|m| (*m).to_string()).collect(),
                force: false,
                questions: false,
                json: false,
            },
        )
    }

    /// The service that found these: a real one, made without observability.
    const SERVICE: &[&str] = &["config", "db", "middleware", "commands"];

    #[test]
    fn a_command_of_the_developers_survives_an_add_that_rewrites_the_list() {
        // AGENTS.md says a command is a file and a `pub mod` line. The line
        // goes below the marker, and `admin` then rewrites the list above
        // it to add its own `admin eject`.
        let root = project("orgs", SERVICE);
        let path = root.join("src/commands/mod.rs");
        let text = read(&root, "src/commands/mod.rs");
        assert!(text.ends_with(&format!("{}\n", owned::YOURS)), "{text}");
        std::fs::write(&path, format!("{text}pub mod orgs;\n")).expect("write");

        if let Err(e) = add(&root, "admin", false) {
            panic!("{}", said(&e));
        }
        let after = read(&root, "src/commands/mod.rs");
        assert!(after.contains("pub mod admin;"), "{after}");
        assert!(after.ends_with("pub mod orgs;\n"), "{after}");
        assert_eq!(owned::unedited(&after), Some(true), "{after}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_edit_above_the_marker_names_the_place_for_a_command() {
        let root = project("orgsabove", SERVICE);
        let path = root.join("src/commands/mod.rs");
        let text = read(&root, "src/commands/mod.rs")
            .replace("pub mod serve;", "pub mod orgs;\npub mod serve;");
        std::fs::write(&path, &text).expect("write");
        let refused = said(&add(&root, "admin", false).expect_err("an edit above the line"));
        assert!(refused.contains("below the line"), "{refused}");
        assert_eq!(read(&root, "src/commands/mod.rs"), text);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_page_the_developer_deleted_is_not_written_back() {
        let root = project("pruned", SERVICE);
        std::fs::remove_file(root.join("src/pages/about.rs")).expect("rm");
        std::fs::remove_file(root.join("src/pages/users/[id].rs")).expect("rm");

        if let Err(e) = add_many(&root, &["observability", "observability-logs"]) {
            panic!("{}", said(&e));
        }
        assert!(!root.join("src/pages/about.rs").exists(), "about came back");
        assert!(
            !root.join("src/pages/users/[id].rs").exists(),
            "a user page came back"
        );
        // The new module's own files still arrive.
        assert!(root.join("src/plugins/observability/mod.rs").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_manifest_keeps_the_developers_comments_and_git_branch() {
        let root = project("pinned", SERVICE);
        let manifest = read(&root, "Cargo.toml")
            .replace(
                "\"https://github.com/DHHwasRight/brocanth.git\"",
                "\"https://github.com/DHHwasRight/brocanth.git\", branch = \"main\"",
            )
            .replace(
                "[dependencies]\n",
                "# Mine: pinned to main until brocanth tags a release.\n[dependencies]\n",
            )
            .replace("[package]\n", "# Mine: the package.\n[package]\n");
        let manifest = format!("{manifest}# Mine: at the end.\n");
        std::fs::write(root.join("Cargo.toml"), &manifest).expect("write");

        if let Err(e) = add_many(&root, &["observability", "observability-logs"]) {
            panic!("{}", said(&e));
        }
        let after = read(&root, "Cargo.toml");
        for mine in [
            "# Mine: pinned to main until brocanth tags a release.\n[dependencies]\n",
            "# Mine: the package.\n[package]\n",
            "# Mine: at the end.\n",
        ] {
            assert!(after.contains(mine), "{mine:?} went missing:\n{after}");
        }
        let observe = after
            .lines()
            .find(|l| l.starts_with("brocanth-observe = "))
            .unwrap_or_else(|| panic!("no brocanth-observe:\n{after}"));
        assert!(observe.contains("branch = \"main\""), "{observe}");
        assert!(observe.contains("\"logs\""), "{observe}");
        // The core keeps its own spec, untouched.
        assert!(
            after.contains("brocanth = { git = \"https://github.com/DHHwasRight/brocanth.git\", branch = \"main\" }"),
            "{after}"
        );
        // The receipt line about the module that arrived is gone.
        assert!(!after.contains("#   brocanth-observe"), "{after}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_feature_a_new_child_needs_is_added_to_the_dependency_already_there() {
        let root = project(
            "featured",
            &["config", "observability", "observability-health"],
        );
        if let Err(e) = add(&root, "observability-logs", false) {
            panic!("{}", said(&e));
        }
        let observe = read(&root, "Cargo.toml")
            .lines()
            .find(|l| l.starts_with("brocanth-observe = "))
            .map(str::to_string)
            .expect("brocanth-observe");
        assert!(observe.contains("\"logs\""), "{observe}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_new_config_section_goes_after_the_last_sections_comments() {
        let root = project("notes", SERVICE);
        let path = root.join("config/development.toml");
        let before = format!(
            "{}# Mine: about the last section.\n# permissions = \"later\"\n",
            read(&root, "config/development.toml")
        );
        std::fs::write(&path, &before).expect("write");

        if let Err(e) = add_many(&root, &["observability", "observability-logs"]) {
            panic!("{}", said(&e));
        }
        let after = read(&root, "config/development.toml");
        assert!(after.starts_with(&before), "the file above moved:\n{after}");
        assert!(
            after[before.len()..]
                .trim_start()
                .starts_with("[observability]"),
            "{after}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
