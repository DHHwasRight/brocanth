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
//! `let config = ejected::apply(config);` before `let admin` in `mount`.
//!
//! `ejected.rs` is rebuilt from the `.rsx` files on disk, so the files are
//! the record of what was ejected. Delete one and run any eject again to
//! drop it from the wiring.
//!
//! # What it refuses
//!
//! An existing copy is never overwritten: it is yours, and the shipped
//! screen is one `rm` away. A screen that shows no one model (`layout`,
//! `index`, `error`, `brand`, `userlinks`) cannot take `--model`. A model
//! key that is not a Rust module name cannot name the directory `rsx`
//! turns into a module.
//!
//! # `brocanth admin diff`
//!
//! After an upgrade, an ejected copy does not change. `admin diff` prints a
//! unified diff from the shipped screen to the copy, for every copy or for
//! one. The shipped screen is the template of the `brocanth-admin` the
//! project has installed: `cargo metadata --offline` names its directory.
//! When cargo cannot say, the command compares with the screens this CLI
//! carries and says so. `--shipped <dir>` names a `brocanth-admin`
//! directory instead.
//!
//! The copy's header line names the version it came from. When that is the
//! shipped version, the diff is your edits. When it is not, the diff holds
//! your edits and the release's changes together, and the command says so.

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
    /// Compare ejected screens with the shipped ones of the installed version
    ///
    /// Prints a unified diff from the shipped screen to each copy. Without
    /// a screen, every copy in the project.
    Diff(DiffArgs),
}

#[derive(Args)]
pub struct DiffArgs {
    /// The screen to compare. Without one, every ejected copy
    #[arg(value_parser = clap::builder::PossibleValuesParser::new(eject::names()))]
    pub screen: Option<String>,
    /// Compare the copy for this model only
    #[arg(long, value_name = "KEY", requires = "screen")]
    pub model: Option<String>,
    /// A brocanth-admin directory to compare with, not the installed one
    #[arg(long, value_name = "DIR")]
    pub shipped: Option<PathBuf>,
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
pub(crate) const PLUGIN: &str = "src/plugins/admin/mod.rs";
pub(crate) const SCREENS: &str = "src/plugins/admin/screens";
const WIRING: &str = "src/plugins/admin/ejected.rs";

/// The line in the generated `mount` the install goes in front of: the
/// admin built over an empty source, or the `OverOrm` that builds it over
/// the ORM.
const ANCHOR: &str = "let admin = Arc::new(";
const INSTALL: &str = "let config = ejected::apply(config);";

pub fn run(ui: &Ui, dir: Option<&Path>, args: AdminArgs) -> Outcome {
    match args.command {
        AdminCommand::Eject(args) => eject(ui, dir, args),
        AdminCommand::Diff(args) => diff(ui, dir, args),
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

/// Where the shipped screens come from, and which version they are.
struct Shipped {
    /// A `brocanth-admin` directory, or `None` for the screens this CLI
    /// carries.
    dir: Option<PathBuf>,
    /// The version, as cargo reports it.
    version: String,
    /// How the directory was found, for the reader.
    origin: &'static str,
}

impl Shipped {
    /// The screens this CLI carries.
    fn embedded() -> Self {
        Shipped {
            dir: None,
            version: eject::VERSION.to_string(),
            origin: "carried by this CLI",
        }
    }

    /// The shipped template of `screen`, rewritten as an eject writes it,
    /// without the header. `None` when this version has no such screen.
    fn template(&self, screen: &eject::Screen) -> Option<String> {
        match &self.dir {
            None => Some(eject::rewrite(screen.shipped())),
            Some(dir) => {
                let path = dir
                    .join("src/templates")
                    .join(format!("{}.rsx", screen.name));
                std::fs::read_to_string(path)
                    .ok()
                    .map(|t| eject::rewrite(&t))
            }
        }
    }
}

/// The version of the `brocanth-admin` package at `dir`, when cargo can say.
fn version_at(dir: &Path) -> Option<String> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(dir.join("Cargo.toml"))
        .no_deps()
        .other_options(vec!["--offline".to_string()])
        .exec()
        .ok()?;
    metadata
        .packages
        .iter()
        .find(|p| p.name.as_str() == "brocanth-admin")
        .map(|p| p.version.to_string())
}

/// The `brocanth-admin` this project resolves, from `cargo metadata`.
///
/// `--offline`: the command reads what is already on disk and never
/// fetches. A project that was never built has nothing on disk yet.
fn installed(root: &Path) -> Result<Shipped, String> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(root.join("Cargo.toml"))
        .other_options(vec!["--offline".to_string()])
        .exec()
        .map_err(|e| e.to_string())?;
    let package = metadata
        .packages
        .iter()
        .find(|p| p.name.as_str() == "brocanth-admin")
        .ok_or_else(|| "the project does not depend on brocanth-admin".to_string())?;
    let dir = Path::new(package.manifest_path.as_str())
        .parent()
        .ok_or_else(|| "brocanth-admin has no directory".to_string())?;
    Ok(Shipped {
        dir: Some(dir.to_path_buf()),
        version: package.version.to_string(),
        origin: "installed",
    })
}

fn diff(ui: &Ui, dir: Option<&Path>, args: DiffArgs) -> Outcome {
    let project = Project::find(dir)?;
    let root = &project.root;
    let screens = root.join(SCREENS);
    let mut copies = on_disk(&screens);
    if let Some(name) = &args.screen {
        copies.retain(|e| e.screen == name.as_str() && e.model == args.model);
        if copies.is_empty() {
            let wanted = Ejected {
                screen: eject::screen(name).map_or("?", |s| s.name),
                model: args.model.clone(),
            };
            let command = match &args.model {
                None => format!("brocanth admin eject {name}"),
                Some(model) => format!("brocanth admin eject {name} --model {model}"),
            };
            return Err(vec![Diagnostic::error(
                "admin/not-ejected",
                format!(
                    "{} is not ejected, so there is no copy to compare",
                    describe(&wanted)
                ),
            )
            .at(format!("{SCREENS}/{}", wanted.file()))
            .help(format!("`{command}` writes the copy"))]);
        }
    }
    if copies.is_empty() {
        ui.note("no admin screen is ejected: the project renders the shipped screens");
        return Ok(ExitCode::SUCCESS);
    }

    let shipped = match &args.shipped {
        Some(dir) => {
            if !dir.join("src/templates").is_dir() {
                return Err(vec![Diagnostic::error(
                    "admin/shipped",
                    format!("{} holds no admin templates", dir.display()),
                )
                .help(
                    "name the brocanth-admin directory, the one with src/templates/",
                )]);
            }
            Shipped {
                version: version_at(dir).unwrap_or_else(|| "of unknown version".to_string()),
                dir: Some(dir.clone()),
                origin: "named by --shipped",
            }
        }
        None => installed(root).unwrap_or_else(|reason| {
            ui.diagnostic(
                &Diagnostic::warning(
                    "admin/no-installed",
                    "cannot find the brocanth-admin this project installed",
                )
                .note(reason)
                .note(format!(
                    "compared with the screens this CLI carries, version {}",
                    eject::VERSION
                ))
                .help("build the project once, or pass --shipped <dir>"),
            );
            Shipped::embedded()
        }),
    };

    let mut differ = 0;
    for copy in &copies {
        let Some(screen) = eject::screen(copy.screen) else {
            continue;
        };
        let path = format!("{SCREENS}/{}", copy.file());
        let text = std::fs::read_to_string(root.join(&path)).map_err(|e| {
            vec![Diagnostic::error("admin/read", format!("cannot read the copy: {e}")).at(&path)]
        })?;
        let Some(theirs) = shipped.template(screen) else {
            ui.warn(format!(
                "{}: brocanth-admin {} has no `{}.rsx` to compare with",
                describe(copy),
                shipped.version,
                screen.name
            ));
            continue;
        };
        let from = eject::ejected_version(&text);
        let ours = eject::without_header(&text);
        let report = unified(
            &theirs,
            ours,
            &format!("shipped/{}.rsx", screen.name),
            &path,
        );
        ui.note(format!(
            "{} {}",
            paint(ACCENT, describe(copy)),
            paint(DIM, &path)
        ));
        ui.note(format!(
            "  copied from brocanth-admin {}; compared with {} ({})",
            from.unwrap_or("of unknown version (no header line)"),
            shipped.version,
            shipped.origin
        ));
        if from.is_some_and(|v| v != shipped.version) {
            ui.note("  the versions differ: the diff holds your edits and the release's changes");
        }
        match report {
            None => ui.note("  the same as the shipped screen"),
            Some(text) => {
                differ += 1;
                ui.result(text.trim_end());
            }
        }
    }
    let summary = match copies.len() {
        1 => format!("{differ} of 1 ejected copy differs from the shipped screen"),
        n => format!("{differ} of {n} ejected copies differ from the shipped screens"),
    };
    ui.done(summary);
    Ok(ExitCode::SUCCESS)
}

/// One line of an edit script.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Same,
    Gone,
    Added,
}

/// How many unchanged lines a hunk shows around a change, as `diff -u`.
const CONTEXT: usize = 3;

/// A unified diff from `old` to `new`, or `None` when they are the same.
///
/// A longest common subsequence over lines. Templates are a few hundred
/// lines, so the quadratic table is small, and a real diff reads better
/// than a common head and tail when the edits are far apart.
fn unified(old: &str, new: &str, old_name: &str, new_name: &str) -> Option<String> {
    if old == new {
        return None;
    }
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // lcs[i][j]: the common length of a[i..] and b[j..].
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = match a[i] == b[j] {
                true => lcs[i + 1][j + 1] + 1,
                false => lcs[i + 1][j].max(lcs[i][j + 1]),
            };
        }
    }
    // The script: each entry is an op and the line indexes it is at.
    let mut script: Vec<(Op, usize, usize)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            script.push((Op::Same, i, j));
            i += 1;
            j += 1;
        } else if i < a.len() && (j == b.len() || lcs[i + 1][j] >= lcs[i][j + 1]) {
            // A removed line before an added one, as `diff -u` prints them.
            script.push((Op::Gone, i, j));
            i += 1;
        } else {
            script.push((Op::Added, i, j));
            j += 1;
        }
    }
    if script.iter().all(|(op, _, _)| *op == Op::Same) {
        // Only the final newline differs: `lines` hides it.
        return Some(format!(
            "--- {old_name}\n+++ {new_name}\n(the files differ only in the final newline)\n"
        ));
    }

    let mut out = format!("--- {old_name}\n+++ {new_name}\n");
    let changes: Vec<usize> = (0..script.len())
        .filter(|&k| script[k].0 != Op::Same)
        .collect();
    let mut k = 0;
    while k < changes.len() {
        // A hunk runs from CONTEXT before one change to CONTEXT after the
        // last change that is close enough to join it.
        let start = changes[k].saturating_sub(CONTEXT);
        let mut last = changes[k];
        while k + 1 < changes.len() && changes[k + 1] <= last + 2 * CONTEXT + 1 {
            k += 1;
            last = changes[k];
        }
        let end = (last + CONTEXT + 1).min(script.len());
        let slice = &script[start..end];
        let (old_start, new_start) = (slice[0].1, slice[0].2);
        let old_len = slice.iter().filter(|(op, _, _)| *op != Op::Added).count();
        let new_len = slice.iter().filter(|(op, _, _)| *op != Op::Gone).count();
        // `diff -u` counts from one, and an empty side starts at the line
        // before it.
        let at = |start: usize, len: usize| match len {
            0 => format!("{start},0"),
            1 => format!("{}", start + 1),
            n => format!("{},{n}", start + 1),
        };
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            at(old_start, old_len),
            at(new_start, new_len)
        ));
        for (op, i, j) in slice {
            match op {
                Op::Same => out.push_str(&format!(" {}\n", a[*i])),
                Op::Gone => out.push_str(&format!("-{}\n", a[*i])),
                Op::Added => out.push_str(&format!("+{}\n", b[*j])),
            }
        }
        k += 1;
    }
    Some(out)
}

/// The plugin as `eject` leaves it: `text` with the two wiring edits made.
///
/// `brocanth remove admin` reads a plugin equal to this as unedited: the
/// eject made the edits, not the developer.
pub(crate) fn wired(text: &str) -> String {
    wire(text).0
}

/// Install again the screens a project ejected before, into an admin
/// plugin `brocanth add` has just written. Returns the paths it changed.
///
/// `brocanth remove admin` keeps the copies, because they are the
/// developer's work, and deletes the plugin that installed them. So the
/// next `add admin` finds copies and a plugin that does not know them, and
/// makes the edits an eject makes, from the copies on disk.
pub(crate) fn rewire(root: &Path) -> Result<Vec<String>, Vec<Diagnostic>> {
    let all = on_disk(&root.join(SCREENS));
    if all.is_empty() {
        return Ok(Vec::new());
    }
    let mut changed = Vec::new();
    let wiring = eject::wiring(SCREENS, &all);
    if std::fs::read_to_string(root.join(WIRING)).ok().as_deref() != Some(wiring.as_str()) {
        write(&root.join(WIRING), &wiring)?;
        changed.push(WIRING.to_string());
    }
    if add_rsx(&root.join("Cargo.toml"))? {
        changed.push("Cargo.toml".to_string());
    }
    let plugin = root.join(PLUGIN);
    if let Ok(text) = std::fs::read_to_string(&plugin) {
        let (wired, _) = wire(&text);
        if wired != text {
            write(&plugin, &wired)?;
            changed.push(PLUGIN.to_string());
        }
    }
    Ok(changed)
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

    #[test]
    fn the_project_command_passes_arguments_this_command_accepts() {
        // `src/commands/admin.rs` in a project with `commands` hands these
        // exact vectors to `brocanth`. A renamed flag here breaks it there.
        use clap::{Args, FromArgMatches};
        let parse = |argv: &[&str]| {
            let cli = clap::Command::new("brocanth")
                .subcommand(AdminArgs::augment_args(clap::Command::new("admin")));
            let matches = cli.try_get_matches_from(argv).expect("brocanth accepts it");
            let (_, admin) = matches.subcommand().expect("the admin subcommand");
            match AdminArgs::from_arg_matches(admin)
                .expect("admin arguments")
                .command
            {
                AdminCommand::Eject(eject) => (eject.screen, eject.model),
                AdminCommand::Diff(diff) => (diff.screen.unwrap_or_default(), diff.model),
            }
        };
        assert_eq!(
            parse(&["brocanth", "admin", "eject", "list", "--model", "post"]),
            ("list".to_string(), Some("post".to_string()))
        );
        assert_eq!(
            parse(&["brocanth", "admin", "eject", "layout"]),
            ("layout".to_string(), None)
        );
        assert_eq!(
            parse(&["brocanth", "admin", "diff", "list", "--model", "post"]),
            ("list".to_string(), Some("post".to_string()))
        );
        assert_eq!(parse(&["brocanth", "admin", "diff"]), (String::new(), None));
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

    fn diff_in(root: &Path, screen: Option<&str>, model: Option<&str>) -> Outcome {
        run(
            &ui(),
            Some(root),
            AdminArgs {
                command: AdminCommand::Diff(DiffArgs {
                    screen: screen.map(str::to_string),
                    model: model.map(str::to_string),
                    shipped: Some(this_admin()),
                }),
            },
        )
    }

    /// The brocanth-admin beside this crate: the version the tests compare
    /// with, so no test reads `cargo metadata` of a generated project.
    fn this_admin() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .join("brocanth-admin")
    }

    #[test]
    fn a_fresh_copy_is_the_shipped_screen_and_an_edit_shows_in_the_diff() {
        let root = project("diffy", &["admin"]);
        eject_in(&root, "list", Some("invoice")).expect("ejects");
        let path = root.join(SCREENS).join("models/invoice/list.rsx");
        let copy = std::fs::read_to_string(&path).unwrap();

        let screen = eject::screen("list").unwrap();
        let shipped = Shipped {
            dir: Some(this_admin()),
            version: eject::VERSION.to_string(),
            origin: "test",
        };
        let theirs = shipped.template(screen).expect("a shipped list");
        assert_eq!(eject::ejected_version(&copy), Some(eject::VERSION));
        assert_eq!(
            unified(&theirs, eject::without_header(&copy), "a", "b"),
            None,
            "a fresh copy is the shipped screen"
        );

        let edited = copy.replacen(
            "pub fn List(ctx: &ListCtx) -> Element {",
            "pub fn List(ctx: &ListCtx) -> Element {\n    <p>Invoices reach customers.</p>",
            1,
        );
        std::fs::write(&path, &edited).unwrap();
        let report = unified(&theirs, eject::without_header(&edited), "a", "b")
            .expect("the edit is a difference");
        assert!(
            report.contains("\n+    <p>Invoices reach customers.</p>\n"),
            "{report}"
        );
        assert_eq!(
            report
                .lines()
                .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
                .count(),
            1,
            "{report}"
        );
        assert!(!report
            .lines()
            .any(|l| l.starts_with('-') && !l.starts_with("---")));

        // The command runs over the project and the one copy.
        diff_in(&root, None, None).expect("every copy");
        diff_in(&root, Some("list"), Some("invoice")).expect("one copy");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_screen_that_is_not_ejected_is_refused_with_the_eject_command() {
        let root = project("diffnone", &["admin"]);
        // Nothing ejected: nothing to say, and not an error.
        diff_in(&root, None, None).expect("no copies");
        let d = refusal(diff_in(&root, Some("detail"), None));
        assert_eq!(d.code.as_deref(), Some("admin/not-ejected"));
        assert!(d
            .notes
            .iter()
            .any(|(kind, text)| *kind == "help" && text.contains("brocanth admin eject detail")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_shipped_directory_without_templates_is_refused() {
        let root = project("diffdir", &["admin"]);
        eject_in(&root, "brand", None).expect("the brand block ejects");
        let d = refusal(run(
            &ui(),
            Some(&root),
            AdminArgs {
                command: AdminCommand::Diff(DiffArgs {
                    screen: None,
                    model: None,
                    shipped: Some(root.clone()),
                }),
            },
        ));
        assert_eq!(d.code.as_deref(), Some("admin/shipped"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_diff_is_unified_with_three_lines_of_context() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\nl\nm\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\nk\nL\nm\n";
        let report = unified(old, new, "shipped", "yours").unwrap();
        // Two changes eight lines apart: two hunks, not one.
        assert_eq!(
            report,
            "--- shipped\n+++ yours\n\
             @@ -1,5 +1,5 @@\n a\n-b\n+B\n c\n d\n e\n\
             @@ -9,5 +9,5 @@\n i\n j\n k\n-l\n+L\n m\n"
        );
        // A line added at the end, and a file that only lost its newline.
        assert_eq!(
            unified("a\n", "a\nb\n", "x", "y").unwrap(),
            "--- x\n+++ y\n@@ -1 +1,2 @@\n a\n+b\n"
        );
        assert!(unified("a\n", "a", "x", "y")
            .unwrap()
            .contains("final newline"));
        assert_eq!(unified("same\n", "same\n", "x", "y"), None);
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
            // `commands` too, so the project's own `admin eject`, which
            // hands its arguments to this CLI, compiles and runs its tests.
            picks: vec![
                "config",
                "commands",
                "admin",
                "middleware",
                "middleware-csrf",
            ],
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
