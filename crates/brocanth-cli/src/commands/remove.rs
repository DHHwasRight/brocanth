//! `brocanth remove` -- take a module out of a project that already exists.
//!
//! The inverse of [`crate::commands::add`], and the same machine. It takes
//! the module off the list in `brocanth.toml`, asks the generator for the
//! project again, and writes the difference. No list of a module's files is
//! read: the generator is asked what the project is with the module and
//! without it, so `add` and `remove` cannot disagree about what a module
//! wrote.
//!
//! # What it may do
//!
//! The rules of `add`, read in the other direction:
//!
//! * **Owned** files are rewritten whole, or deleted when the module was the
//!   reason for them. A digest that does not match stops the command.
//! * **Shared** files lose what the module put in them and nothing else: its
//!   dependencies and features in `Cargo.toml`, its sections in
//!   `config/<env>.toml`. A section the developer has changed stops the
//!   command, because its values are theirs.
//! * **The developer's** files are rewritten or deleted only while they
//!   hold, byte for byte, what the generator wrote. An edited file stops
//!   the command, and the refusal shows the edit.
//!
//! Every other module is generated again too, so a module that adapted to
//! the one leaving writes its code without it.
//!
//! # What it keeps
//!
//! Three things a remove followed by an add must find again:
//!
//! * **The module file, once it is edited.** `src/modules/<key>.rs` holds
//!   the module's choices: its renames, its hooks, its integrations. An
//!   unedited one is deleted. An edited one stays, out of
//!   `src/modules/mod.rs`, so nothing compiles it, and `add` finds it: it
//!   writes a module file only when it is missing. A rename survives a
//!   remove and an add.
//! * **Ejected admin screens.** The copies under
//!   `src/plugins/admin/screens/` and their wiring stay. They are the
//!   developer's work, and nothing compiles them once the admin plugin is
//!   gone. `add admin` installs them again.
//! * **Migrations.** `migrations/` is the history of a database that still
//!   has the tables. Nothing deletes it, and `add` writes around it.
//!
//! # When it refuses
//!
//! A module that another module needs, or that is the parent of a module
//! that stays: the help line names the modules to remove with it. And a
//! file it would change or delete that the developer has edited.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use toml_edit::{DocumentMut, Item, Value};

use crate::commands::add::{self, read_error, toml_error, write_error};
use crate::commands::create::{self, Answers};
use crate::commands::Outcome;
use crate::owned::{self, Kind};
use crate::ui::{Cell, Diagnostic, Table, Ui, ACCENT, DIM, STRONG};

#[derive(Args)]
pub struct RemoveArgs {
    /// Modules to remove; repeat the argument or separate with commas
    #[arg(
        value_name = "MODULE",
        required = true,
        value_delimiter = ',',
        value_parser = create::offered()
    )]
    pub modules: Vec<String>,
    /// Rewrite or delete a generated file that was edited, discarding the edit
    #[arg(long)]
    pub force: bool,
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: RemoveArgs) -> Outcome {
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

    let marker = root.join("brocanth.toml");
    let recorded = std::fs::read_to_string(&marker).map_err(|e| read_error(&marker, &e))?;
    if owned::unedited(&recorded) == Some(false) && !args.force {
        return Err(vec![Diagnostic::error(
            "edited",
            "brocanth.toml has been edited since brocanth wrote it",
        )
        .at("brocanth.toml")
        .note("it records what this project is made of, and `remove` reads it before it writes")
        .help("undo the edit, or pass --force to let `remove` rewrite it")]);
    }
    let project = add::read_record(&recorded)?;

    let wanted = add::named(&args.modules)?;
    let removing: Vec<&'static str> = wanted
        .iter()
        .copied()
        .filter(|key| project.enabled.contains(key))
        .collect();
    if removing.is_empty() {
        ui.done(format!("{} is not in this project", wanted.join(", ")));
        return Ok(ExitCode::SUCCESS);
    }
    let after_picks: Vec<&'static str> = create::all()
        .into_iter()
        .filter(|key| project.enabled.contains(key) && !removing.contains(key))
        .collect();
    still_needed(&after_picks, &removing)?;
    create::selection(&after_picks, create::Door::Add)?;

    let manifest_path = root.join("Cargo.toml");
    let manifest_text =
        std::fs::read_to_string(&manifest_path).map_err(|e| read_error(&manifest_path, &e))?;
    let manifest: DocumentMut = manifest_text
        .parse()
        .map_err(|e| vec![Diagnostic::error("Cargo.toml", format!("{e}")).at("Cargo.toml")])?;
    let local = add::local_checkout(&manifest);
    let answers = |picks: Vec<&'static str>| Answers {
        name: project.name.clone(),
        title: project.title.clone(),
        picks,
        local: local.clone(),
        git: false,
    };
    let before_answers = answers(project.enabled.clone());
    let after_answers = answers(after_picks.clone());
    let before: BTreeMap<String, String> = create::scaffold(&before_answers)?.into_iter().collect();
    let after: BTreeMap<String, String> = create::scaffold(&after_answers)?.into_iter().collect();

    ui.heading("remove", removing.join(", "));

    // A project from before `src/modules/`: its `src/flags.rs` moves into
    // the module files, those of the leaving modules included, which stay.
    let before_shapes = create::module_shapes(&before_answers)?;
    let after_shapes = create::module_shapes(&after_answers)?;
    let moved = add::legacy_flags(&root, &before_shapes)?;

    // Nothing is written until every file has been accounted for, as in
    // `add`: half a remove is a project nobody can reason about.
    let mut writes: Vec<(PathBuf, String, &'static str)> = Vec::new();
    let mut deletes: Vec<PathBuf> = Vec::new();
    let mut refusals: Vec<Diagnostic> = Vec::new();
    let mut kept: Vec<(String, String)> = Vec::new();
    let mut shared: Vec<(String, String, String, String)> = Vec::new();

    let paths: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    for path in paths {
        let full = root.join(path);
        let on_disk = std::fs::read_to_string(&full).ok();
        let was = before.get(path);
        let now = after.get(path);

        if path.starts_with("migrations/") && now.is_none() {
            if on_disk.is_some() {
                kept.push((
                    path.clone(),
                    "the history of a database that still has the tables".to_string(),
                ));
            }
            continue;
        }

        match owned::kind(path) {
            // The project's choices. A staying module's file is never
            // rewritten. A leaving module's file goes while it is what the
            // generator wrote, and stays once it holds a choice.
            Kind::Theirs if crate::choices::is_module_file(path) => {
                let legacy = moved.as_ref().and_then(|m| m.files.get(path));
                match (now, was, on_disk.as_deref()) {
                    (Some(text), _, None) => {
                        writes.push((full, legacy.unwrap_or(text).clone(), "written"));
                    }
                    (Some(_), _, Some(current)) => {
                        refusals.extend(stranded_keys(
                            path,
                            current,
                            &before_shapes,
                            &after_shapes,
                        ));
                        if legacy.is_some() {
                            refusals.push(add::legacy_in_the_way(path));
                        }
                    }
                    (None, _, None) => {
                        if let Some(text) = legacy {
                            writes.push((full, text.clone(), "written"));
                            kept.push((path.clone(), returns(path, &removing)));
                        }
                    }
                    (None, Some(was), Some(current)) if current == was && legacy.is_none() => {
                        deletes.push(full);
                    }
                    (None, _, Some(_)) => kept.push((path.clone(), returns(path, &removing))),
                }
            }
            Kind::Owned => match (now, &on_disk) {
                (Some(text), _) => {
                    let text = owned::keep_yours(path, on_disk.as_deref().unwrap_or(""), text);
                    if let Some(current) = &on_disk {
                        match owned::unedited(current) {
                            Some(true) => {}
                            _ if args.force => {}
                            Some(false) => refusals.push(add::edited(
                                path,
                                current,
                                was.map_or(text.as_str(), String::as_str),
                            )),
                            None => refusals.push(add::unstamped(path)),
                        }
                    }
                    if on_disk.as_deref() != Some(text.as_str()) {
                        let what = if on_disk.is_some() {
                            "rewritten"
                        } else {
                            "written"
                        };
                        writes.push((full, text, what));
                    }
                }
                (None, Some(current)) => {
                    match owned::unedited(current) {
                        Some(true) => {}
                        _ if args.force => {}
                        Some(false) => refusals.push(add::edited(
                            path,
                            current,
                            was.map_or("", String::as_str),
                        )),
                        None => refusals.push(add::unstamped(path)),
                    }
                    if owned::yours(current).trim().is_empty() {
                        deletes.push(full);
                    } else {
                        refusals.push(commands_of_yours(path, owned::yours(current)));
                    }
                }
                (None, None) => {}
            },
            Kind::Shared => match (now, on_disk) {
                (Some(text), Some(current)) => shared.push((
                    path.clone(),
                    current,
                    was.cloned().unwrap_or_default(),
                    text.clone(),
                )),
                // A config file the module brought, and nothing else wants.
                (None, Some(current)) => {
                    let was = was.map_or("", String::as_str);
                    if same_document(&current, was) {
                        deletes.push(full);
                    } else {
                        refusals.push(yours_goes(path, was, &current));
                    }
                }
                // New with this remove: nothing of the developer's in it.
                (Some(text), None) if was.is_none() => {
                    writes.push((full, text.clone(), "written"));
                }
                // Gone, and the developer deleted it.
                _ => {}
            },
            Kind::Theirs => match (now, was, on_disk.as_deref()) {
                (Some(text), None, None) => writes.push((full, text.clone(), "written")),
                // The developer deleted it; a remove does not bring it back.
                (_, _, None) => {}
                (Some(text), _, Some(current)) if current == text => {}
                (Some(text), Some(was), Some(_)) if was == text => {}
                (Some(text), Some(was), Some(current)) if current == was => {
                    writes.push((full, text.clone(), "rewritten"));
                }
                (Some(text), Some(was), Some(current)) if add::applied(current, was, text) => {}
                (Some(text), was, Some(_)) => {
                    refusals.push(would_change(path, was.map_or("", String::as_str), text))
                }
                (None, Some(was), Some(current)) if current == was => deletes.push(full),
                (None, Some(was), Some(current))
                    if path == super::admin::PLUGIN && current == super::admin::wired(was) =>
                {
                    deletes.push(full);
                }
                (None, Some(was), Some(current)) => {
                    refusals.push(yours_goes(path, was, current));
                }
                (None, None, Some(_)) => {}
            },
        }
    }

    for (path, current, was, now) in &shared {
        let merged = if path == "Cargo.toml" {
            strip_manifest(path, current, was, now)
        } else {
            strip_config(path, current, was, now)
        };
        match merged {
            Ok(merged) if merged != *current => {
                writes.push((root.join(path), merged, "taken out of"));
            }
            Ok(_) => {}
            Err(d) => refusals.extend(d),
        }
    }

    if !refusals.is_empty() {
        refusals.push(
            Diagnostic::warning("nothing written", "the project is exactly as it was")
                .note("`remove` regenerates the project and changes only what it owns"),
        );
        return Err(refusals);
    }

    let mut table = Table::new();
    for (full, text, what) in &writes {
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| write_error(parent, &e))?;
        }
        std::fs::write(full, text).map_err(|e| write_error(full, &e))?;
        table.row(vec![
            Cell::styled(shown(&root, full), ACCENT),
            Cell::styled((*what).to_string(), DIM).right(),
        ]);
    }
    if moved.is_some() {
        let flags = root.join(crate::choices::LEGACY_FLAGS);
        std::fs::remove_file(&flags).map_err(|e| write_error(&flags, &e))?;
        table.row(vec![
            Cell::styled(crate::choices::LEGACY_FLAGS.to_string(), ACCENT),
            Cell::styled("moved to src/modules/".to_string(), DIM).right(),
        ]);
    }
    for full in &deletes {
        std::fs::remove_file(full).map_err(|e| write_error(full, &e))?;
        prune(&root, full);
        table.row(vec![
            Cell::styled(shown(&root, full), ACCENT),
            Cell::styled("deleted", DIM).right(),
        ]);
    }
    // And a directory the module filled and the developer already emptied.
    for path in before.keys().filter(|path| !after.contains_key(*path)) {
        prune(&root, &root.join(path));
    }
    ui.table_note(&table);

    if removing.contains(&"admin") && root.join(super::admin::SCREENS).is_dir() {
        kept.push((
            super::admin::SCREENS.to_string(),
            "your ejected screens; `brocanth add admin` installs them again".to_string(),
        ));
    }
    for (path, why) in &kept {
        ui.note(format!(
            "  {}",
            crate::ui::paint(DIM, format!("kept {path}: {why}"))
        ));
    }

    ui.gap();
    let mut contents = Table::new().indent(4);
    for key in &after_picks {
        contents.row(vec![
            Cell::styled(*key, DIM),
            Cell::styled(create::about(key).gist, DIM),
        ]);
    }
    ui.table_note(&contents);
    crate::questions::warn_wanted(ui, &after_picks);

    ui.gap();
    ui.done(format!(
        "{} {}",
        crate::ui::paint(crate::ui::PLAIN, "removed"),
        crate::ui::paint(ACCENT, removing.join(", "))
    ));
    ui.gap();
    let mut next = Table::new().indent(4);
    next.row(vec![
        Cell::styled("cargo build", STRONG),
        Cell::styled("the project builds without it", DIM),
    ]);
    ui.table_note(&next);
    Ok(ExitCode::SUCCESS)
}

fn shown(root: &Path, full: &Path) -> String {
    full.strip_prefix(root)
        .unwrap_or(full)
        .display()
        .to_string()
}

/// Delete the directories above `file` that are now empty, up to `root`.
fn prune(root: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) {
            break;
        }
        // `remove_dir` deletes only an empty directory, which is the test.
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

// -- what stays must still stand --------------------------------------------

/// Refuse to take out a module that a module staying is part of or needs.
///
/// One diagnostic for each module leaving, naming every module that holds
/// it, so the help line is one command that works.
fn still_needed(stay: &[&'static str], removing: &[&'static str]) -> Result<(), Vec<Diagnostic>> {
    let mut bad = Vec::new();
    for gone in removing {
        let children: Vec<&str> = stay
            .iter()
            .copied()
            .filter(|key| create::about(key).parent == Some(*gone))
            .collect();
        if !children.is_empty() {
            bad.push(
                Diagnostic::error(
                    *gone,
                    format!("`{gone}` is the parent of {}", ticked(&children)),
                )
                .note("a child is part of its parent and cannot stay without it")
                .help(format!("brocanth remove {} {gone}", children.join(" "))),
            );
        }
        let needers: Vec<(&str, &str)> = stay
            .iter()
            .filter_map(|key| {
                create::about(key)
                    .needs
                    .iter()
                    .find(|(need, _)| need == gone)
                    .map(|(_, why)| (*key, *why))
            })
            .collect();
        if !needers.is_empty() {
            let names: Vec<&str> = needers.iter().map(|(key, _)| *key).collect();
            let mut d = Diagnostic::error(
                *gone,
                format!("{} cannot work without `{gone}`", ticked(&names)),
            );
            for (key, why) in &needers {
                d = d.note(format!("{key}: {why}"));
            }
            bad.push(d.help(format!(
                "brocanth remove {} {gone}, or keep `{gone}`",
                names.join(" ")
            )));
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad)
    }
}

fn ticked(keys: &[&str]) -> String {
    keys.iter()
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

// -- the module files ---------------------------------------------------------

/// What `remove` says about a module file it keeps.
fn returns(path: &str, removing: &[&'static str]) -> String {
    let key = removing
        .iter()
        .find(|key| crate::choices::path_of(key) == path)
        .copied()
        .unwrap_or("<module>");
    format!("your choices return with `brocanth add {key}`")
}

/// A staying module's file that names a `Key` the removal takes away: a
/// setting another module declared only beside the leaving one. The file
/// would not compile, and `remove` never edits it.
fn stranded_keys(
    path: &str,
    current: &str,
    before: &[crate::choices::Shape],
    after: &[crate::choices::Shape],
) -> Vec<Diagnostic> {
    let variants = |shapes: &[crate::choices::Shape]| -> Vec<String> {
        shapes
            .iter()
            .find(|shape| crate::choices::path_of(&shape.key) == path)
            .map(|shape| shape.keys.iter().map(|(v, _)| v.clone()).collect())
            .unwrap_or_default()
    };
    let (was, now) = (variants(before), variants(after));
    crate::choices::keys_named(current)
        .into_iter()
        .filter(|key| was.contains(key) && !now.contains(key))
        .map(|key| {
            let line = current
                .lines()
                .find(|line| line.contains(&format!("Key::{key}")))
                .unwrap_or_default()
                .trim();
            Diagnostic::error(
                "flags",
                format!("{path} names `Key::{key}`, which leaves with this module"),
            )
            .at(path)
            .note(format!("the line: {line}"))
            .help(format!(
                "delete that line from {path}, and run the command again"
            ))
        })
        .collect()
}

// -- the two files both sides write in --------------------------------------

/// The project's `Cargo.toml` without what the leaving modules put in it.
///
/// The inverse of [`add::merge_manifest`](crate::commands::add): a
/// dependency the generator wrote before and does not write now goes, a
/// feature it asked for and no longer asks for goes, and every other byte
/// stays. The receipt above `[dependencies]` gets the line of each module
/// that left.
fn strip_manifest(
    path: &str,
    current: &str,
    was: &str,
    now: &str,
) -> Result<String, Vec<Diagnostic>> {
    let mut doc: DocumentMut = current.parse().map_err(|e| toml_error(path, &e))?;
    let was: DocumentMut = was.parse().map_err(|e| toml_error(path, &e))?;
    let now: DocumentMut = now.parse().map_err(|e| toml_error(path, &e))?;

    let empty = toml_edit::Table::new();
    let was_deps = was
        .get("dependencies")
        .and_then(Item::as_table)
        .unwrap_or(&empty);
    let now_deps = now
        .get("dependencies")
        .and_then(Item::as_table)
        .unwrap_or(&empty);
    if let Some(have) = doc.get_mut("dependencies").and_then(Item::as_table_mut) {
        for (key, item) in was_deps.iter() {
            match now_deps.get(key) {
                None => {
                    have.remove(key);
                }
                Some(stays) => {
                    if let Some(old) = have.get_mut(key) {
                        drop_features(old, item, stays);
                    }
                }
            }
        }
        let prefix = |t: &toml_edit::Table| {
            t.decor()
                .prefix()
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string()
        };
        let current = prefix(have);
        let fresh = prefix(now_deps);
        let restored = if current == prefix(was_deps) {
            fresh
        } else {
            restore_receipt(&current, &fresh)
        };
        have.decor_mut().set_prefix(restored);
    }

    let had_run = was.get("package").and_then(|p| p.get("default-run"));
    let has_run = now.get("package").and_then(|p| p.get("default-run"));
    if had_run.is_some() && has_run.is_none() {
        if let Some(package) = doc.get_mut("package").and_then(Item::as_table_mut) {
            package.remove("default-run");
        }
    }

    let bins = |doc: &DocumentMut| -> Vec<String> {
        doc.get("bin")
            .and_then(Item::as_array_of_tables)
            .map(|all| {
                all.iter()
                    .filter_map(|t| t.get("name").and_then(|n| n.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let stay = bins(&now);
    let gone: Vec<String> = bins(&was)
        .into_iter()
        .filter(|name| !stay.contains(name))
        .collect();
    if !gone.is_empty() {
        if let Some(all) = doc.get_mut("bin").and_then(Item::as_array_of_tables_mut) {
            all.retain(|t| {
                !t.get("name")
                    .and_then(|n| n.as_str())
                    .is_some_and(|n| gone.iter().any(|g| g == n))
            });
            if all.is_empty() {
                doc.remove("bin");
            }
        }
    }
    Ok(doc.to_string())
}

/// Take out of `old` every feature `was` asked for and `now` does not.
fn drop_features(old: &mut Item, was: &Item, now: &Item) {
    let features = |item: &Item| -> Vec<String> {
        item.as_table_like()
            .and_then(|t| t.get("features"))
            .and_then(Item::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|f| f.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let keep = features(now);
    let gone: Vec<String> = features(was)
        .into_iter()
        .filter(|f| !keep.contains(f))
        .collect();
    if gone.is_empty() {
        return;
    }
    let Some(old) = old.as_table_like_mut() else {
        return;
    };
    let Some(have) = old.get_mut("features").and_then(Item::as_array_mut) else {
        return;
    };
    have.retain(|f| !f.as_str().is_some_and(|f| gone.iter().any(|g| g == f)));
    if have.is_empty() && keep.is_empty() {
        old.remove("features");
    }
}

/// The receipt in `current`, its block under `# Not here` replaced with the
/// block in `fresh`, and every other line left where it was.
fn restore_receipt(current: &str, fresh: &str) -> String {
    let block: Vec<&str> = fresh
        .lines()
        .skip_while(|line| !line.starts_with("# Not here"))
        .skip(1)
        .take_while(|line| line.starts_with("#   "))
        .collect();
    let mut out = String::with_capacity(current.len());
    let mut in_block = false;
    let mut placed = false;
    for line in current.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if bare.starts_with("# Not here") {
            in_block = true;
            placed = true;
            out.push_str(line);
            for l in &block {
                out.push_str(l);
                out.push('\n');
            }
            continue;
        }
        if in_block && bare.starts_with("#   ") {
            continue;
        }
        in_block = false;
        out.push_str(line);
    }
    if !placed && !block.is_empty() {
        if let Some(header) = fresh.lines().find(|l| l.starts_with("# Not here")) {
            out.push_str(header);
            out.push('\n');
            for l in &block {
                out.push_str(l);
                out.push('\n');
            }
        }
    }
    out
}

/// The project's `config/<env>.toml` without the sections of the leaving
/// modules.
///
/// A section goes only while it holds the values the generator wrote. A
/// changed value is the developer's -- a database URL, a pool size -- and
/// `remove` does not decide to lose it.
fn strip_config(
    path: &str,
    current: &str,
    was: &str,
    now: &str,
) -> Result<String, Vec<Diagnostic>> {
    let mut doc: DocumentMut = current.parse().map_err(|e| toml_error(path, &e))?;
    let was: DocumentMut = was.parse().map_err(|e| toml_error(path, &e))?;
    let now: DocumentMut = now.parse().map_err(|e| toml_error(path, &e))?;
    let mut bad = Vec::new();
    strip(
        doc.as_table_mut(),
        was.as_table(),
        now.as_table(),
        "",
        path,
        &mut bad,
    );
    if bad.is_empty() {
        Ok(doc.to_string())
    } else {
        Err(bad)
    }
}

/// Take out of `have` every key `was` holds and `now` does not, while it
/// holds what the generator wrote. A table both hold is walked the same
/// way: a child module's keys sit in its parent's section.
fn strip(
    have: &mut dyn toml_edit::TableLike,
    was: &dyn toml_edit::TableLike,
    now: &dyn toml_edit::TableLike,
    at: &str,
    path: &str,
    bad: &mut Vec<Diagnostic>,
) {
    for (key, item) in was.iter() {
        let name = if at.is_empty() {
            key.to_string()
        } else {
            format!("{at}.{key}")
        };
        if let Some(stays) = now.get(key) {
            if let (Some(was), Some(stays), Some(have)) = (
                item.as_table_like(),
                stays.as_table_like(),
                have.get_mut(key).and_then(Item::as_table_like_mut),
            ) {
                strip(have, was, stays, &name, path, bad);
            }
            continue;
        }
        let Some(current) = have.get(key) else {
            continue;
        };
        if canonical(current) == canonical(item) {
            have.remove(key);
            continue;
        }
        let shown = match current {
            Item::Value(v) => format!("{key} = {}", v.to_string().trim()),
            other => {
                let mut one = DocumentMut::new();
                one.insert(key, other.clone());
                one.to_string().trim().to_string()
            }
        };
        bad.push(
            Diagnostic::error("yours", format!("`{name}` in {path} holds a value of yours"))
                .at(path.to_string())
                .note(
                    "it leaves with the module, and `remove` does not delete a value you set",
                )
                .note(shown)
                .help(format!(
                    "copy what you want to keep, delete `{name}` from {path}, and run the command again"
                )),
        );
    }
}

/// Whether two TOML documents hold the same keys and values, whatever their
/// comments, order and spacing.
fn same_document(a: &str, b: &str) -> bool {
    match (a.parse::<DocumentMut>(), b.parse::<DocumentMut>()) {
        (Ok(a), Ok(b)) => canonical(a.as_item()) == canonical(b.as_item()),
        _ => a == b,
    }
}

/// An item as text with no comments and no spacing, keys sorted.
fn canonical(item: &Item) -> String {
    match item {
        Item::None => String::new(),
        Item::Value(v) => canonical_value(v),
        Item::Table(t) => {
            let mut pairs: Vec<String> = t
                .iter()
                .map(|(k, v)| format!("{k}={}", canonical(v)))
                .collect();
            pairs.sort();
            format!("{{{}}}", pairs.join(","))
        }
        Item::ArrayOfTables(all) => {
            let tables: Vec<String> = all
                .iter()
                .map(|t| canonical(&Item::Table(t.clone())))
                .collect();
            format!("[{}]", tables.join(","))
        }
    }
}

fn canonical_value(value: &Value) -> String {
    match value {
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical_value).collect::<Vec<_>>().join(",")
        ),
        Value::InlineTable(t) => {
            let mut pairs: Vec<String> = t
                .iter()
                .map(|(k, v)| format!("{k}={}", canonical_value(v)))
                .collect();
            pairs.sort();
            format!("{{{}}}", pairs.join(","))
        }
        other => {
            let mut v = other.clone();
            v.decor_mut().clear();
            v.to_string()
        }
    }
}

// -- the refusals -----------------------------------------------------------

/// A file of the developer's that would have to change, and was edited.
fn would_change(path: &str, was: &str, now: &str) -> Diagnostic {
    Diagnostic::error(
        "yours",
        format!("{path} would have to change, and it is yours"),
    )
    .at(path)
    .note("you have edited it, and `brocanth remove` never rewrites an edit of yours")
    .note(owned::diff(was, now))
    .help("make this change by hand, keeping your own lines, and run the command again")
}

/// A file that leaves with the module, and holds an edit of the developer's.
fn yours_goes(path: &str, was: &str, current: &str) -> Diagnostic {
    Diagnostic::error(
        "yours",
        format!("{path} leaves with the module, and you have edited it"),
    )
    .at(path)
    .note("`brocanth remove` deletes only what the generator wrote; your edit:")
    .note(owned::diff(was, current))
    .help(format!(
        "move what you want to keep out of {path}, delete the file, and run the command again"
    ))
}

/// `src/commands/mod.rs` leaves with `commands`, and declares commands of
/// the developer's.
fn commands_of_yours(path: &str, yours: &str) -> Diagnostic {
    Diagnostic::error(
        "yours",
        format!("{path} leaves with the module, and it declares commands of yours"),
    )
    .at(path)
    .note(yours.trim().to_string())
    .help(format!(
        "move those commands out of src/commands/, delete their lines below `{}`, and run the command again",
        owned::YOURS
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ui() -> Ui {
        Ui::new(crate::ui::Face::Plain, true, false)
    }

    fn answers(name: &str, picks: &[&'static str]) -> Answers {
        Answers {
            name: name.to_string(),
            title: crate::ui::title_case(name),
            picks: create::all()
                .into_iter()
                .filter(|key| picks.contains(key))
                .collect(),
            local: None,
            git: false,
        }
    }

    /// A project on disk, made the way `brocanth create` makes one.
    fn project(name: &str, picks: &[&'static str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "brocanth-remove-{}-{name}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in create::scaffold(&answers(name, picks)).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().expect("a directory")).expect("mkdir");
            std::fs::write(&full, text).expect("write");
        }
        root
    }

    fn remove(root: &Path, modules: &[&str]) -> Outcome {
        run(
            &ui(),
            Some(root),
            RemoveArgs {
                modules: modules.iter().map(|m| (*m).to_string()).collect(),
                force: false,
            },
        )
    }

    fn add(root: &Path, modules: &[&str]) -> Outcome {
        add::run(
            &ui(),
            Some(root),
            add::AddArgs {
                modules: modules.iter().map(|m| (*m).to_string()).collect(),
                force: false,
                questions: false,
                json: false,
            },
        )
    }

    fn read(root: &Path, path: &str) -> String {
        std::fs::read_to_string(root.join(path)).unwrap_or_default()
    }

    fn said(diagnostics: &[Diagnostic]) -> String {
        diagnostics.iter().map(Diagnostic::render).collect()
    }

    /// Every file under `root`, as `path -> text`.
    fn tree(root: &Path) -> BTreeMap<String, String> {
        fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
            for entry in std::fs::read_dir(dir).expect("a directory").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(root, &path, out);
                } else {
                    let rel = path
                        .strip_prefix(root)
                        .expect("under root")
                        .display()
                        .to_string();
                    out.insert(rel, std::fs::read_to_string(&path).unwrap_or_default());
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
    }

    /// The project `create` would have made with `picks`, compared with
    /// `root`: the same files, each with the same text. The shared files
    /// hold the same keys and values; their order may differ.
    fn assert_as_created(root: &Path, name: &str, picks: &[&'static str], kept: &[&str]) {
        let want: BTreeMap<String, String> = create::scaffold(&answers(name, picks))
            .expect("the modules agree")
            .into_iter()
            .collect();
        let have = tree(root);
        for (path, text) in &have {
            if kept.iter().any(|k| path.starts_with(k)) {
                continue;
            }
            let Some(want) = want.get(path) else {
                panic!("{path} is still there");
            };
            if owned::kind(path) == Kind::Shared {
                assert!(same_document(text, want), "{path}:\n{text}\n---\n{want}");
            } else {
                assert_eq!(text, want, "{path}");
            }
        }
        for path in want.keys() {
            assert!(have.contains_key(path), "{path} is missing");
        }
    }

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

    fn with(picks: &[&'static str], more: &[&'static str]) -> Vec<&'static str> {
        picks.iter().chain(more).copied().collect()
    }

    #[test]
    fn removing_a_module_leaves_what_creating_without_it_would_have_written() {
        for (module, base) in [
            ("cache", DEFAULT),
            ("jobs", DEFAULT),
            ("admin", DEFAULT),
            ("sessions", DEFAULT),
            ("openapi", DEFAULT),
            ("observability-metrics", DEFAULT),
            ("commands", &["config", "logging"][..]),
            ("config", &["logging", "sessions"][..]),
        ] {
            let name = format!("rm{}", module.replace('-', ""));
            let full = if base.contains(&module) {
                base.to_vec()
            } else {
                with(base, &[module])
            };
            let root = project(&name, &full);
            if let Err(e) = remove(&root, &[module]) {
                panic!("{module}: {}", said(&e));
            }
            let without: Vec<&'static str> =
                full.iter().copied().filter(|k| *k != module).collect();
            assert_as_created(&root, &name, &without, &[]);
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn removing_the_model_keeps_the_migrations() {
        let root = project("rmmodel", &["config", "db", "model"]);
        if let Err(e) = remove(&root, &["model"]) {
            panic!("{}", said(&e));
        }
        assert!(root.join("migrations/mod.rs").exists());
        assert!(!root.join("src/bin/migrate.rs").exists());
        assert!(!read(&root, "Cargo.toml").contains("[[bin]]"));
        assert_as_created(&root, "rmmodel", &["config", "db"], &["migrations/"]);

        // And `add` writes around them: the migration list is the `migrate`
        // tool's, and it has moved on since the generator wrote it.
        let list = format!("{}// one migration\n", read(&root, "migrations/mod.rs"));
        std::fs::write(root.join("migrations/mod.rs"), &list).expect("write");
        if let Err(e) = add(&root, &["model"]) {
            panic!("{}", said(&e));
        }
        assert_eq!(read(&root, "migrations/mod.rs"), list);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_edited_file_of_the_module_stops_the_remove_and_nothing_changes() {
        let root = project("rmedited", &with(DEFAULT, &["cache"]));
        let path = root.join("src/plugins/cache/mod.rs");
        let edited = format!("{}// mine\n", read(&root, "src/plugins/cache/mod.rs"));
        std::fs::write(&path, &edited).expect("write");
        let before = tree(&root);

        let refused = said(&remove(&root, &["cache"]).expect_err("the edit is in the way"));
        assert!(refused.contains("src/plugins/cache/mod.rs"), "{refused}");
        assert!(
            refused.contains("+ // mine"),
            "the edit is not shown: {refused}"
        );
        assert!(refused.contains("help:"), "{refused}");
        assert_eq!(tree(&root), before, "a refused remove wrote something");

        // The developer moves their line out and deletes the file, as the
        // help says; the remove then goes through.
        std::fs::remove_file(&path).expect("rm");
        if let Err(e) = remove(&root, &["cache"]) {
            panic!("{}", said(&e));
        }
        assert!(!root.join("src/plugins/cache").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_edited_config_section_of_the_module_is_refused_with_its_values() {
        let root = project("rmconfig", &with(DEFAULT, &["cache"]));
        let path = root.join("config/development.toml");
        let text = read(&root, "config/development.toml");
        assert!(text.contains("[cache]"), "{text}");
        let edited = text.replacen("[cache]\n", "[cache]\n# mine, and only a comment\n", 1);
        std::fs::write(&path, &edited).expect("write");
        // A comment is not a value: the section still goes.
        if let Err(e) = remove(&root, &["cache"]) {
            panic!("{}", said(&e));
        }
        assert!(!read(&root, "config/development.toml").contains("[cache]"));
        let _ = std::fs::remove_dir_all(&root);

        let root = project("rmconfig2", &["config", "db"]);
        let path = root.join("config/development.toml");
        let edited = read(&root, "config/development.toml")
            .replace("postgres://localhost/development", "postgres://db/mine");
        std::fs::write(&path, &edited).expect("write");
        let refused = said(&remove(&root, &["db"]).expect_err("the URL is the developer's"));
        assert!(refused.contains("`db`"), "{refused}");
        assert!(refused.contains("postgres://db/mine"), "{refused}");
        assert!(refused.contains("help:"), "{refused}");
        assert_eq!(read(&root, "config/development.toml"), edited);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_module_another_one_needs_is_refused_with_the_command_that_works() {
        let root = project("rmneeded", &["config", "db", "model"]);
        let refused = said(&remove(&root, &["db"]).expect_err("model needs db"));
        assert!(
            refused.contains("`model` cannot work without `db`"),
            "{refused}"
        );
        assert!(refused.contains("brocanth remove model db"), "{refused}");
        assert!(root.join("src/plugins/db/mod.rs").exists());
        if let Err(e) = remove(&root, &["model", "db"]) {
            panic!("{}", said(&e));
        }
        assert_as_created(&root, "rmneeded", &["config"], &["migrations/"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_parent_with_children_that_stay_is_refused() {
        let root = project("rmparent", DEFAULT);
        let refused = said(&remove(&root, &["observability"]).expect_err("its children stay"));
        assert!(
            refused.contains("`observability` is the parent of"),
            "{refused}"
        );
        assert!(
            refused.contains(
                "brocanth remove observability-logs observability-health observability-metrics observability"
            ),
            "{refused}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_module_that_is_not_there_changes_nothing() {
        let root = project("rmabsent", &["config", "logging"]);
        let before = tree(&root);
        remove(&root, &["cache"]).expect("absent is not an error");
        assert_eq!(tree(&root), before);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_then_add_gives_back_the_project_with_its_renames_and_ejected_screens() {
        let picks = with(DEFAULT, &["sessions", "admin"]);
        let root = project("rmround", &picks);

        // A rename of a flag that leaves with `sessions`, in its module
        // file. `admin.rs` stays as the generator wrote it.
        let file = "src/modules/sessions.rs";
        let choices = read(&root, file).replace(
            "    rename: &[],",
            "    rename: &[(Key::Cookie, Flag::As(\"cookie\"))],",
        );
        assert!(choices.contains("Key::Cookie"), "{choices}");
        std::fs::write(root.join(file), &choices).expect("write");
        // An ejected screen.
        crate::commands::admin::run(
            &ui(),
            Some(&root),
            crate::commands::admin::AdminArgs {
                command: crate::commands::admin::AdminCommand::Eject(
                    crate::commands::admin::EjectArgs {
                        screen: "list".to_string(),
                        model: None,
                    },
                ),
            },
        )
        .expect("eject");
        let screen = format!(
            "{}// mine\n",
            read(&root, "src/plugins/admin/screens/list.rsx")
        );
        std::fs::write(root.join("src/plugins/admin/screens/list.rsx"), &screen).expect("write");
        let whole = tree(&root);

        if let Err(e) = remove(&root, &["sessions", "admin"]) {
            panic!("{}", said(&e));
        }
        // The edited file stays, out of the list; the unedited one goes.
        assert_eq!(read(&root, file), choices);
        assert!(!read(&root, "src/modules/mod.rs").contains("pub mod sessions;"));
        assert!(!root.join("src/modules/admin.rs").exists());
        assert_eq!(read(&root, "src/plugins/admin/screens/list.rsx"), screen);
        assert!(!root.join("src/plugins/admin/mod.rs").exists());
        assert!(!root.join("src/plugins/sessions").exists());

        if let Err(e) = add(&root, &["sessions", "admin"]) {
            panic!("{}", said(&e));
        }
        let back = tree(&root);
        for (path, text) in &whole {
            let now = back
                .get(path)
                .unwrap_or_else(|| panic!("{path} did not come back"));
            if owned::kind(path) == Kind::Shared {
                assert!(same_document(now, text), "{path}:\n{now}\n---\n{text}");
            } else {
                assert_eq!(now, text, "{path}");
            }
        }
        for path in back.keys() {
            assert!(whole.contains_key(path), "{path} is new");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_rename_of_a_key_that_leaves_with_a_child_stops_the_remove() {
        // `observability` declares `live_path` only beside
        // `observability-health`, so the variant goes with the child.
        let root = project("rmkey", DEFAULT);
        let file = "src/modules/observability.rs";
        let text = read(&root, file);
        assert!(text.contains("rename: &[],"), "{text}");
        let shapes = create::module_shapes(&answers("rmkey", DEFAULT)).expect("a project");
        let Some((_, variant)) = crate::choices::key_of(&shapes, "observability.live_path") else {
            panic!("no Key for observability.live_path");
        };
        let edited = text.replace(
            "    rename: &[],",
            &format!("    rename: &[(Key::{variant}, Flag::Hidden)],"),
        );
        std::fs::write(root.join(file), &edited).expect("write");
        let before = tree(&root);
        let refused =
            said(&remove(&root, &["observability-health"]).expect_err("the rename is in the way"));
        assert!(refused.contains(&format!("Key::{variant}")), "{refused}");
        assert!(refused.contains("help:"), "{refused}");
        assert_eq!(tree(&root), before, "a refused remove wrote something");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_old_flags_file_moves_into_the_module_files_on_the_next_add() {
        let root = project("addlegacy", &["config", "sessions"]);
        // The project as the generator wrote it before src/modules/.
        for path in ["src/modules", "src/plugins/shape.rs"] {
            let full = root.join(path);
            let _ = std::fs::remove_dir_all(&full);
            let _ = std::fs::remove_file(&full);
        }
        std::fs::write(
            root.join("src/flags.rs"),
            "pub const WITH_CONFIG: bool = true;\n\
             pub const RENAMES: &[(&str, &str)] = &[\n    (\"sessions.cookie\", \"cookie\"),\n];\n",
        )
        .expect("write");
        if let Err(e) = add(&root, &["cache"]) {
            panic!("{}", said(&e));
        }
        assert!(!root.join("src/flags.rs").exists());
        assert!(read(&root, "src/modules/brocanth.rs").contains("flags_with_config: true,"));
        assert!(read(&root, "src/modules/sessions.rs")
            .contains("rename: &[(Key::Cookie, Flag::As(\"cookie\"))],"));
        assert!(root.join("src/modules/cache.rs").exists());
        assert!(root.join("src/plugins/shape.rs").exists());
        let _ = std::fs::remove_dir_all(&root);

        // A line the generator did not write: nothing moves.
        let root = project("addlegacy2", &["config", "sessions"]);
        let _ = std::fs::remove_dir_all(root.join("src/modules"));
        std::fs::write(
            root.join("src/flags.rs"),
            "pub const WITH_CONFIG: bool = false;\npub const MINE: u8 = 1;\n",
        )
        .expect("write");
        let before = tree(&root);
        let refused = said(&add(&root, &["cache"]).expect_err("a line of theirs"));
        assert!(refused.contains("pub const MINE: u8 = 1;"), "{refused}");
        assert!(
            refused
                .contains("move these lines to src/modules/<module>.rs, then delete src/flags.rs"),
            "{refused}"
        );
        assert_eq!(tree(&root), before, "a refused add wrote something");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_admin_file_from_before_admin_panels_is_rewritten_or_shown() {
        use brocanth_admin::scaffold::{LEGACY_ANALYTICS_PANEL, PANELS};
        let file = "src/plugins/admin/mod.rs";
        let picks = ["config", "db", "analytics", "admin"];
        let root = project("addadmin", &picks);
        let now = read(&root, file);
        assert!(now.contains(PANELS), "{now}");
        // As the generator wrote it before: the tab, with no guard.
        std::fs::write(root.join(file), now.replace(PANELS, LEGACY_ANALYTICS_PANEL))
            .expect("write");
        if let Err(e) = add(&root, &["cache"]) {
            panic!("{}", said(&e));
        }
        assert_eq!(read(&root, file), now);
        let _ = std::fs::remove_dir_all(&root);

        // Edited as well: the add goes on, and the file stays as it is.
        let root = project("addadmin2", &picks);
        let edited = format!("{}// mine\n", now.replace(PANELS, LEGACY_ANALYTICS_PANEL));
        std::fs::write(root.join(file), &edited).expect("write");
        if let Err(e) = add(&root, &["cache"]) {
            panic!("{}", said(&e));
        }
        assert_eq!(read(&root, file), edited);
        let _ = std::fs::remove_dir_all(&root);
    }
}
