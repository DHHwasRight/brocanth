//! `brocanth create` -- scaffold an application.
//!
//! Interactive when a person is watching and an answer is missing. **Every
//! answer is also a flag**, so the prompt is a convenience and never the only
//! route: the same application can be produced by a script, a Dockerfile or a
//! CI job with no terminal attached. With no terminal and no name, the command
//! fails naming the argument rather than blocking on a read that will never
//! return.
//!
//! # This command is where §8 becomes visible
//!
//! The governing constraint is that nothing ships unless it is asked for, and
//! the first page a user reads is the one this command writes. So three things
//! are true of the output, and they are the whole design of this file:
//!
//! 1. **Every plugin is named in `main`.** Not a feature flag, not a
//!    convention, not a builder that "sets up the usual things" -- a line a
//!    reader can delete. The generated `main` is a numbered list of what the
//!    binary contains, and the middleware mounts sit beside it, because the
//!    pattern is a decision and the decision is the application's.
//! 2. **The generated `Cargo.toml` holds exactly what was chosen**, and says in
//!    a comment what it does *not* hold and why. A scaffold that quietly pulled
//!    in an ORM nobody asked for would contradict the framework's central claim
//!    before the user had read a line of it.
//! 3. **Choosing the config plugin changes the source, not a flag.** With it,
//!    a plugin's settings struct derives `Deserialize` and `main` reads a TOML
//!    file. Without it, the struct derives nothing, `main` writes a literal,
//!    and no parser is linked. "A feature that is off must mean the code is
//!    absent" is a sentence in §8; here it is two scaffolds.

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use brocanth_scaffold::{Dep, Scaffold as Built, Slot, Source};
use clap::Args;

use crate::tui::{Choice, Draft};

use crate::commands::Outcome;
use crate::ui::{Cell, Diagnostic, Table, Ui, ACCENT, DIM, STRONG};

/// Where the generated `Cargo.toml` points for the framework itself.
const REPOSITORY: &str = "https://github.com/DHHwasRight/brocanth.git";

#[derive(Args)]
pub struct CreateArgs {
    /// Directory to create, and the cargo package name
    #[arg(value_name = "NAME")]
    pub name: Option<String>,
    /// Human name of the application, used in `[app] name` and on the pages
    #[arg(long, value_name = "TEXT")]
    pub title: Option<String>,
    /// Plugins to include; repeat the flag or separate with commas
    ///
    /// The list is whatever modules this CLI was compiled with, so it grows
    /// by adding a crate rather than by editing an enum here.
    #[arg(
        long = "with",
        value_name = "PLUGIN",
        value_delimiter = ',',
        conflicts_with = "core_only",
        value_parser = offered()
    )]
    pub with: Vec<String>,
    /// The core alone: a router, a context, and nothing else
    #[arg(long)]
    pub core_only: bool,
    /// Depend on a local brocanth instead of the published one
    ///
    /// DIR is the core crate's own directory -- `crates/brocanth` inside a
    /// checkout, not the checkout itself, which is a virtual manifest cargo
    /// will refuse. Sibling crates are found next to it, so `brocanth-config`
    /// is read from `crates/brocanth-config`.
    #[arg(long, value_name = "DIR")]
    pub brocanth_path: Option<String>,
    /// Initialise a git repository
    #[arg(long)]
    pub git: bool,
    /// Do not initialise a git repository
    #[arg(long, conflicts_with = "git")]
    pub no_git: bool,
    /// Write into a directory that already exists
    #[arg(long)]
    pub force: bool,
}

/// Every module this CLI has, as clap's possible values.
///
/// Built from the registry rather than written out, so a CLI compiled with
/// somebody's module offers it in `--help` with its own one-line gist and
/// without a line changing here.
pub fn offered() -> clap::builder::PossibleValuesParser {
    clap::builder::PossibleValuesParser::new(
        crate::modules::ALL
            .iter()
            .map(|module| {
                let m = module.manifest();
                // A listed module this build cannot make says so here as
                // well as on the chooser's row. `--help` is the other way
                // in, and a word it offers without a caveat is a word
                // somebody types.
                let help = match m.blocked {
                    Some(_) => format!("{} -- not available yet", m.gist),
                    None => m.gist.to_string(),
                };
                clap::builder::PossibleValue::new(m.key).help(help)
            })
            .collect::<Vec<_>>(),
    )
}

/// What a non-interactive run gets when no flag chose.
///
/// Stated rather than guessed at: the command prints the flag that overrides
/// it. Three keys rather than three modules, because this list outlives any
/// particular registry -- a CLI compiled without one of them says so.
const DEFAULT: [&str; 3] = ["logging", "config", "commands"];

/// Everything the CLI was compiled with, in listing order.
///
/// Including the modules nobody can choose yet. They are on the chooser's
/// screen with the reason on the row, and `--with` has to recognise the
/// word to be able to print that reason, so the full list is what the two
/// front doors read. Everything that then *writes* reads [`available`].
pub fn all() -> Vec<&'static str> {
    crate::modules::keys()
}

/// The modules a project can actually be made of.
pub fn available() -> Vec<&'static str> {
    all()
        .into_iter()
        .filter(|key| about(key).blocked.is_none())
        .collect()
}

/// The manifest of a module this CLI has. Panics for a key it does not,
/// which is a programming error here rather than anything a user can type:
/// every key reaching this came from [`all`] or from a validated flag.
pub fn about(key: &str) -> brocanth_scaffold::Manifest {
    crate::modules::find(key)
        .unwrap_or_else(|| unreachable!("`{key}` is not a module this CLI has"))
        .manifest()
}

/// The answers, however they arrived.
///
/// `brocanth create` fills it from flags or from the chooser;
/// [`crate::commands::add`] fills it from `brocanth.toml` and the module
/// the user named. One struct, because there is one generator.
pub struct Answers {
    /// The cargo package name, and the directory.
    pub name: String,
    /// The human name.
    pub title: String,
    pub picks: Vec<&'static str>,
    /// A local checkout to depend on, instead of the repository.
    pub local: Option<String>,
    pub git: bool,
}

impl Answers {
    pub fn has(&self, key: &str) -> bool {
        debug_assert!(
            crate::modules::find(key).is_some(),
            "asked about `{key}`, which no module declares"
        );
        self.picks.contains(&key)
    }

    /// The command line that would have produced these answers with nothing
    /// to ask.
    ///
    /// Written out in full rather than minimally: a line that relies on a
    /// default is a line that changes meaning when the default does, and the
    /// point of printing it is that somebody will paste it into a script.
    fn as_flags(&self) -> String {
        let mut out = format!("brocanth create {}", shell_quote(&self.name));
        out.push_str(&format!(" --title {}", shell_quote(&self.title)));
        if self.picks.is_empty() {
            out.push_str(" --core-only");
        } else {
            let names: Vec<&str> = self.picks.clone();
            out.push_str(&format!(" --with {}", names.join(",")));
        }
        out.push_str(if self.git { " --git" } else { " --no-git" });
        if let Some(path) = &self.local {
            out.push_str(&format!(" --brocanth-path {}", shell_quote(path)));
        }
        out
    }

    /// `my-blog` -> `my_blog`. The crate identifier, and the prefix of every
    /// environment variable the generated binary reads.
    fn ident(&self) -> String {
        self.name.replace('-', "_")
    }
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: CreateArgs) -> Outcome {
    let answers = ask(ui, &args)?;
    // `-C` is where to create it. Everywhere else it means "the package root",
    // and here there is no package yet, so it is the directory to put one in.
    let root = dir.unwrap_or(Path::new(".")).join(&answers.name);
    if root.exists() && !args.force {
        return Err(vec![Diagnostic::error(
            "exists",
            format!("{} already exists", root.display()),
        )
        .help("pass --force to write into it anyway, or choose another name")]);
    }

    ui.heading("create", &answers.name);

    let files = scaffold(&answers)?;
    let mut table = Table::new();
    for (path, contents) in &files {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io_error(parent, &e))?;
        }
        std::fs::write(&full, contents).map_err(|e| io_error(&full, &e))?;
        table.row(vec![
            Cell::styled(format!("{}/{path}", answers.name), ACCENT),
            Cell::styled(format!("{} bytes", contents.len()), DIM).right(),
        ]);
    }
    ui.table_note(&table);

    if answers.git {
        let ok = Command::new("git")
            .arg("init")
            .arg("--quiet")
            .current_dir(&root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            ui.note(format!("  {}", crate::ui::paint(DIM, "git repository")));
        } else {
            ui.warn("git init did not run; the files are written either way");
        }
    }

    // The §8 receipt: what the binary contains, and what it therefore does
    // not. The same list, read from both ends.
    ui.gap();
    let mut contents = Table::new().indent(4);
    contents.row(vec![
        Cell::styled("core", STRONG),
        Cell::styled("a router handing a request and a context to a handler", DIM),
    ]);
    for pick in &answers.picks {
        contents.row(vec![
            Cell::styled(*pick, ACCENT),
            Cell::styled(about(pick).gist, DIM),
        ]);
    }
    for pick in available().iter().filter(|p| !answers.has(p)) {
        contents.row(vec![
            Cell::styled(*pick, DIM),
            Cell::styled(
                "not asked for: absent from the source, the binary and the lock file",
                DIM,
            ),
        ]);
    }
    ui.table_note(&contents);

    ui.gap();
    ui.done(format!(
        "{} {}",
        crate::ui::paint(crate::ui::PLAIN, "created"),
        crate::ui::paint(ACCENT, root.display())
    ));
    ui.gap();
    let mut next = Table::new().indent(4);
    next.row(vec![
        Cell::styled(format!("cd {}", answers.name), STRONG),
        Cell::new(""),
    ]);
    if answers.has("commands") {
        next.row(vec![
            Cell::styled("brocanth serve", STRONG),
            Cell::styled("start the server on 127.0.0.1:3000", DIM),
        ]);
        next.row(vec![
            Cell::styled("brocanth --help", STRONG),
            Cell::styled("every command this project defines", DIM),
        ]);
    } else {
        next.row(vec![
            Cell::styled("cargo run", STRONG),
            Cell::styled("start the server on 127.0.0.1:3000", DIM),
        ]);
    }
    ui.table_note(&next);
    Ok(ExitCode::SUCCESS)
}

// -- the answers ------------------------------------------------------------

fn ask(ui: &Ui, args: &CreateArgs) -> Result<Answers, Vec<Diagnostic>> {
    let local = local_checkout(args)?;
    unavailable(&picked(args))?;
    orphans(&picked(args))?;
    unmet(&picked(args))?;

    // The chooser is reached on one condition, and the condition is the single
    // answer that has no default. Everything else falls back to something this
    // command can state out loud, so supplying a name is what distinguishes
    // being *driven* from being *asked*.
    if args.name.is_none() {
        if !ui.interactive() {
            // The whole reason every answer is also a flag. A chooser here
            // would block a CI job until it timed out.
            return Err(vec![Diagnostic::error(
                "missing-argument",
                "no name, and no terminal to ask for one",
            )
            .help("pass it: `brocanth create <NAME>`")
            .note(
                "every answer the chooser collects has a flag: --title, --with, \
                 --core-only, --git/--no-git",
            )]);
        }
        return chooser(ui, args, local);
    }

    let name = args.name.clone().expect("the None arm returned above");
    check_name(&name)?;
    let title = args
        .title
        .clone()
        .unwrap_or_else(|| crate::ui::title_case(&name));

    Ok(Answers {
        name,
        title,
        picks: picked(args),
        local,
        // Neither flag means no repository. Stated, not guessed: the receipt
        // this command prints names `--git` as the way to change it.
        git: args.git,
    })
}

/// What the flags alone say goes in the binary.
///
/// Shared by both routes, so the chooser opens with its tick-boxes already
/// matching what `--with` or `--core-only` asked for.
fn picked(args: &CreateArgs) -> Vec<&'static str> {
    if args.core_only {
        Vec::new()
    } else if !args.with.is_empty() {
        // Back to the registry's own `&'static str`, because the rest of
        // this file compares keys by value and a `String` from clap is the
        // same word with a different lifetime. clap already refused
        // anything that is not a module, so the lookup cannot miss.
        // In the registry's order, not the order they were typed and not
        // alphabetical. The chooser lists modules in that order and writes
        // them into `main` in that order; a flag that produced a different
        // `main` for the same set of words would be a difference nobody
        // asked for. clap already refused anything that is not a module, so
        // the lookup cannot miss.
        all()
            .into_iter()
            .filter(|key| args.with.iter().any(|named| named == key))
            .collect()
    } else {
        DEFAULT.to_vec()
    }
}

/// A choice this CLI lists and cannot make.
///
/// Checked before [`orphans`], because a blocked child named without its
/// parent has two things wrong with it and only one of them matters: the
/// parent would not help.
pub fn unavailable(picks: &[&str]) -> Result<(), Vec<Diagnostic>> {
    let bad: Vec<Diagnostic> = picks
        .iter()
        .filter_map(|pick| {
            let why = about(pick).blocked?;
            Some(
                Diagnostic::error(*pick, format!("`{pick}` is not available yet"))
                    .note(why)
                    .help("brocanth create --help lists what this build can make"),
            )
        })
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad)
    }
}

/// A choice that is part of another choice, named without it.
///
/// The chooser cannot produce this -- it hides a child whose parent is off --
/// so it is only reachable from the flags, where it is nearly always a
/// forgotten word rather than an intention.
pub fn orphans(picks: &[&str]) -> Result<(), Vec<Diagnostic>> {
    let bad: Vec<Diagnostic> = picks
        .iter()
        .filter_map(|pick| {
            let parent = about(pick).parent?;
            if picks.contains(&parent) {
                return None;
            }
            let with: Vec<&str> = picks
                .iter()
                .copied()
                .chain(std::iter::once(parent))
                .collect();
            let mut with = with;
            with.sort_unstable();
            Some(
                Diagnostic::error(*pick, format!("`{pick}` is part of `{parent}`"))
                    .note(about(pick).because)
                    .help(format!("--with {}", with.join(","))),
            )
        })
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad)
    }
}

/// Picks that cannot be written without a module nobody asked for.
///
/// The flat command line's half of [`Manifest::needs`]. The chooser's half
/// is a tick that brings what it requires, which a screen can do and a flag
/// must not: a flag that quietly adds a module is a flag nobody can read.
pub fn unmet(picks: &[&str]) -> Result<(), Vec<Diagnostic>> {
    let bad: Vec<Diagnostic> = picks
        .iter()
        .flat_map(|pick| {
            let missing: Vec<(&'static str, &'static str)> = about(pick)
                .needs
                .into_iter()
                .filter(|(need, _)| !picks.contains(need))
                .collect();
            missing.into_iter().map(move |(need, why)| {
                let mut with: Vec<&str> = picks.iter().copied().chain([need]).collect();
                with.sort_unstable();
                Diagnostic::error(
                    *pick,
                    format!("`{pick}` cannot be written without `{need}`"),
                )
                .note(why)
                .help(format!("--with {}", with.join(",")))
            })
        })
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad)
    }
}

/// The full-screen route. Returns the same [`Answers`] the flags produce.
fn chooser(ui: &Ui, args: &CreateArgs, local: Option<String>) -> Result<Answers, Vec<Diagnostic>> {
    let preset = picked(args);
    let draft = Draft {
        name: String::new(),
        title: args.title.clone().unwrap_or_default(),
        choices: all()
            .iter()
            .map(|pick| Choice {
                label: pick.to_string(),
                blurb: about(pick).gist.to_string(),
                // The guided route's shorter voice, which falls back to the
                // gist for a module that did not write one.
                short: about(pick).phrasing().to_string(),
                section: about(pick).section.to_string(),
                starts_on: about(pick).on_with_parent,
                // A child starts ticked once its parent is, which is the
                // asymmetry with the flags: somebody in a terminal building
                // an HTML application wants styling and wants a click not to
                // reload the page, and can see both and untick them. A
                // script gets neither unless it asked, because a flag that
                // quietly brings two more is a flag you cannot read.
                on: preset.contains(pick) || about(pick).on_with_parent,
                under: about(pick)
                    .parent
                    .and_then(|parent| all().iter().position(|p| *p == parent)),
                blocked: about(pick).blocked.map(str::to_string),
                // A tick brings what it requires, and an untick takes the
                // ticks that required it. The screen can do that honestly
                // because every row it changed is still on the screen.
                // Each one carries its own reason, because the chooser
                // prints it the moment the row it names changes under you.
                needs: about(pick)
                    .needs
                    .iter()
                    .filter_map(|(need, why)| {
                        all()
                            .iter()
                            .position(|p| p == need)
                            .map(|at| (at, (*why).to_string()))
                    })
                    .collect(),
            })
            .collect(),
        // The chooser offers a repository and the flag route does not, and the
        // asymmetry is deliberate: a person starting a project in a terminal
        // nearly always wants one, and a script that wanted one would have
        // said `--git`. `--no-git` still wins here, because it was explicit.
        git: !args.no_git,
    };

    let chosen = crate::tui::choose(draft, name_problem).map_err(|e| {
        vec![Diagnostic::error("chooser", e.to_string()).help(
            "this terminal would not take a full-screen view; pass the answers as \
             flags instead, every one of them has a flag",
        )]
    })?;

    // Escape is an answer, and it is not "accepted the defaults". The next act
    // of this command is to create a directory.
    let Some(draft) = chosen else {
        return Err(vec![Diagnostic::warning(
            "cancelled",
            "nothing was written",
        )]);
    };

    // `taken`, not `on`: a ticked child of an unticked parent is a tick
    // nobody can see, and the chooser must not return an answer the screen
    // did not show.
    let picks = all()
        .iter()
        .enumerate()
        .filter(|(index, _)| draft.taken(*index))
        .map(|(_, pick)| *pick)
        .collect();

    let answers = Answers {
        title: if draft.title.is_empty() {
            crate::ui::title_case(&draft.name)
        } else {
            draft.title
        },
        name: draft.name,
        picks,
        local,
        git: draft.git,
    };

    // What the chooser stood in for. Every answer it collects has a flag, and
    // this is that sentence made usable: copy the line into a script and the
    // same project comes out, with nothing to ask. Only here -- a run driven
    // by flags was already the line, and has nothing to report back.
    ui.debug(answers.as_flags());
    Ok(answers)
}

/// A relative `--brocanth-path` is relative to the directory the user typed it
/// in, and the generated manifest sits one deeper, so it is resolved here.
fn local_checkout(args: &CreateArgs) -> Result<Option<String>, Vec<Diagnostic>> {
    match &args.brocanth_path {
        Some(path) => {
            let dir = std::fs::canonicalize(path).map_err(|e| {
                vec![Diagnostic::error("brocanth-path", format!("{path}: {e}"))
                    .help("--brocanth-path takes a directory holding brocanth's Cargo.toml")]
            })?;
            virtual_manifest(&dir)?;
            Ok(Some(dir.display().to_string()))
        }
        None => Ok(None),
    }
}

/// Refuses a checkout root, which is the natural thing to type and the wrong
/// thing to pass.
///
/// Cargo's own message for it names `virtual manifest` and says nothing about
/// what to do instead, so the mistake is worth catching here while the word
/// the user typed is still in hand.
fn virtual_manifest(dir: &std::path::Path) -> Result<(), Vec<Diagnostic>> {
    let manifest = dir.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&manifest) else {
        return Err(vec![Diagnostic::error(
            "brocanth-path",
            format!("{} has no Cargo.toml", dir.display()),
        )
        .help(
            "pass the core crate's own directory, such as <checkout>/crates/brocanth",
        )]);
    };

    // A manifest with a `[workspace]` and no `[package]` describes a workspace
    // and not a crate, so nothing can depend on it by path.
    let has_package = text
        .lines()
        .any(|l| l.trim_start().starts_with("[package]"));
    if has_package {
        return Ok(());
    }

    let core = dir.join("crates/brocanth");
    let mut error = Diagnostic::error(
        "brocanth-path",
        format!("{} is a workspace, not a crate", dir.display()),
    )
    .note("cargo cannot take a path dependency on a virtual manifest");
    if core.join("Cargo.toml").is_file() {
        error = error.help(format!("--brocanth-path {}", core.display()));
    } else {
        error =
            error.help("pass the core crate's own directory, such as <checkout>/crates/brocanth");
    }
    Err(vec![error])
}

/// Single-quotes a value when a shell would otherwise take it apart.
///
/// Only for the debug line. A title is the one answer here that can carry a
/// space, and a line a reader is invited to paste has to survive the paste.
fn shell_quote(value: &str) -> String {
    let safe = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:".contains(c));
    if safe {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// Why `name` is not a usable package name, in a few words, or `None`.
///
/// Split out from [`check_name`] because the chooser needs the reason as text
/// to show under the field as it is typed, and a diagnostic is the wrong shape
/// for that. One rule, two presentations.
fn name_problem(name: &str) -> Option<String> {
    let why = if name.is_empty() {
        "is empty"
    } else if name.contains(['/', '\\', ' ']) {
        "is a path, not a name"
    } else if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        "is not a cargo package name"
    } else if !name.starts_with(|c: char| c.is_ascii_lowercase()) {
        "does not start with a letter"
    } else {
        return None;
    };
    Some(why.to_string())
}

/// A cargo package name and a directory name at once, so the rules are the
/// stricter of the two.
fn check_name(name: &str) -> Result<(), Vec<Diagnostic>> {
    match name_problem(name) {
        Some(why) => Err(vec![Diagnostic::error("name", format!("{name:?} {why}"))
            .help("lowercase letters, digits, - and _, starting with a letter")]),
        None => Ok(()),
    }
}

fn io_error(path: &Path, e: &std::io::Error) -> Vec<Diagnostic> {
    vec![Diagnostic::error("write", format!("{e}")).at(path.display().to_string())]
}

// -- the files --------------------------------------------------------------

/// Ask every chosen module to write, in the order their manifests allow.
///
/// This is the whole extension point. The CLI owns the files every project
/// has; a module owns the files only it knows about, and adds lines to the
/// slots in between. Anything it got wrong comes back as a diagnostic rather
/// than as a project that compiles and is missing a piece.
fn contributions(a: &Answers) -> Result<brocanth_scaffold::Scaffold, Vec<Diagnostic>> {
    let known = all();
    let project = brocanth_scaffold::Project::new(&a.name, &a.title, &a.picks, &known);
    let order = brocanth_scaffold::order(&a.picks, |key| about(key).after).map_err(|stuck| {
        vec![Diagnostic::error(
            "cycle",
            format!("these modules wait for each other: {}", stuck.join(", ")),
        )
        .note("a module's `after` list says which other modules write first")]
    })?;

    let mut built = brocanth_scaffold::Scaffold::new();
    for key in order {
        built.now_writing(key);
        crate::modules::find(key)
            .unwrap_or_else(|| unreachable!("`{key}` is not a module this CLI has"))
            .contribute(&project, &mut built);
    }

    // A registration the provider list does not know about. `register!`
    // widens the builder's type and `plugins::Installed` does not, so the
    // generated `src/app.rs` fails on `App { context }` with a nested
    // associated type that names neither the module nor the slot. One
    // call writes both halves; this catches a module that wrote only the
    // first, and names it.
    let listed: Vec<&str> = built.slot_owners(Slot::PluginsInstalled).collect();
    let unpaired: Vec<&str> = built
        .slot_owners(Slot::AppPlugins)
        .zip(built.slot(Slot::AppPlugins))
        // A module may write commented guidance into the chain without
        // registering anything -- `middleware` writes five of them. Only
        // an uncommented `let builder = ..` is a registration.
        .filter(|(_, block)| block.lines().any(registers))
        .map(|(owner, _)| owner)
        .filter(|owner| !listed.contains(owner))
        .collect();

    let mut wrong: Vec<Diagnostic> = built
        .problems()
        .iter()
        .map(|problem| Diagnostic::error("module", problem.clone()))
        .collect();
    for owner in unpaired {
        wrong.push(
            Diagnostic::error(
                "module",
                format!("`{owner}` registers a plugin that the provider list does not have"),
            )
            .note("`Slot::AppPlugins` was written without `Slot::PluginsInstalled`")
            .help("call `brocanth_scaffold::snippet::plugin`, which writes both"),
        );
    }
    for key in project.unknown_keys() {
        wrong.push(
            Diagnostic::error("module", format!("a module asked about `{key}`"))
                .note("no module in this CLI declares that key, so the answer was `no`")
                .help("check the spelling, or add the module that declares it"),
        );
    }
    if wrong.is_empty() {
        Ok(built)
    } else {
        Err(wrong)
    }
}

/// Every file this project is, with the header and digest on the ones the
/// generator will come back and rewrite.
///
/// `brocanth create` writes the result into a new directory;
/// [`crate::commands::add`] writes the parts of it that a project without
/// the new module did not already have. One generator, two front doors --
/// this repository has already paid twice for two of them drifting apart.
pub fn scaffold(a: &Answers) -> Result<Vec<(String, String)>, Vec<Diagnostic>> {
    let built = contributions(a)?;
    let mut files = scaffold_files(a)?;

    // The record of what all of it was made from, after everything it
    // records: `attributed` reads the paths this list produced.
    files.push((
        "brocanth.toml".to_string(),
        brocanth_toml(a, &built, &files),
    ));

    // Header and digest on the files the generator will come back and
    // rewrite. Everything else is the developer's from the moment it lands.
    Ok(files
        .into_iter()
        .map(|(path, text)| match crate::owned::kind(&path) {
            crate::owned::Kind::Owned => {
                let stamped = crate::owned::stamp(&path, &text);
                (path, stamped)
            }
            _ => (path, text),
        })
        .collect())
}

/// Everything but `brocanth.toml`, and nothing stamped.
///
/// Split out because `brocanth.toml` records which paths each module is
/// responsible for, and it works that out by generating the project again
/// without the module. A record that recorded itself would not terminate.
fn scaffold_files(a: &Answers) -> Result<Vec<(String, String)>, Vec<Diagnostic>> {
    let built = contributions(a)?;
    let mut files = vec![
        ("Cargo.toml".to_string(), cargo_toml(a, &built)),
        (".gitignore".to_string(), {
            let mut out = String::from("/target\n");
            // And whatever a module keeps under the working tree. The
            // slot was declared and never read until a module wrote a
            // line into it and the line went nowhere.
            for line in built.slot(Slot::GitIgnore) {
                // `Writer::line` already ended it. This used to add a
                // second newline and leave a blank after every entry.
                out.push_str(line);
            }
            out
        }),
        // Without a pin, a generated application picks whatever rustc is on
        // PATH, and brocanth's proc-macro artifacts are not compatible across
        // patch releases: the failure is E0514, which names a crate the user
        // has never heard of and says nothing about toolchains.
        (
            "rust-toolchain.toml".to_string(),
            "[toolchain]\nchannel = \"1.98\"\n".to_string(),
        ),
        // And again for mise, which the rest of this ecosystem pins with and
        // whose shim does not read rust-toolchain.toml. Two files because two
        // tools, not because one is a fallback.
        (
            "mise.toml".to_string(),
            // Raw, because the `\n\` form this file used here kept the source's
            // own indentation and wrote it into the TOML.
            mise_toml(&built),
        ),
        (
            "src/main.rs".to_string(),
            if a.has("commands") {
                main_with_commands_rs(a)
            } else {
                main_serve_only_rs(a)
            },
        ),
        // Always: it is the one answer to what this application is, and both
        // shapes of `main` read it.
        ("src/app.rs".to_string(), app_rs(a, &built)),
        ("src/pages/mod.rs".to_string(), pages_mod_rs(a, &built)),
        ("src/pages/index.rs".to_string(), index_rs(a)),
        ("src/pages/about.rs".to_string(), about_rs(a)),
        ("src/pages/users/[id].rs".to_string(), user_rs(a)),
    ];
    // For every project, including one with no plugins. `src/app.rs` asks
    // this file the same four questions whatever the module list is, which
    // is what lets `brocanth add` leave `src/app.rs` alone.
    files.push(("src/plugins/mod.rs".to_string(), plugins_mod_rs(a, &built)));
    if a.has("config") {
        files.push((
            format!("config/{}.toml", brocanth_config::DEFAULT_ENV),
            config_toml(a, &built),
        ));
    }
    if a.has("commands") {
        // No cargo alias. A project's command line is a framework concern and
        // cargo is the build tool; hooking one into the other conflates them.
        // The ways in are `brocanth <command>` from anywhere inside the
        // project, and the built binary itself.
        files.push(("src/commands/mod.rs".to_string(), commands_mod_rs(&built)));
        files.push(("src/commands/serve.rs".to_string(), SERVE_RS.to_string()));
    }
    // The modules' own files, after the ones every project has. A clash
    // with a path above is caught here rather than by whichever happened to
    // be written second.
    for (path, contents) in built.files() {
        if let Some((taken, _)) = files.iter().find(|(p, _)| p == path) {
            return Err(vec![Diagnostic::error(
                "module",
                format!("a module writes {taken}, which the scaffold already writes"),
            )]);
        }
        files.push((path.to_string(), contents.to_string()));
    }

    // The brief goes last because it is the one file about all the others,
    // and the listing `create` prints reads in this order.
    files.push((
        "AGENTS.md".to_string(),
        // Whatever the modules have to say to a model reading the project,
        // appended after the sections every project gets. The slot was
        // declared and never read until a module needed to explain why it
        // had registered nothing.
        agents_md(a) + &built.slot(Slot::AgentsSections).collect::<String>(),
    ));
    Ok(files)
}

/// `brocanth.toml`: the marker, the module list, and what each module put
/// in the project.
///
/// The marker came first and held a name. It is the source of truth now,
/// because `brocanth add` has to answer "what is this project made of"
/// before it can answer anything else, and reading it out of the source
/// would be a parser guessing at a decision somebody already made.
///
/// The per-module record is for a `remove` that does not exist yet, and it
/// is written now because it cannot be added later: a project scaffolded
/// today without it is a project a later `remove` has to guess about. Each
/// entry is derived, never declared -- `files` is the difference between
/// what the generator writes with this module and what it writes without
/// it, so a module that stops writing a file stops recording it.
fn brocanth_toml(a: &Answers, built: &Built, files: &[(String, String)]) -> String {
    let mut out = format!(
        "# What this project is made of. Every other generated file follows from it.\n\
         \n\
         [project]\n\
         name = {name:?}\n\
         title = {title:?}\n\
         \n\
         [modules]\n\
         enabled = [{enabled}]\n",
        name = a.name,
        title = a.title,
        enabled = a
            .picks
            .iter()
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>()
            .join(", "),
    );
    for key in &a.picks {
        out.push_str(&format!(
            "\n[modules.{key}]\nfiles = [{files}]\nconfig = [{sections}]\ncrates = [{crates}]\n",
            files = quoted(&attributed(a, files, key)),
            sections = quoted(&sections_of(built, key)),
            crates = quoted(
                &about(key)
                    .crates
                    .iter()
                    .map(|dep| dep.name.to_string())
                    .collect::<Vec<_>>()
            ),
        ));
    }
    out
}

/// A TOML array's contents, from a list of strings.
fn quoted(values: &[String]) -> String {
    values
        .iter()
        .map(|v| format!("{v:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Which paths this project has because `key` is in it.
///
/// A difference and not a declaration: generate the project again without
/// the module and see what stopped being written. Three of the modules --
/// `config`, `commands` and the rsx family -- write nothing through the
/// scaffold at all, because `create.rs` still writes their files directly,
/// so anything that asked the module would get an empty answer for them.
///
/// A directory, where the module owns one. `src/plugins/jobs/mod.rs` is
/// recorded as `src/plugins/jobs/`, because what the module owns is the
/// directory and what a later `remove` deletes is the directory.
fn attributed(a: &Answers, files: &[(String, String)], key: &str) -> Vec<String> {
    let without = Answers {
        name: a.name.clone(),
        title: a.title.clone(),
        picks: a.picks.iter().copied().filter(|k| k != &key).collect(),
        local: a.local.clone(),
        git: a.git,
    };
    // A smaller module list cannot make two modules collide, so this cannot
    // fail where the list it came from did not.
    let Ok(rest) = scaffold_files(&without) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for (path, _) in files {
        if rest.iter().any(|(p, _)| p == path) {
            continue;
        }
        let owned = match path.strip_suffix("/mod.rs") {
            Some(dir) => format!("{dir}/"),
            None => path.clone(),
        };
        if !out.contains(&owned) {
            out.push(owned);
        }
    }
    // A file inside a directory this module already owns is not a second
    // entry: `src/commands/` and `src/commands/serve.rs` would have a later
    // `remove` delete one thing twice.
    let dirs: Vec<String> = out.iter().filter(|p| p.ends_with('/')).cloned().collect();
    out.retain(|path| path.ends_with('/') || !dirs.iter().any(|dir| path.starts_with(dir)));
    out
}

/// The config sections `key` claimed, read off the text it wrote.
///
/// The section name is parsed back out of the block rather than assumed to
/// be the module's key. They are the same today for every module, and a
/// module that claimed two sections or named one differently would
/// otherwise be recorded wrongly and silently.
fn sections_of(built: &Built, key: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (by, text) in built
        .slot_owners(Slot::ConfigSections)
        .zip(built.slot(Slot::ConfigSections))
    {
        if by != key {
            continue;
        }
        for line in text.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// The brief a coding agent reads before it writes anything here.
///
/// Written by the scaffold rather than left to the developer, because the
/// facts it carries are *this* project's: which capabilities were chosen, and
/// therefore which are absent from the binary. A model that cannot see that
/// list guesses, and the guess is always that the framework has the feature.
///
/// `AGENTS.md` is the filename the common agent tools look for; Claude Code
/// also reads `CLAUDE.md`, and a developer who wants that can symlink it.
fn agents_md(a: &Answers) -> String {
    let mut chosen = vec!["`core`  the router, the context, the plugin seams".to_string()];
    for pick in &a.picks {
        chosen.push(format!("`{}`  {}", pick, about(pick).gist));
    }
    let absent: Vec<String> = available()
        .iter()
        .filter(|p| !a.has(p))
        .map(|p| format!("`{p}`"))
        .collect();
    let absent = if absent.is_empty() {
        "Nothing. Every capability was chosen.".to_string()
    } else {
        format!(
            "{} — not compiled, not linked, absent from `Cargo.lock`. Do not \
             write code that uses them; add the dependency and name the plugin \
             in `main` first.",
            absent.join(", ")
        )
    };

    // A raw string, for the reason given above `COMMANDS_MOD_RS`: the `\n\`
    // continuation used elsewhere here keeps the source's own indentation and
    // writes it into the file, which in Markdown turns a paragraph into a
    // code block.
    let mut views = String::new();
    if a.has("rsx") {
        views.push_str(AGENTS_VIEWS_MD);
    }
    if a.has("rsx-tailwind") || a.has("rsx-htmx") {
        views.push_str(AGENTS_ASSETS_MD);
    }
    if a.has("rsx-htmx") {
        views.push_str(AGENTS_PARTIAL_MD);
    }

    let commands = if a.has("commands") {
        let mut out = String::from(
            "\n## Tasks\n\nA command is a `clap::Args` struct with `#[cmd]` on it, in its own file under\n\
             `src/commands/`, plus a `mod` line. It becomes a subcommand automatically —\n\
             registration happens at link time and there is no list to add it to.\n\n\
             ```\nbrocanth --help      # lists this project's commands too\nbrocanth serve\n",
        );
        if a.has("rsx-tailwind") {
            // The CSS build is one of them, and the reason it is a command
            // rather than a script belongs where an agent will look first.
            out.push_str(
                "brocanth css         # the CSS build; -W rebuilds on each change\n```\n\n\
                 The stylesheet is embedded, so a new one is a new binary. `css` rewrites\n\
                 `src/assets.rs` to say so, which is what makes cargo notice.\n",
            );
        } else {
            out.push_str("```\n");
        }
        out
    } else {
        String::new()
    };

    format!(
        "# Working in {name}\n\
         \n\
         A brief for a coding agent. Written by `brocanth`, so the lists\n\
         below are this project's and not the framework's in general.\n\
         \n\
         ## Check your work\n\
         \n\
         Run these after every change, in this order. They are ordered by how\n\
         fast they fail.\n\
         \n\
         ```\n\
         cargo test\n\
         cargo clippy --all-targets -- -D warnings\n\
         cargo fmt --check\n\
         ```\n\
         \n\
         The route table is `pages::router()` and the test beside it asserts the\n\
         shape of the directory. Read it rather than guessing what covers a\n\
         request.\n\
         \n\
         ## What this binary contains\n\
         \n\
         {chosen}\n\
         \n\
         **Absent:** {absent}\n\
         \n\
         That is the framework's central rule: a capability nobody asked for is\n\
         not compiled and not linked. If you need one, add it to `Cargo.toml`\n\
         *and* name its plugin in `src/main.rs`. Nothing registers itself.\n\
         \n\
         ## Adding a route\n\
         \n\
         A route is a file. `src/pages/about.rs` is `/about`;\n\
         `src/pages/users/[id].rs` is `/users/{{id}}`. **Write the file. That is\n\
         the whole of it** -- no `mod` line, no row to register, because there is\n\
         no list. `brocanth_pages::pages!` in `src/pages/mod.rs` reads this\n\
         directory while the crate compiles and writes both.\n\
         \n\
         The verb comes off the handler's name: `handle` is GET, then\n\
         `handle_head`, `handle_post`, `handle_put`, `handle_patch`,\n\
         `handle_delete`, `handle_options`. A `pub fn handle_*` outside that\n\
         list is a build error naming the list, rather than a handler nothing\n\
         reaches; a page with no handler is a build error too. A private\n\
         `fn handle_*` is a helper and is left alone.\n\
         \n\
         **Adding a file does not rebuild on its own** -- nothing referenced it\n\
         yet. Run `touch src/pages/mod.rs` after creating a page or a layout.\n\
         Editing or deleting one is noticed.\n\
         \n\
         A handler returns a value and never writes a response itself. That is\n\
         what makes a half-written body on an error impossible rather than\n\
         unlikely.\n\
         {views}\
         {commands}\
         \n\
         ## House style\n\
         \n\
         - A comment says **why**, next to the decision, and what was chosen\n\
         \x20 against. Not what the line does.\n\
         - A test name is a sentence describing the guarantee it protects.\n\
         - Prefer a type that cannot be built wrong over a check that can be\n\
         \x20 forgotten.\n\
         - State a limit where someone would look for it rather than leaving it\n\
         \x20 to be discovered.\n\
         \n\
         ## Framework reference\n\
         \n\
         `docs/llm-authoring.md` in the brocanth checkout is the full brief: what\n\
         cannot compile and can therefore be relied on, what can still go wrong\n\
         and therefore needs a test, and the crate map.\n",
        name = a.title,
        chosen = chosen
            .iter()
            .map(|l| format!("- {l}"))
            .collect::<Vec<_>>()
            .join("\n"),
        absent = absent,
        views = views,
        commands = commands,
    )
}

/// The `## Views` section of `AGENTS.md`, written only when there are views.
const AGENTS_VIEWS_MD: &str = r#"
## Views

The markup for a route is a `.rsx` file **next to the page that renders it**:
`src/pages/index.rsx` beside `src/pages/index.rs`. There is no second tree and
nothing to register -- `rsx::load_dir!("src/pages")` in `src/pages/mod.rs`
compiles them all into the binary. One file is one module, under `views`, so
`index.rsx` is `views::index` and `users/[id].rsx` is `views::users::_id_`.

### Layouts

`layout.rsx` wraps every page in its directory **and every directory below**,
and a page never names it: which layout wraps a page is a property of where
the page is. That is the Crystal page_router's rule for `layout.cr`.

They stack root to leaf, and **the chain is not written down anywhere**.
`html!` is a macro: it knows which file it was written in, walks up to
`src/`, and wraps the page in every `layout.rsx` it passes. So
`src/pages/users/layout.rsx` wraps the pages under `users/` inside the root
layout, and deleting that file removes exactly that layer with no other edit.

Every page uses the same import and the same call, whatever directory it is
in. Do not add a chain of your own; if a page is wrapped wrongly, the
directory tree is what to change.

A layout takes `children: Element`, and may also take `layout: &str`, which
`html!` fills with the URL prefix that layout covers. Those two are the only
parameters it fills, because it cannot know what a third one would mean.
A value every page shares comes from a `const` -- the root layout reads
`pages::TITLE` that way. A value belonging to one request belongs on the page
component, which the handler builds and which already has the context.

For a fragment that is not a document -- an htmx swap, a partial -- call
`render` instead of `html!`: the same thing with no shell.

**Adding a `layout.rsx` where there was none does not rebuild anything**,
because nothing referenced the file yet. Run `touch src/pages/mod.rs` after
creating one. Editing or deleting an existing layout is noticed.

A component is a function returning `Element`. Its parameters are its contract
with the handler, and a caller fills them as struct fields, so a field the
component does not declare is a compile error rather than a blank in the page.
A parameter of type `Element` receives children, which is all a layout is.

Reach a template from a handler with `html!`, and with nothing else:

```rust
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    html!(ctx, views::index::Index { title: TITLE })
}
```

It returns the handler's own `Result`, so there is no `Ok(..)` to write and a
template that bails propagates with `?`. For another status or a header,
compose with the other return shapes: `Ok((html!(ctx, tpl)?, 404))`.

Adding a page with markup is two files in one directory -- `<name>.rs` and
`<name>.rsx` -- and nothing else anywhere. Neither needs a line in
`src/pages/mod.rs`, because the directory is the registry.

Not every response is a page. A redirect is `Response::empty(303)` with a
header and no template anywhere near it.

An interpolated value is escaped on the way out. Templates are read at
**compile** time, so an edit needs a rebuild, and a template that does not
parse is a build failure on its own line rather than a 500.
"#;

/// The static file rules, for a project that has files to serve.
const AGENTS_ASSETS_MD: &str = r#"
## Static files

Two directories, and the difference matters. `assets/` is **fingerprinted**:
`site.css` is published at `/css/site.<hash>.css` and cached for a year.
`public/` is served **verbatim**, for names that cannot move -- `robots.txt`,
`favicon.ico`, anything linked to from outside.

Never write either URL by hand. `crate::assets::url("css/site.css")` takes the
logical path and returns the published one, and a name nothing publishes
panics there rather than 404ing in a browser.

A basename starting with `_` is an input and is never published, which is what
`assets/css/_tailwind.css` is.

Both directories are embedded in the binary. **Adding a file where there was
none does not rebuild anything**; run `touch src/assets.rs` after creating one.
"#;

/// The partial navigation contract, for a project that asked for it.
const AGENTS_PARTIAL_MD: &str = r#"
## Partial navigation

Link to a route with the `Link` component, never a bare `<a>`:

```
<Link to={&crate::pages::routes::users::id::path(7)}>User 7</Link>
```

`to` takes what a route helper returns, so a link to a route that does not
exist is a compile error. It renders a real `<a href>` with `dp-boost` on it,
so the page still works with no JavaScript.

A layout marks what a swap may replace with `dp-layout={layout}`, on the
element wrapping **exactly** the children. Anything inside that element is
replaced with them, so a sidebar or a scroll position belongs outside it.

A request carrying `DP-Partial: n` drops the outermost `n` layouts, and
`html!` resolves that into a `match` over the chain. A handler never reads the
header and never changes for it.
"#;

/// One command, as an example to copy.
// A raw string, and deliberately: the `\n\` continuation used elsewhere in
// this file eats the newline *and every leading space on the next line*, so an
// indented template written that way is emitted flat. This one was, until a
// scaffolded project was run through rustfmt. PLAN.md §4.8 -- "if scaffolding
// produces code you would not write by hand, the framework below it is wrong"
// -- and nobody writes an unindented struct body.
/// `src/commands/mod.rs`: the `mod` lines, and nothing else.
fn commands_mod_rs(built: &Built) -> String {
    let mut out = String::from(
        r#"//! Every command in this project.
//!
//! The `mod` lines are the whole of it. A command registers itself through a
//! link-time slice, so adding one is a file and a `mod`, and there is no list
//! to keep in step.
//!
//! `serve` arrived with the scaffold and is ordinary code here: rename it,
//! give it flags, or delete it. The framework does not own this command line.

"#,
    );
    // Alphabetical, which is also what rustfmt wants. A scaffold that hands
    // a developer a diff on their first `cargo fmt` is a scaffold that got
    // it wrong.
    // Alphabetical, which is also what rustfmt wants, so a module's line
    // is sorted in with the two this file writes rather than appended.
    let mut lines: Vec<String> = built
        .slot(Slot::CommandsMod)
        // A slot line arrives with its newline; the sort and the writer
        // below both want it without one.
        .map(|line| line.trim_end().to_string())
        .chain(std::iter::once("pub mod serve;".to_string()))
        .collect();
    lines.sort();
    lines.dedup();
    for line in lines {
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// Where a brocanth crate comes from: a local checkout when one was named, the
/// repository otherwise.
fn source_of(a: &Answers, dep: &Dep) -> String {
    let crate_name = dep.name;
    let note = dep.note.map_or_else(String::new, |n| format!("  # {n}"));
    // Features and `default-features` belong to the dependency whatever it
    // is fetched from, so they are written once here rather than in each
    // of the four shapes below. `Dep::feature` existed, was merged across
    // modules, and was never written out: a module could ask for a feature
    // and the manifest would not have it.
    let mut extra = String::new();
    if !dep.features.is_empty() {
        let names: Vec<String> = dep.features.iter().map(|f| format!("{f:?}")).collect();
        extra.push_str(&format!(", features = [{}]", names.join(", ")));
    }
    if !dep.defaults {
        extra.push_str(", default-features = false");
    }

    let where_from = match &dep.source {
        // Not a brocanth crate, but `--brocanth-path` still has something
        // to say: a checkout beside the brocanth repository stands in for
        // the repository, the same way the flag does for brocanth's own
        // crates. It is what makes `the_scaffold_compiles` able to build a
        // project against work that is not pushed yet. Without the flag, a
        // generated project always gets the URL.
        Source::Git { url, branch } => {
            match a.local.as_deref().and_then(|at| sibling(at, crate_name)) {
                Some(dir) => format!("path = {:?}", dir.display().to_string()),
                None => format!("git = {url:?}, branch = {branch:?}"),
            }
        }
        Source::Version(req) => format!("version = {req:?}"),
        Source::Brocanth => match &a.local {
            // `--brocanth-path` points at `crates/brocanth`; its siblings
            // are the other crates of the same checkout.
            Some(path) => {
                let dir = if crate_name == "brocanth" {
                    Path::new(path).to_path_buf()
                } else {
                    Path::new(path)
                        .parent()
                        .map(|p| p.join(crate_name))
                        .unwrap_or_else(|| Path::new(path).to_path_buf())
                };
                format!("path = {:?}", dir.display().to_string())
            }
            None => format!("git = {REPOSITORY:?}"),
        },
    };

    format!("{crate_name} = {{ {where_from}{extra} }}{note}")
}

/// A checkout of `name` beside the brocanth repository, or `None`.
///
/// `--brocanth-path` points at `crates/brocanth`, so the repository is two
/// levels up and its siblings are one more. A directory with no
/// `Cargo.toml` is not a checkout and does not count.
///
/// Two layouts, because the neighbours have both. `rsx` and `zod` are one
/// package at the root of their repository. `jt` is a workspace whose
/// package is `crates/jt`, which is how this repository is laid out too.
/// A path pointing at a virtual manifest is not a dependency cargo can
/// read -- it says so, and names neither the flag nor the fix -- so the
/// member is found here instead.
fn sibling(brocanth_crate: &str, name: &str) -> Option<std::path::PathBuf> {
    let repo = Path::new(brocanth_crate).parent()?.parent()?;
    let root = repo.parent()?.join(name);
    [root.join("crates").join(name), root]
        .into_iter()
        .find(|dir| is_package(dir))
}

/// Whether this directory holds a manifest cargo can depend on.
///
/// A `[workspace]` with no `[package]` is a virtual manifest: a real file,
/// and not a crate.
fn is_package(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|text| {
        text.lines()
            .any(|line| line.trim_start().starts_with("[package]"))
    })
}

/// One of brocanth's own crates, which every project gets whatever it chose.
fn core(name: &'static str) -> Dep {
    Dep::brocanth(name)
}

/// The manifest, and the comment that makes it a statement rather than a list.
/// Every crate the chosen modules ask for, merged by name.
///
/// Two modules may want one crate with different features. Until this
/// existed each `Dep` became its own line, so the pair wrote the crate
/// into `[dependencies]` twice and cargo refused to read the manifest.
/// Nothing hit it, because the two places it would have — an
/// `auth-oauth` child turning on a provider, a `storage-variants` child
/// turning on the image decoders — were each abandoned on discovering it.
/// Both are now possible.
///
/// Features union. `default-features = false` survives only if *every*
/// requester asked for it, which is what cargo does anyway: one dependant
/// wanting the defaults turns them on for all.
fn merged(picks: &[&'static str]) -> Result<Vec<Dep>, Vec<Diagnostic>> {
    let mut out: Vec<Dep> = Vec::new();
    for pick in picks {
        for dep in about(pick).crates {
            match out.iter_mut().find(|held| held.name == dep.name) {
                None => out.push(dep),
                Some(held) => {
                    // Two sources for one crate is not a merge, it is two
                    // different crates sharing a name, and guessing which
                    // one the project meant is worse than stopping.
                    if held.source != dep.source {
                        return Err(vec![Diagnostic::error(
                            *pick,
                            format!("two modules want `{}` from different places", dep.name),
                        )
                        .note("a dependency line can name one source")]);
                    }
                    for feature in dep.features {
                        if !held.features.contains(&feature) {
                            held.features.push(feature);
                        }
                    }
                    held.defaults = held.defaults || dep.defaults;
                    held.note = held.note.or(dep.note);
                }
            }
        }
    }
    Ok(out)
}

fn cargo_toml(a: &Answers, built: &Built) -> String {
    let mut deps = vec![source_of(a, &core("brocanth"))];
    // `expect` and not `?`: `ask` already refused a pick whose module is
    // not in this build, and two sources for one name is a mistake in this
    // repository's own registry rather than anything a user typed.
    for dep in merged(&a.picks).expect("the registry does not disagree with itself") {
        deps.push(source_of(a, &dep));
    }
    // The macros that read `src/pages`: the route table always, and the
    // layouts when there are any. Compile-time only -- a proc macro puts no
    // bytes in the binary, which is why this one is not a choice.
    deps.push(source_of(a, &core("brocanth-pages")));
    if a.has("rsx-tailwind") || a.has("rsx-htmx") {
        // The static files, embedded. Also a proc macro, but this one does
        // put bytes in the binary -- the files themselves.
        deps.push(source_of(a, &core("brocanth-assets")));
    }
    {
        // Every project serves, so every project gets a socket and a reactor.
        deps.push(source_of(a, &core("brocanth-server")));
        deps.push(
            "tokio = { version = \"1\", features = [\"rt-multi-thread\", \"macros\", \"signal\", \"sync\", \"time\"] }  # the reactor `serve` runs on, and the signals that stop it"
                .to_string(),
        );
    }
    if needs_serde(a, built) {
        deps.push(
            "serde = { version = \"1\", features = [\"derive\"] }  # the config plugin reads typed sections"
                .to_string(),
        );
    }

    // The receipt, read from the other end. Every module that was not
    // chosen and that has something to say about its own absence says it
    // here, in the order the chooser lists them -- so a contributed module
    // appears without a line changing in this file.
    let mut absent: Vec<(&str, &str)> = Vec::new();
    for key in available().into_iter().filter(|key| !a.has(key)) {
        if let Some(pair) = about(key).absent {
            absent.push(pair);
        }
    }
    if !needs_serde(a, built) {
        absent.push(("serde", "nothing here is deserialised"));
    }
    // The column is measured rather than guessed. It was 17, which fitted
    // every name until one was exactly 17 characters long and the reason
    // arrived with no space in front of it.
    let column = absent.iter().map(|(what, _)| what.len()).max().unwrap_or(0) + 2;
    let absent = format!(
        "#\n# Not here, because nothing asked for it:\n{}\n",
        absent
            .iter()
            .map(|(what, why)| format!("#   {what:<column$}{why}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    // With commands there are two binaries, the application and `manage`, and
    // cargo refuses to guess between them: a brand-new project where
    // `cargo run` answers "could not determine which binary to run" is a bad
    // first minute. Naming the default here costs one line and means the
    // obvious command works.
    // A second compile target, where a module asked for one.
    //
    // A file under `src/bin/` needs no section: cargo finds it, and writing
    // one as well is the duplicate-name error. A binary anywhere else is
    // invisible to cargo and gets a section, which is also the only form a
    // reader can see from the manifest alone.
    let mut bins = String::new();
    for bin in built.binaries() {
        if bin.path == format!("src/bin/{}.rs", bin.name) {
            continue;
        }
        bins.push_str(&format!(
            "\n[[bin]]\nname = {:?}\npath = {:?}\n",
            bin.name, bin.path
        ));
    }

    // `cargo run` with two binaries and no default answers "could not
    // determine which binary to run", which is a bad first minute. The
    // command line is one second binary; a module's is another.
    let default_run = if a.has("commands") || built.binaries().next().is_some() {
        format!("default-run = {:?}\n", a.name)
    } else {
        String::new()
    };

    format!(
        "[package]\n\
         name = {name:?}\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         {default_run}\
         \n\
         # Exactly what was chosen when this application was scaffolded.\n\
         #\n\
         # The core is a router handing a request and a context to a handler, and it\n\
         # depends on none of the rest. A capability that is not listed below is not\n\
         # compiled, not linked, and contributes nothing: no section in the config, no\n\
         # field on the context, no bytes in the binary.\n\
         {absent}\
         [dependencies]\n\
         {deps}\n\
         {bins}",
        name = a.name,
        bins = bins,
        default_run = default_run,
        deps = deps.join("\n"),
        absent = if absent.contains("#   ") {
            absent
        } else {
            String::new()
        },
    )
}

/// The modules that register a plugin instance in `build`.
///
/// Read off the slot rather than listed here. A module that wrote a line
/// into the builder chain has registered an instance, by definition, and a
/// list in this file would be a second copy of that fact for somebody to
/// forget. The instance is named after the module, which is also the
/// config section it claims.
// Only a test asks, today. It is a function rather than a line inside that
// test because the next thing to ask is the generated `main`, and the
// question is about the project rather than about the assertion.
#[cfg(test)]
fn instances(built: &Built) -> Vec<&str> {
    built.slot_owners(Slot::AppPlugins).collect()
}

/// Whether anything in this project deserialises.
///
/// Only the config plugin makes that possible, and only a module that
/// claimed a config section makes it necessary: a plugin built from a
/// literal in `main` needs no deserialiser, and the scaffold derives none.
fn needs_serde(a: &Answers, built: &Built) -> bool {
    a.has("config") && !built.slot_is_empty(Slot::ConfigSections)
}

/// `src/app.rs`: what this binary is, in the order it happens.
///
/// # Why this file no longer carries the plugin list
///
/// It used to. The builder chain, the imports the chain needed and the
/// context demonstration were all written here by the generator, which
/// meant the generator owned the one file a developer edits first. A
/// project could then be scaffolded and never added to.
///
/// They live in `src/plugins/mod.rs` now, which the generator also writes
/// and nobody edits. What is left here is what a person decides: where a
/// layer is mounted, what a 404 is, and what the response to a matched
/// route is. Those do not follow from the module list, so nothing rewrites
/// them.
fn app_rs(a: &Answers, built: &Built) -> String {
    let mut out = String::new();

    out.push_str(&format!(
        "//! {title} -- everything this binary contains, in the order it happens.\n\
         //!\n\
         //! This file is yours. `brocanth add` does not rewrite it: what it would\n\
         //! have to say is in `src/plugins/mod.rs`, which is generated, and this one\n\
         //! only asks that file four questions.\n\
         //!\n\
         //! A plugin that is not named in `plugins::register!` is not registered, and\n\
         //! one that is not in `Cargo.toml` is not compiled at all. \"The framework\n\
         //! decided for you and you cannot see where\" is the thing this layout exists\n\
         //! to make impossible.\n\
         //!\n\
         //! `serve` and every other command read this, so there is one answer to what\n\
         //! this application is.\n\
         \n\
         use std::sync::Arc;\n\
         use std::time::{{Duration, Instant}};\n\
         \n\
         use brocanth::plugin_app::AppBuilder;\n\
         use brocanth::plugin_shutdown::ShutdownReason;\n\
         use brocanth::routing::{{Dispatch, MiddlewareStack, Router}};\n\
         \n\
         use crate::pages::{{self, Ctx, Handler, IntoResponse, Response}};\n\
         use crate::plugins;\n\
         \n",
        title = a.title
    ));

    out.push_str(&format!(
        "/// The application name. Every environment variable this binary reads is\n\
         /// `{upper}_<SUFFIX>`: one convention, derived once, whatever spelling the\n\
         /// name arrived in.\n\
         pub const APP: &str = {name:?};\n\
         \n\
         /// What a request needs, built once and shared by every connection.\n\
         pub struct App {{\n\
        \x20   pub router: Router<Handler>,\n\
        \x20   pub stack: MiddlewareStack<Ctx, Response>,\n\
        \x20   pub plugins: Vec<String>,\n\
        \x20   /// The application the plugins built.\n\
        \x20   ///\n\
        \x20   /// Held rather than dropped, because three things need it\n\
        \x20   /// after `build` returns: a per-request context, the routes a\n\
        \x20   /// plugin contributed, and an HTTP test client. It used to be\n\
        \x20   /// a boxed closure over `context()` alone, which cost an\n\
        \x20   /// indirect call per request and reached none of the rest.\n\
        \x20   ///\n\
        \x20   /// Nameable at last: `plugins::Installed` and `plugins::Hooks`\n\
        \x20   /// are the two type parameters, and the generator writes both.\n\
        \x20   pub core: Core,\n\
         }}\n\
         \n\
         /// The built application, with its provider list and hook record.\n\
         type Core = brocanth::plugin_app::App<plugins::Installed, plugins::Hooks>;\n\
         \n\
         /// Builds it. The only place that decides what this binary is.\n\
         pub fn build() -> Result<App, String> {{\n\
        \x20   // 1. The application, and every plugin it contains. The chain is in\n\
        \x20   //    `src/plugins/mod.rs`, one line per plugin and each one a line a\n\
        \x20   //    reader can delete. It is there rather than here because the\n\
        \x20   //    generator rewrites it whenever the module list changes.\n\
        \x20   let app = plugins::register!(AppBuilder::new())\n\
        \x20       .setup(|setup| {{\n\
        \x20           // A plugin *offers* middleware and cannot mount it: a\n\
        \x20           // `MiddlewareCatalog` has no `mount`. Mounting takes a pattern, the\n\
        \x20           // pattern is a decision, and the decision is the application's --\n\
        \x20           // which is why these lines are here, in the file you own.\n",
        upper = a.ident().to_uppercase(),
        name = a.name
    ));
    for line in built.slot(Slot::AppSetup) {
        out.push_str(line);
    }
    if built.slot_is_empty(Slot::AppSetup) {
        out.push_str(
            "            // Nothing in this binary offers middleware, so nothing is mounted.\n\
            \x20           let _ = setup.router();\n",
        );
    }
    out.push_str(
        "            Ok(())\n\
        \x20       })\n\
        \x20       .build()\n\
        \x20       .map_err(|e| e.to_string())?;\n\
         \n\
        \x20   // 2. The route table: the shape of src/pages and nothing else. A file is\n\
        \x20   //    a route, and `pages::router` is the only place that says so.\n\
        \x20   let router = pages::router().map_err(|e| e.to_string())?;\n\
         \n\
        \x20   // 3. The chain, from exactly what the block above mounted. A name this\n\
        \x20   //    binary has no layer for stops the application rather than silently\n\
        \x20   //    not running.\n\
        \x20   let mut stack: MiddlewareStack<Ctx, Response> = MiddlewareStack::new();\n\
        \x20   for mount in app.router_plan().mounts() {\n\
        \x20       let layer = plugins::layer(&mount.middleware).ok_or_else(|| {\n\
        \x20           format!(\n\
        \x20               \"`{}` is mounted, but this binary has no layer for it\",\n\
        \x20               mount.middleware\n\
        \x20           )\n\
        \x20       })?;\n\
        \x20       stack\n\
        \x20           .use_at(&mount.pattern, layer)\n\
        \x20           .map_err(|e| e.to_string())?;\n\
        \x20   }\n\
         \n\
        \x20   // 4. What the plugins put on the per-request context, said once.\n\
        \x20   plugins::announce!(&app);\n\
         \n\
        \x20   let plugins = app.instance_names().map(str::to_string).collect();\n\
         \n\
        \x20   Ok(App {\n\
        \x20       router,\n\
        \x20       stack,\n\
        \x20       plugins,\n\
        \x20       core: app,\n\
        \x20   })\n\
         }\n\
         \n",
    );

    out.push_str(SENT_FN);
    if a.has("rsx-tailwind") || a.has("rsx-htmx") {
        out.push_str(STATIC_FN);
    }
    if a.has("rsx-htmx") {
        out.push_str(HOLDING_FN);
    }

    out.push_str(&serve_fns(a));
    out.push_str(
        "/// One request: the table, then the chain, then the handler.\n\
         ///\n\
         /// The chain runs only for a route that matched. A 404 has no route and\n\
         /// therefore no context, which is also why the request hooks the plan\n\
         /// reserves for a logger are the core's job and not a layer's.\n\
         pub fn dispatch(app: &App, request: &brocanth_server::Request) -> Response {\n\
        \x20   let (verb, path) = (request.verb, request.path.as_str());\n\
        \x20   match app.router.dispatch(verb, path) {\n\
        \x20       Dispatch::Found { handler, matched } => {\n\
        \x20           let mut ctx = @CTX@;\n\
        \x20           // The one place a failed handler becomes a response --\n\
        \x20           // what Echo calls an HTTPErrorHandler. It sits inside\n\
        \x20           // the chain's terminal, so a layer still sees what the\n\
        \x20           // error turned into and can rewrite it.\n\
        \x20           app.stack.run(path, &mut ctx, |c| {\n\
        \x20               handler(c).unwrap_or_else(IntoResponse::into_response)\n\
        \x20           })\n\
        \x20       }\n\
        \x20       Dispatch::MethodNotAllowed { allow, .. } => Response {\n\
        \x20           status: 405,\n\
        \x20           headers: vec![(\"allow\".to_string(), allow.to_string())],\n\
        \x20           ..Default::default()\n\
        \x20       },\n\
        \x20       Dispatch::NotFound => Response::new(404, \"Not Found\"),\n\
        \x20   }\n\
         }\n",
    );
    // One of two shapes, and the difference is whether anything can skip a
    // layout. Without templates there is no `html!`, so counting layers
    // would be counting for nobody.
    // What the router matched, then what the client sent. The second is a
    // copy: the server's request lives for the connection, the handler's
    // context lives for the handler, and a borrow would tie the two.
    // Two shapes, because rustfmt has two: one call after `Ctx::new` stays
    // on the line, and two break the chain.
    let args = "\x20               &request.query,\n\
         \x20               request.body.to_vec(),\n\
         \x20               request\n\
         \x20                   .headers()\n\
         \x20                   .map(|(name, value)| (name.to_string(), value.to_string()))\n\
         \x20                   .collect(),\n";
    let deeper: String = args.lines().map(|line| format!("    {line}\n")).collect();
    // `.with_plugins` last, and always: a handler reads what a plugin put
    // on the context, and `Ctx::new` cannot build one because the
    // providers live in the application. This is the one place that has
    // both.
    out.replace(
        "@CTX@",
        &if a.has("rsx-htmx") {
            format!(
                "Ctx::new(verb, path, matched.params())\n\
                 \x20               .holding(holding(request))\n\
                 \x20               .with_plugins(app.core.context())\n\
                 \x20               .sent(\n{deeper}\x20               )"
            )
        } else {
            format!(
                "Ctx::new(verb, path, matched.params())\n\
                 \x20               .with_plugins(app.core.context())\n\
                 \x20               .sent(\n{deeper}\x20               )"
            )
        },
    )
}

/// The grace period, `serve`, and the stop that follows it, in `src/app.rs`.
///
/// The same text whatever the module list is: `src/app.rs` is the
/// developer's, and `brocanth add` does not rewrite it. Where the grace
/// period comes from does vary -- `[server] shutdown_grace_secs` with the
/// config plugin, the constant without -- so that question goes to
/// `plugins::shutdown_grace`, which the generator owns.
fn serve_fns(a: &Answers) -> String {
    let dispatch = if a.has("rsx-tailwind") || a.has("rsx-htmx") {
        // Static files never reach a page. They are bytes with a content
        // type, and the page contract carries a `String` body on purpose.
        "serve_asset(&request).unwrap_or_else(|| sent(dispatch(&app, &request)));"
    } else {
        "sent(dispatch(&app, &request));"
    };
    SERVE_FN.replace("@DISPATCH@", dispatch) + "\n"
}

/// `serve`, the drain, and the signals. ADR 2 in the generated project.
const SERVE_FN: &str = r#"/// How long a stop waits for open requests and then for the plugins.
///
/// Kubernetes sends SIGTERM, waits `terminationGracePeriodSeconds` (30 by
/// default) and then sends SIGKILL. 25 seconds leaves 5 for the exit. With
/// the config plugin, `[server] shutdown_grace_secs` overrides it: see
/// `plugins::shutdown_grace`.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(25);

/// Serves until SIGTERM or SIGINT, then stops in two phases (PLAN.md ADR 2).
///
/// 1. The listener closes. Each open request has the grace period to
///    finish, and each open response stream is told to end.
/// 2. Each plugin stops, in reverse registration order, inside what is left
///    of the grace period.
///
/// An error is a non-zero exit, and an orchestrator reads it: the drain ran
/// out of time, the listener failed, or a plugin did not stop cleanly.
///
/// Synchronous on the outside because the command that calls it is, and
/// the runtime is built here rather than wrapped around `main`: a command
/// that never serves should not start a reactor to run.
pub fn serve(addr: &str) -> Result<(), String> {
    let grace = plugins::shutdown_grace()?;
    // Before the socket, so a signal during startup stops the server
    // through the drain and does not kill the process.
    let stop = watch(grace).map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;

    // The runtime first, and the application inside it. A plugin
    // that opens a connection pool does it in `before_start`,
    // where the hook is synchronous and connecting is not, so it
    // reaches for the current runtime handle and blocks on a
    // scoped thread. Building the application first would leave
    // it with no handle to reach for.
    let app = {
        let _guard = runtime.enter();
        build()?
    };
    eprintln!("{APP}: plugins: [{}]", app.plugins.join(", "));

    let result = runtime.block_on(run(addr, app, stop, grace));
    // A request that outlived the drain can still be on a worker. Waiting
    // for it here would undo the grace period, so it is dropped.
    runtime.shutdown_background();
    result
}

/// The async half of [`serve`].
async fn run(
    addr: &str,
    mut app: App,
    mut stop: tokio::sync::mpsc::UnboundedReceiver<Stop>,
    grace: Duration,
) -> Result<(), String> {
    // One gate: a stop at the listener and a stop at `App::shutdown`
    // close the same set of response streams.
    let server = brocanth_server::Server::bind(addr)
        .await
        .map_err(|e| e.to_string())?
        .with_streams(app.core.streams().clone());
    let bound = server.local_addr().map_err(|e| e.to_string())?;
    let stopper = server.stopper();

    // After the bind, so a plugin does not start work for a port that
    // turned out to be taken.
    if let Err(e) = app.core.start() {
        eprintln!("{APP}: {}", app.core.shutdown(ShutdownReason::Failed, None));
        return Err(e.to_string());
    }

    let app = Arc::new(app);
    let mut serving = tokio::spawn(server.run({
        let app = Arc::clone(&app);
        move |request| {
            // Dispatch is synchronous and runs here, outside the
            // future, so what crosses the await is a finished
            // response and nothing borrowed.
            let out = @DISPATCH@
            async move { out }
        }
    }));
    eprintln!("{APP}: listening on http://{bound}");

    // Phase 1: stop accepting, and give the open requests the grace period.
    let mut failed = tokio::select! {
        event = stop.recv() => {
            let signal = match event {
                Some(Stop::Signal(signal)) => signal,
                _ => "stop",
            };
            eprintln!("{APP}: {signal}: stopping; open requests have {grace:?} to finish");
            None
        }
        // The listener stopped by itself. That is a failure, and the
        // plugins still stop.
        served = &mut serving => Some(stopped(served)),
    };
    let reason = match failed {
        None => ShutdownReason::Signal,
        Some(_) => ShutdownReason::Failed,
    };
    let started = Instant::now();
    if failed.is_none() {
        stopper.stop();
        failed = tokio::select! {
            served = &mut serving => match served {
                Ok(Ok(())) => None,
                other => Some(stopped(other)),
            },
            event = stop.recv() => Some(match event {
                // A second Ctrl-C means "now".
                Some(Stop::Signal(signal)) => {
                    format!("a second {signal} arrived, so open requests were cut off")
                }
                _ => format!("open requests did not finish in {grace:?} and were cut off"),
            }),
        };
    }
    // Nothing when the drain finished. After a timeout, this drops the
    // connections that are still open.
    serving.abort();

    // Phase 2: the plugins, inside what is left of the grace period.
    let remaining = grace.saturating_sub(started.elapsed());
    match owned(app).await {
        Some(mut app) => {
            let report = app.core.shutdown(reason, Some(remaining));
            eprintln!("{APP}: {report}");
            if failed.is_none() && (!report.is_clean() || report.overran()) {
                failed = Some(format!("the plugins did not stop cleanly in {grace:?}"));
            }
        }
        None => eprintln!("{APP}: a request still holds the application, so no plugin stopped"),
    }
    match failed {
        None => Ok(()),
        Some(why) => Err(why),
    }
}

/// Why the listener task ended before a clean drain.
fn stopped(
    served: Result<Result<(), brocanth_server::ServeError>, tokio::task::JoinError>,
) -> String {
    match served {
        Ok(Ok(())) => "the listener stopped with no signal".to_string(),
        Ok(Err(e)) => e.to_string(),
        Err(e) => e.to_string(),
    }
}

/// The application back from the connections that shared it, or `None` when
/// one of them still runs.
///
/// `App::shutdown` needs `&mut`. A finished connection task can be a few
/// instructions from dropping its clone, so this yields a few times first.
async fn owned(mut app: Arc<App>) -> Option<App> {
    for _ in 0..64 {
        match Arc::try_unwrap(app) {
            Ok(app) => return Some(app),
            Err(shared) => {
                app = shared;
                tokio::task::yield_now().await;
            }
        }
    }
    None
}

/// What the stop thread reports to [`run`].
enum Stop {
    /// SIGTERM or SIGINT, by name.
    Signal(&'static str),
    /// The grace period since the first signal is over.
    Deadline,
}

/// Watches for SIGTERM and SIGINT on a thread of its own. It reports each
/// signal, and the end of the grace period after the first one.
///
/// Not on the serving runtime. A handler is synchronous and holds its
/// worker, and while it runs that runtime can leave its signal and timer
/// drivers unpolled: a stop would wait for the slowest request, and the
/// grace period would not run out. This thread has a runtime of its own,
/// and the channel wakes `run` directly.
fn watch(grace: Duration) -> std::io::Result<tokio::sync::mpsc::UnboundedReceiver<Stop>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut signals = {
        let _guard = runtime.enter();
        Signals::listen()?
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("stop".to_string())
        .spawn(move || {
            runtime.block_on(async move {
                let first = signals.next().await;
                if tx.send(Stop::Signal(first)).is_err() {
                    return;
                }
                let next = tokio::select! {
                    () = tokio::time::sleep(grace) => Stop::Deadline,
                    second = signals.next() => Stop::Signal(second),
                };
                let _ = tx.send(next);
            });
        })?;
    Ok(rx)
}

/// SIGTERM and SIGINT as one stream. An orchestrator sends SIGTERM, and
/// Ctrl-C is SIGINT.
#[cfg(unix)]
struct Signals {
    term: tokio::signal::unix::Signal,
    int: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn listen() -> std::io::Result<Self> {
        use tokio::signal::unix::{signal, SignalKind};
        Ok(Self {
            term: signal(SignalKind::terminate())?,
            int: signal(SignalKind::interrupt())?,
        })
    }

    /// The next signal, by name.
    async fn next(&mut self) -> &'static str {
        tokio::select! {
            _ = self.term.recv() => "SIGTERM",
            _ = self.int.recv() => "SIGINT",
        }
    }
}

/// Ctrl-C alone, where there is no SIGTERM.
#[cfg(not(unix))]
struct Signals;

#[cfg(not(unix))]
impl Signals {
    fn listen() -> std::io::Result<Self> {
        Ok(Self)
    }

    /// The next signal, by name.
    async fn next(&mut self) -> &'static str {
        let _ = tokio::signal::ctrl_c().await;
        "Ctrl-C"
    }
}
"#;

/// `mise.toml`. The sections are `[tasks.*]`, which is mise's spelling and not
/// brocanth's -- a rename of brocanth's own vocabulary once turned these into
/// `[commands.*]`, and mise answered `unknown field`.
/// `src/main.rs` when the project has no command line: it serves, and that is
/// all it does.
fn main_serve_only_rs(a: &Answers) -> String {
    format!(
        r##"//! {title} -- a brocanth application.
//!
//! No command line was asked for, so this binary does one thing. Everything it
//! contains is in `src/app.rs`; `brocanth add commands` turns this into a
//! command line with `serve` as one of its commands.

mod app;{assets}
mod pages;
{plugins}
use std::process::ExitCode;

fn main() -> ExitCode {{
    match app::serve("127.0.0.1:3000") {{
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {{
            eprintln!("error: {{message}}");
            ExitCode::FAILURE
        }}
    }}
}}
"##,
        title = a.title,
        assets = if a.has("rsx-tailwind") || a.has("rsx-htmx") {
            "\nmod assets;"
        } else {
            ""
        },
        plugins = "mod plugins;\n",
    )
}

/// `src/main.rs` when the project has a command line: it is the command line.
fn main_with_commands_rs(a: &Answers) -> String {
    format!(
        r##"//! {title} -- a brocanth application.
//!
//! This binary is the project's command line. `serve` is the one command the
//! scaffold wrote; the rest are yours. A struct with `#[cmd]` under
//! `src/commands/` is a subcommand, registered at link time, with no list to
//! add it to.
//!
//! The wiring lives in `src/app.rs`, so `serve` and anything else that needs
//! the router read the same thing.

mod app;{assets}
mod commands;
mod pages;
{plugins}
fn main() -> brocanth_command::anyhow::Result<()> {{
    brocanth_command::main(brocanth_command::Context::boot()?)
}}
"##,
        title = a.title,
        assets = if a.has("rsx-tailwind") || a.has("rsx-htmx") {
            "\nmod assets;"
        } else {
            ""
        },
        plugins = "mod plugins;\n",
    )
}

/// `src/commands/serve.rs`: the one command a new project gets.
///
/// Written into the project rather than built into the framework, because the
/// command line is the project's and tuning it should not mean forking
/// anything.
const SERVE_RS: &str = r#"//! `serve` -- the one command the scaffold wrote.
//!
//! It is ordinary project code: rename it, give it more flags, or delete it.
//! The framework does not own this command line.

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Serve the application over HTTP.
#[cmd]
#[derive(clap::Args)]
pub struct Serve {
    /// Address to bind.
    #[arg(long, default_value = "127.0.0.1:3000")]
    pub addr: String,
}

impl Command for Serve {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        crate::app::serve(&self.addr).map_err(anyhow::Error::msg)
    }
}
"#;

fn mise_toml(built: &Built) -> String {
    let mut out = String::from(MISE_TOML);
    // The tasks the modules asked for. The slot was declared with the rest
    // and never read, so `rsx-tailwind` wrote its two here by hand and a
    // module from anywhere else had nowhere to write one at all.
    for task in built.slot(Slot::MiseTasks) {
        out.push('\n');
        out.push_str(task);
    }
    out
}

const MISE_TOML: &str = r#"[tools]
# Must match the toolchain brocanth's proc-macro artifacts were built with:
# they are not compatible across patch releases.
rust = "1.98"

# Named for what it does, and not `run`: `run` is one of mise's own
# subcommands, so a task called that is only reachable as `mise run run`.
# The alias keeps a bare `mise run` working, because mise runs the task
# called `default` when it is given no name.
[tasks.serve]
alias = ["default"]
run = "cargo run"

[tasks.test]
run = "cargo test"
"#;

const PAGES_MOD_RS_HEAD: &str = r##"//! The route table, which is this directory.
//!
//! One file is one route, and the path on disk is the URL: `index.rs` is `/`,
//! `about.rs` is `/about`, `users/[id].rs` is `/users/{id}`.
//!
//! Nothing below lists them. The macro reads this directory while the crate
//! is compiled and writes the `mod` lines and [`router`] from what is there,
//! so adding a page is adding a file and renaming one renames its route.
//! The verb comes off the handler's name: `handle` is GET, `handle_post` is
//! POST, and a `pub fn handle_*` that is not a verb is a build error rather
//! than a handler nothing reaches.
//!
//! `brocanth_pages::pages!` and `cargo expand` is how to read the result.

brocanth_pages::pages!("src/pages"@DESCRIBE@);

use brocanth::plugin_context::{Context, ContextItem, Has};
use brocanth::routing::{BuildError, Params, Router, Verb};

use crate::plugins;

/// What a handler produces.
///
/// The framework owns rendering: a handler returns a value and never writes a
/// response itself, so a half-written body on an error is not something that
/// can happen. Echo and Gin hand the handler a writer and take an error back,
/// and both carry the scar -- once a 200 has gone out, the central error
/// handler cannot take it back. Here there is nothing to take back.
pub struct Response {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

/// Written by hand rather than derived, and the difference matters.
///
/// A derived `Default` would give `status: 0`, which is not an HTTP status, so
/// `Response { body: .., ..Default::default() }` would compile and be wrong.
/// 200 is what "I did not say" means for a response.
impl Default for Response {
    fn default() -> Self {
        Self {
            status: 200,
            body: String::new(),
            headers: Vec::new(),
        }
    }
}

impl Response {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            ..Default::default()
        }
    }

    pub fn ok(body: impl Into<String>) -> Self {
        Self::new(200, body)
    }

    pub fn empty(status: u16) -> Self {
        Self::new(status, "")
    }

    /// For what the struct literal does not cover: several headers, added one
    /// at a time. Rust has no variadic generics, so a tuple return is
    /// fixed-arity and this is the unbounded form.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// Drops every header with this name, ignoring case.
    ///
    /// `with_header` appends, because `set-cookie` has to repeat. A
    /// single-valued header does not: two `content-type` lines are two
    /// claims and the client picks. Replacing one drops it first, which
    /// is what `set_header` below does.
    #[allow(dead_code)]
    #[must_use]
    pub fn without_header(mut self, name: &str) -> Self {
        self.headers
            .retain(|(held, _)| !held.eq_ignore_ascii_case(name));
        self
    }

    /// The one value for this header, replacing any already set.
    ///
    /// What to compose with when overriding a type an earlier step chose:
    /// `Ok(render_json(tpl)?.set_header("content-type", "application/problem+json"))`.
    #[allow(dead_code)]
    #[must_use]
    pub fn set_header(self, name: &str, value: impl Into<String>) -> Self {
        self.without_header(name).with_header(name, value)
    }
}

/// What a handler may return instead of building a [`Response`] by hand.
///
/// The Crystal brocanth overloads `write_response` on the handler's return
/// type, so a page returns the simplest thing that says what it means. Rust
/// spells the same idea as a trait: each handler still returns exactly one
/// type, and this is what lets different handlers return different ones.
pub trait IntoResponse {
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

/// A body, answered 200.
impl IntoResponse for String {
    fn into_response(self) -> Response {
        Response::ok(self)
    }
}

impl IntoResponse for &str {
    fn into_response(self) -> Response {
        Response::ok(self)
    }
}

/// Nothing: 200 with no body. Crystal's `Nil` return.
impl IntoResponse for () {
    fn into_response(self) -> Response {
        Response::empty(200)
    }
}

/// A status and nothing else -- `Ok(404)`.
impl IntoResponse for u16 {
    fn into_response(self) -> Response {
        Response::empty(self)
    }
}

/// A body and a status.
impl<T: IntoResponse> IntoResponse for (T, u16) {
    fn into_response(self) -> Response {
        Response {
            status: self.1,
            ..self.0.into_response()
        }
    }
}

/// A body and headers.
impl<T: IntoResponse> IntoResponse for (T, Vec<(String, String)>) {
    fn into_response(self) -> Response {
        let mut response = self.0.into_response();
        response.headers.extend(self.1);
        response
    }
}

/// A body, a status and headers.
impl<T: IntoResponse> IntoResponse for (T, u16, Vec<(String, String)>) {
    fn into_response(self) -> Response {
        let mut response = self.0.into_response();
        response.status = self.1;
        response.headers.extend(self.2);
        response
    }
}

/// What a handler may fail with.
///
/// Deliberately **not** an `impl std::error::Error`, which is what lets the
/// blanket `From` below exist without colliding with the reflexive
/// `impl<T> From<T> for T`. `anyhow::Error` is this shape for the same reason.
/// The effect is that `?` works on anything in a handler.
#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub message: String,
    /// Headers the failure needs. A `content-type` for a body that is not
    /// HTML, a `www-authenticate` for a 401, a `retry-after` for a 429.
    ///
    /// `Response` always had these and `Error` did not, which made a
    /// machine-readable error body impossible to send.
    pub headers: Vec<(String, String)>,
}

impl Error {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            headers: Vec::new(),
        }
    }

    /// The default for a failure nobody classified: it is a bug here, not a
    /// mistake by the caller.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(500, message)
    }

    /// The same, carrying a header.
    ///
    /// `allow(dead_code)` for the same reason the request readers below
    /// have it: it is here for the first handler that needs it, and a
    /// brand-new project has none yet.
    #[allow(dead_code)]
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(error: E) -> Self {
        Self::internal(error.to_string())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        // Everything a client sees for a 5xx is the status. The message is for
        // whoever is reading the logs, and putting it in the body is how an
        // internal path ends up on a stranger's screen.
        let withheld = self.status >= 500;
        let body = if withheld {
            String::from("Internal Server Error")
        } else {
            self.message
        };
        let mut headers = self.headers;
        if withheld {
            // The body is replaced, so a `content-type` describing the
            // original no longer describes anything. Keeping it labelled a
            // plain sentence `application/problem+json` -- harmless while
            // `sent` sent two types and the client could pick the other,
            // and wrong now that the replacement is exact.
            headers.retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
        }
        Response {
            status: self.status,
            body,
            // The rest survive: the body is withheld, not the framing the
            // client needs to read what is left.
            headers,
        }
    }
}

/// What a handler is given. Middleware sees it too, and may change it before
/// the handler runs or after it returns.
pub struct Ctx {
    /// Which verb matched. A layer reads it; a page usually does not,
    /// because the verb is already in the name of the function it is in.
    #[allow(dead_code)]
    pub verb: Verb,
    pub path: String,
    /// Everything after the first `?`, without it. Empty when there was none.
    #[allow(dead_code)]
    pub query: String,
    /// The body as it arrived. Empty for the verbs that carry none, and for
    /// a context built by a test that did not set one.
    #[allow(dead_code)]
    pub body: Vec<u8>,
    params: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    /// What the plugins put here for this request, or `None` for a context
    /// built by hand.
    plugins: Option<Context<plugins::Installed>>,@DEPTH_FIELD@
}

impl Ctx {
    pub fn new(verb: Verb, path: &str, params: Params<'_>) -> Self {
        Self {
            verb,
            path: path.to_string(),
            query: String::new(),
            body: Vec::new(),
            params: params
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            headers: Vec::new(),
            plugins: None,@DEPTH_INIT@
        }
    }

    /// The rest of the request: what the client sent, rather than what the
    /// router matched.
    ///
    /// Two calls and not one, because a test that exercises a handler
    /// usually wants the first and not the second. `src/app.rs` makes both.
    ///
    /// The three are copied out of the server's request, which lives for
    /// the connection and not for the handler. One copy per request, and
    /// the handler owns what it reads.
    #[must_use]
    pub fn sent(mut self, query: &str, body: Vec<u8>, headers: Vec<(String, String)>) -> Self {
        self.query = query.to_string();
        self.body = body;
        self.headers = headers;
        self
    }

    /// What the plugins put on this request's context.
    ///
    /// `src/app.rs` calls it once per request, with a context the
    /// application made from its providers. A `Ctx` built in a test has
    /// none, which is what the `Option` says.
    #[must_use]
    pub fn with_plugins(mut self, plugins: Context<plugins::Installed>) -> Self {
        self.plugins = Some(plugins);
        self
    }

    /// The value a plugin published for this request.
    ///
    /// **Compiles only when some registered plugin provides `T`.** Ask for
    /// a type nothing installs and the build stops and names it; there is
    /// no string key to misspell and no guard to forget. That check is the
    /// whole point, and it is the compiler's, not this method's.
    ///
    /// `Option` is the other half, and it is not about `T`'s type. It is
    /// `None` in two cases: a `Ctx` built by hand in a test, which has no
    /// application behind it and so no providers; and a provider that
    /// declared `T` and inserted nothing, which `Plugin::provide` allows
    /// because it is one method and `Provides` is a separate declaration.
    /// A dispatched request whose provider wrote its value gets `Some`.
    ///
    /// The `_` is the position of `T` in the provider list, which is
    /// always inferred. It is there so two impls do not overlap, and it is
    /// the same `_` `Context::get` takes.
    ///
    /// ```ignore
    /// let session = ctx.provided::<Session, _>().expect("dispatched");
    /// ```
    #[allow(dead_code)]
    pub fn provided<T, I>(&self) -> Option<&T>
    where
        T: ContextItem,
        plugins::Installed: Has<T, I>,
    {
        // `and_then` with `try_get`, not `map` with `get`: `get` panics
        // when the provider list claims `T` and the provider inserted
        // nothing, which the generated plugin templates make easy --
        // `Provides` is unconditional and `provide` is often
        // `if let Some(..)`. A method returning `Option` must not abort
        // the process to say no.
        self.plugins.as_ref().and_then(|cx| cx.try_get::<T>())
    }@DEPTH_IMPL@

    /// A route parameter, or `None`.
    ///
    /// Never `""`. The Crystal prototype returned an empty string for a name
    /// nobody declared, so a typo read as a present-but-blank value and went
    /// unnoticed; `Option` is that bug deleted rather than documented.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The same, for a parameter the route declares and so must be there.
    ///
    /// Returns `Result` rather than `Option` so it composes with `?`. A name
    /// the route does not declare is this file disagreeing with its own
    /// filename, which is a 500 and not a 400: nobody sent a bad request.
    pub fn require(&self, name: &str) -> Result<&str, Error> {
        self.param(name)
            .ok_or_else(|| Error::internal(format!("the route declares no `{name}` parameter")))
    }
}

/// What the client sent, as opposed to what the router matched.
///
/// A second block, and `allow(dead_code)`, for one reason: the request is
/// on the context whether or not this project reads it yet. The other way
/// round is a context whose shape depends on which modules were chosen,
/// and then a page that starts reading a header needs the generator rather
/// than a line.
#[allow(dead_code)]
impl Ctx {
    /// A request header by name, case-insensitively, or `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Every request header, in the order they arrived.
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// What the body is written in, or `None` when the client did not say.
    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    /// The body as text, or `None` when it is not UTF-8.
    ///
    /// `Option` and not a lossy conversion: a body that is not text is a
    /// request to answer, not a string to repair.
    pub fn body_text(&self) -> Option<&str> {
        core::str::from_utf8(&self.body).ok()
    }
}

/// A handler, as the table stores it. One per verb a page answers.
///
/// Boxed rather than a bare `fn` pointer, because every handler returns its
/// own type and a `fn` pointer can only hold one of them. [`page`] is what
/// unifies them, and it runs once per route at startup rather than per
/// request.
pub type Handler = Box<dyn Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync>;

/// Puts a handler in the table whatever it returns.
///
/// Generic over the return type, so `-> Result<impl IntoResponse, Error>` on
/// the handler and one concrete `Handler` in the table are the same thing.
pub fn page<H, T>(handler: H) -> Handler
where
    H: Fn(&mut Ctx) -> Result<T, Error> + Send + Sync + 'static,
    T: IntoResponse,
{
    Box::new(move |ctx| handler(ctx).map(IntoResponse::into_response))
}

"##;

/// How many layout layers the client says it holds, from `DP-Partial`.
///
/// Written only with partial navigation; without it every request takes the
/// first arm of `html!` and this would be a function that always returns
/// zero.
const HOLDING_FN: &str = r#"/// How many layout layers the client already has, from `DP-Partial`.
///
/// `html!` leaves out that many layouts, counting from the root, so a
/// navigation inside a section sends the section's contents and not the
/// document around them. dp-swap sends the number; the header is absent on
/// a first load, for a crawler, and whenever JavaScript is off, and then
/// this is zero and the whole document goes out.
///
/// `true` is dp-swap's word for "a fragment, I did not count", which is one
/// layer. Anything that does not parse is zero: a client that cannot say
/// what it holds gets everything, which is always correct and only slower.
fn holding(request: &brocanth_server::Request) -> u32 {
    match request.header("dp-partial") {
        None => 0,
        Some("true") => 1,
        Some(value) => value.parse().unwrap_or(0),
    }
}

"#;

/// A page's answer, as the server sends it. Always written: both shapes of
/// `serve` go through it, so one place turns a `Response` into bytes.
const SENT_FN: &str = r#"/// Turns a page's answer into the bytes the server writes.
///
/// `html` rather than a guess: most pages return markup, and a handler that
/// wants another type says so with a `content-type` header.
///
/// That header replaces the seeded one rather than joining it. `with_header`
/// appends, because `set-cookie` has to repeat, so a page answering JSON
/// used to put two `content-type` lines on the wire and let the client pick.
fn sent(answered: Response) -> brocanth_server::Response {
    let mut out = brocanth_server::Response::html(answered.status, answered.body);
    if answered
        .headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        out = out.without_header("content-type");
    }
    for (name, value) in &answered.headers {
        out = out.with_header(name, value);
    }
    out
}

"#;

/// The static-file half of `serve`, written only when there are assets.
///
/// Not a page, and not routed. A page returns a `Response` whose body is a
/// `String`, which a font is not, so a static file answers before the router
/// is consulted and never touches the page contract.
const STATIC_FN: &str = r##"/// The file a request named, or `None` to let the router answer.
///
/// Checked before the route table, which is also the order of surprise: a
/// page at the same path as a file would be shadowed, and a path under
/// `/assets` or `/public` is not a route anybody writes by hand.
fn serve_asset(request: &brocanth_server::Request) -> Option<brocanth_server::Response> {
    if !matches!(
        request.verb,
        brocanth::routing::Verb::Get | brocanth::routing::Verb::Head
    ) {
        return None;
    }
    let asset = crate::assets::lookup(&request.path)?;

    // The ETag is the content hash, so it is strong and a match means the
    // bytes are identical. 304 is the whole point of serving `public/`
    // without a fingerprint in the name.
    if request.header("if-none-match") == Some(asset.etag) {
        return Some(
            brocanth_server::Response::new(304, asset.content_type, &[][..])
                .with_header("etag", asset.etag)
                .with_header("cache-control", asset.cache_control()),
        );
    }

    Some(
        brocanth_server::Response::new(200, asset.content_type, asset.body)
            .with_header("etag", asset.etag)
            .with_header("cache-control", asset.cache_control()),
    )
}

"##;

const PAGES_MOD_RS_TESTS: &str = r##"#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_the_shape_of_the_directory() {
        let router = router().expect("the pages produce a table");
        let mut patterns: Vec<&str> = router
            .routes()
            .iter()
            .map(|(template, _)| template.pattern())
            .collect();
        patterns.sort();
        assert_eq!(@ROUTES@);
    }

    #[test]
    fn a_path_that_exists_with_a_verb_that_does_not_is_a_405() {
        let router = router().expect("the pages produce a table");
        let allow = router.allow_for("/about").expect("/about is a route");
        assert_eq!(allow.to_string(), "GET, HEAD");
    }
}
"##;

/// Whether this line of a `register!` chain registers something.
///
/// Text, because the slot holds text. A commented line does not count:
/// `brocanth-middleware` writes five of them on purpose, as guidance at
/// the place they would be uncommented.
fn registers(line: &str) -> bool {
    let line = line.trim_start();
    !line.starts_with("//") && line.starts_with("let builder")
}

/// The header and body of the generated `Hooks` alias.
///
/// `NoHooks`, and it is not a placeholder. The record widens only through
/// `AppBuilder::plugin_hooking`, and no module in this CLI registers a
/// hooking plugin: `brocanth-middleware` writes five, all commented out,
/// at the place somebody uncomments one. The day a module registers one
/// for real, this becomes `Hooked<ThatPlugin, NoHooks>` and is built the
/// way `Installed` is, one alias per hooking plugin.
///
/// It is written down rather than inferred for the reason `Installed` is:
/// `App` is a struct and a struct field needs a type to name.
const HOOK_RECORD_DOC: &str = "\
/// Which hook families this binary installed, as a type.
///
/// `NoHooks` while nothing registers with `plugin_hooking`. A hook nobody
/// registers costs nothing, and this type is how the compiler knows:
/// every hook entry point opens with a constant `if` on this record.
pub type Hooks = ::brocanth::plugin_hooks::NoHooks;
";

/// `my-store` -> `MyStore`, for a type name built from a module key.
fn upper_camel(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut head = true;
    for ch in key.chars() {
        if ch == '-' || ch == '_' {
            head = true;
        } else if head {
            out.extend(ch.to_uppercase());
            head = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// The opening of the generated `pages::mounted`.
///
/// `pages!("src/pages", mounts)` calls this and does not write it, so the
/// generator owns the body and a module owns one line of it.
const MOUNTED_HEAD: &str = "\
/// Every route this project did not read off `src/pages`.
///
/// One table holds all of them. A module contributes routes by offering a
/// `mount` and the application decides the prefix, which is the same rule
/// the core states for a plugin: a path is a decision, so a module that
/// mounted itself would be making one on the application's behalf.
///
/// A prefix two sources both claim is a `BuildError` naming the pattern,
/// rather than whichever of them happened to run last.
fn mounted(
    routes: &mut ::brocanth::routing::RouterBuilder<Handler>,
) -> Result<(), ::brocanth::routing::BuildError> {
";

/// The header over the generated `Installed` alias.
///
/// A const rather than a `\n\`-continued literal: that form keeps this
/// file's own indentation and writes it into the generated one, which has
/// already happened twice.
const PROVIDER_LIST_DOC: &str = "\
/// What this binary installs on a request's context, as a type.
///
/// One `Install` per registered plugin, innermost first, which is exactly
/// what `register!` builds. The two have to be the same type: if they
/// drift this file does not compile, which is why the list is written
/// once and both readers derive from it.
///
/// A plugin that puts nothing on the context folds away, because
/// `NoContext::Install<E>` is `E`, so this is `Base` in a project whose
/// plugins all provide nothing.
";

/// `src/pages/mod.rs`, in three parts so the rsx bridge can land before the
/// test module rather than after it, which clippy refuses.
fn pages_mod_rs(a: &Answers, built: &Built) -> String {
    // The partial-navigation members exist only where something reads them.
    // `html!` is the only reader, and it is written only with templates, so
    // without them these would be a field nobody sets and a method nobody
    // calls -- two warnings in a brand-new project.
    let (field, init, methods) = if a.has("rsx") {
        (
            "\n    /// How many layout layers the client says it already holds.\n\
             \x20   ///\n\
             \x20   /// Zero unless a request asked for less, so a first load, a crawler\n\
             \x20   /// and a reader with JavaScript off all get the whole document.\n\
             \x20   partial: u32,",
            "\n            partial: 0,",
            "\n\n\
             \x20   /// What `html!` asks, to leave out the layouts the client has.\n\
             \x20   ///\n\
             \x20   /// A method rather than a public field so the number has one\n\
             \x20   /// meaning: layers to skip, counted from the root.\n\
             \x20   pub fn partial_depth(&self) -> u32 {\n\
             \x20       self.partial\n\
             \x20   }",
        )
    } else {
        ("", "", "")
    };
    // `holding` is the setter and only partial navigation ever calls it.
    let setter = if a.has("rsx-htmx") {
        "\n\n\
         \x20   /// The same, for a request that said how much of the page it has.\n\
         \x20   #[must_use]\n\
         \x20   pub fn holding(mut self, layers: u32) -> Self {\n\
         \x20       self.partial = layers;\n\
         \x20       self\n\
         \x20   }"
    } else {
        ""
    };
    let mut out = String::from(PAGES_MOD_RS_HEAD);
    // The renderers: one `pub mod` per template engine, each wrapping its
    // own `load_dir!`. Read from a slot rather than written here, because
    // which engine makes a page is the engine's own statement -- `rsx` was
    // a literal in this function until there was a second one.
    for block in built.slot(Slot::PagesRenderers) {
        out.push_str(block);
    }
    // The other two sources of the one route table. A module that
    // contributes routes writes a line here; a declarative routes file
    // writes one too. The pages are already in the builder by the time
    // this runs, so a path claimed twice is a `BuildError` naming both --
    // which is the property that makes one table worth having.
    if !built.slot_is_empty(Slot::RouterMounts) {
        // One blank line above it, whatever the renderer block ended with.
        // No golden shape mounted anything until `admin`, and the first
        // one that did showed `mounted` glued to `render_json`.
        while out.ends_with("\n\n") {
            out.pop();
        }
        out.push('\n');
        out.push_str(MOUNTED_HEAD);
        for line in built.slot(Slot::RouterMounts) {
            out.push_str(line);
        }
        out.push_str("    Ok(())\n}\n\n");
    }

    // What the chosen modules add to the pages: another `impl Ctx`, a
    // helper every handler shares. Before the tests, because an item after
    // a `mod tests` block is what clippy calls `items_after_test_module`.
    for block in built.slot(Slot::PagesItems) {
        // One blank line before each, whatever the piece above ended with.
        while out.ends_with("\n\n") {
            out.pop();
        }
        out.push('\n');
        out.push_str(block.trim_start_matches('\n'));
    }
    if !built.slot_is_empty(Slot::PagesItems) {
        while out.ends_with("\n\n") {
            out.pop();
        }
        out.push('\n');
    }
    out.push_str(PAGES_MOD_RS_TESTS);
    // `describe` makes the macro emit what every route takes and gives,
    // which names `brocanth_input` and `zod`. A project that chose
    // neither must not be made to depend on them by a macro.
    let describe = if a.has("input") { ", describe" } else { "" };
    // The second word, and only when something mounts. `mounted` is a
    // function the macro calls and does not write, so a project with
    // nothing to mount must not be told to call it.
    let mounts = if built.slot_is_empty(Slot::RouterMounts) {
        ""
    } else {
        ", mounts"
    };

    // Every route this project has, including the ones a module's own page
    // brought. The list is written down because a route is a file and the
    // directory is the only other record of it; a module that writes a page
    // says so here, or the test it is not allowed to edit starts failing.
    let mut routes: Vec<String> = ["/", "/about", "/users/{id}"]
        .iter()
        .map(|r| (*r).to_string())
        .chain(
            built
                .slot(Slot::PagesRoutes)
                .map(|r| r.trim_end().to_string()),
        )
        .collect();
    routes.sort();
    routes.dedup();
    let quoted: Vec<String> = routes.iter().map(|r| format!("{r:?}")).collect();
    // One line while it fits, and rustfmt's own broken form when it does
    // not. `every_generated_rust_file_is_already_formatted` rejects the
    // long line, and a generated project nobody can `cargo fmt --check` is
    // a generated project with a chore in it.
    // rustfmt breaks a macro call on `fn_call_width` (60) applied to the
    // ARGUMENT LIST, not on `max_width` (100) applied to the line. This
    // read 100 and so emitted a one-liner for any argument list between
    // the two, which `--with openapi` lands in exactly: a brand-new
    // project that fails `cargo fmt --check` on its first day.
    let inner = quoted.join(", ");
    let args = format!("patterns, [{inner}]");
    let routes = if args.len() <= 60 {
        args
    } else if inner.len() <= 60 {
        // The call breaks and the array does not. rustfmt measures an array
        // on `array_width` (60) applied to what is between the brackets, so
        // a list 61 to 72 wide is two arguments on two lines with the array
        // whole. `--with admin` was the first shape to land here.
        format!("\n            patterns,\n            [{inner}]\n        ")
    } else {
        let items = quoted
            .iter()
            .map(|r| format!("                {r}"))
            .collect::<Vec<_>>()
            .join(",\n");
        format!("\n            patterns,\n            [\n{items}\n            ]\n        ")
    };

    out.replace("@ROUTES@", &routes)
        .replace("@DEPTH_FIELD@", field)
        .replace("@DEPTH_INIT@", init)
        .replace("@DEPTH_IMPL@", &format!("{setter}{methods}"))
        .replace("@DESCRIBE@", &format!("{describe}{mounts}"))
}

fn index_rs(a: &Answers) -> String {
    if a.has("rsx") {
        return String::from(
            r##"//! `/` -- because the file is called `index.rs`.

use crate::pages::{html, views, Ctx, Error, IntoResponse, TITLE};

/// GET. The framework reads the verb off the function name: `handle` is GET
/// and `handle_post` is POST.
///
/// The whole body is the bridge. `html` takes a component and gives back this
/// handler's own `Result`, so there is no `Ok(..)` to write and a template
/// that bails is propagated rather than swallowed.
///
/// The markup is in `src/pages/index.rsx`, beside this file and compiled into
/// the binary: nothing is read from disk when a request arrives, and a
/// template that does not parse is a compile error on the `.rsx` line.
///
/// Nothing here names a layout. `html!` reads them off the directory tree:
/// this file is in `src/pages`, so it is wrapped in `src/pages/layout.rsx`,
/// and a page under `src/pages/users/` is wrapped in that one too.
pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    html!(ctx, views::index::Index { title: TITLE })
}
"##,
        );
    }
    format!(
        r##"//! `/` -- because the file is called `index.rs`.

use crate::pages::{{Ctx, Error, IntoResponse}};

/// GET. The framework reads the verb off the function name: `handle` is GET,
/// `handle_post` is POST, and `handle_json` would be a content-negotiation hook
/// rather than a verb.
///
/// The return type is the simplest thing that says what this page means. A
/// `&str` is a 200 with that body; see `users/[id].rs` for the other shapes.
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {{
    Ok("<!doctype html><title>{title}</title>\
         <h1>{title}</h1>\
         <p>This file is <code>src/pages/index.rs</code>. Rename it and the route \
         moves with it.</p>\
         <ul><li><a href=\"/about\">/about</a></li>\
         <li><a href=\"/users/7\">/users/7</a></li></ul>")
}}
"##,
        title = a.title
    )
}

fn about_rs(a: &Answers) -> String {
    if a.has("rsx") {
        return String::from(
            r##"//! `/about` -- one file, one route.

use crate::pages::{html, views, Ctx, Error, IntoResponse, TITLE};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    // The context is the request, and a component's parameters are an
    // ordinary struct: whatever the page needs, named, and type-checked
    // against `src/pages/about.rsx` beside this file. A field the component
    // does not declare is a compile error, not a blank in the output.
    let verb = ctx.verb.to_string();
    html!(
        ctx,
        views::about::About {
            title: TITLE,
            verb: &verb,
            path: &ctx.path,
        }
    )
}
"##,
        );
    }
    format!(
        r##"//! `/about` -- one file, one route.

use crate::pages::{{Ctx, Error, IntoResponse, Response}};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {{
    // The context is the request. The framework owns the response, so a
    // handler returns a value and never writes one itself -- which is what
    // makes a half-written body on an error impossible rather than unlikely.
    //
    // The struct literal is the form that scales: name the fields that differ
    // and `..Default::default()` fills the rest. Rust will not let you omit a
    // field without it, and there are no implicit zero values.
    Ok(Response {{
        body: format!(
            "<!doctype html><title>About \u{{b7}} {title}</title>\
             <h1>About</h1>\
             <p>You asked for <code>{{}} {{}}</code>. This page answers GET and \
             nothing else, so a DELETE to it is a 405 carrying an \
             <code>Allow</code> header rather than a 404.</p>",
            ctx.verb, ctx.path
        ),
        ..Default::default()
    }})
}}
"##,
        title = a.title
    )
}

/// `src/pages/users/[id].rs`, which is where the scaffold shows that not
/// every response is a page: the POST is a redirect and no template is
/// involved.
fn user_rs(a: &Answers) -> String {
    if a.has("rsx") {
        return String::from(USER_RSX_RS);
    }
    String::from(USER_RS)
}

const USER_RSX_RS: &str = r##"//! `/users/{id}` -- the brackets in the file name are the parameter.

use crate::pages::{html, routes, views, Ctx, Error, IntoResponse, Response};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    // `?` is why a handler returns `Result`. Anything that fails converts
    // into `Error` through a blanket `From` -- a parse here, and a template
    // that bails inside `html`, by the same route.
    let id: u64 = ctx.require("id")?.parse()?;

    html!(ctx, views::users::_id_::User { id })
}

/// POST to the same path. One file, one route, one handler per verb.
///
/// Not every response is a page, and this is the reason `html` is a function
/// rather than something the framework does to every return: a redirect is a
/// status and one header, with no markup anywhere near it.
pub fn handle_post(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let id: u64 = ctx.require("id")?.parse()?;

    Ok(Response::empty(303).with_header("location", routes::users::id::path(id)))
}
"##;

const USER_RS: &str = r##"//! `/users/{id}` -- the brackets in the file name are the parameter.
//!
//! There is no ceiling on how many a route may have. The Crystal prototype had
//! `MAX_PARAMS = 4` and dropped the fifth without saying so, because its
//! collector could not prove a captured parameter does not outlive the request.
//! Here the parameters borrow the path, so the question does not arise.

use crate::pages::{routes, Ctx, Error, IntoResponse, Response};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    // `?` is why a handler returns `Result`. Anything that fails converts
    // into `Error` through a blanket `From`, so a parse, a query or a file
    // read is one character rather than a match arm.
    let id: u64 = ctx.require("id")?.parse()?;

    Ok(Response {
        body: format!("<!doctype html><title>User {id}</title><h1>User {id}</h1>"),
        ..Default::default()
    })
}

/// POST to the same path. One file, one route, one handler per verb.
///
/// A redirect is a status, one header and no body. The struct literal above is
/// the other form; this one stays readable when there are several headers,
/// because Rust has no variadic generics and a tuple return is therefore
/// fixed-arity.
///
/// The location is built rather than written. `routes` is generated from the
/// directory alongside the table, so a link to a route that does not exist
/// does not compile, and neither does one missing a parameter.
pub fn handle_post(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let id: u64 = ctx.require("id")?.parse()?;

    Ok(Response::empty(303).with_header("location", routes::users::id::path(id)))
}
"##;

// -- the views ---------------------------------------------------------------

/// The root layout: the document every page is written into.
///
/// A layout is not a special kind of thing here. It is a component with a
/// parameter of type `Element`, which is what the page below it fills.
/// Four spaces on every line that has anything on it.
///
/// The slots were written for the depth they used to land at, in
/// `src/app.rs`. They land one block deeper now, inside a macro body, and
/// re-indenting here is one function rather than a width every module has
/// to know.
fn deeper(text: &str) -> String {
    text.lines()
        .map(|line| {
            if line.trim().is_empty() {
                line.to_string()
            } else {
                format!("    {line}")
            }
        })
        .map(|line| line + "\n")
        .collect()
}

/// `src/plugins/mod.rs`: the `mod` lines, and the four things `src/app.rs`
/// asks this file for.
///
/// Written for every project, including one with no plugins at all. That is
/// the point of it: `src/app.rs` says the same four sentences whatever the
/// module list is, so `brocanth add` never has to edit the one file the
/// developer owns. Four small functions cost a project nothing, and the
/// alternative costs it the ability to add a plugin later.
fn plugins_mod_rs(a: &Answers, built: &Built) -> String {
    let mut out = String::from(
        "//! The plugins this application contains, and the four things `src/app.rs`\n\
         //! asks this file for.\n\
         //!\n\
         //! Each plugin is also named in `register!`: being here makes it compile,\n\
         //! being named there makes it run. The core depends on none of them, and the\n\
         //! dependency arrow points this way and never back.\n\
         //!\n\
         //! The registration chain sits here and not in `src/app.rs` because the\n\
         //! generator rewrites it every time the module list changes, and `src/app.rs`\n\
         //! is yours.\n\
         \n",
    );
    // Sorted, not in the order the modules ran. rustfmt orders `mod`
    // declarations alphabetically, and a scaffold that hands a developer a
    // diff on their first `cargo fmt` is a scaffold that got it wrong.
    let mut mods: Vec<&str> = built.slot(Slot::PluginsMod).collect();
    mods.sort_unstable();
    for line in &mods {
        out.push_str(line);
    }
    if !mods.is_empty() {
        out.push('\n');
    }
    // `After` only appears when something registered. Six of the twelve
    // golden shapes have no plugin at all, `--core-only` among them, and
    // each warned `unused import` on its very first build.
    out.push_str(if built.slot_is_empty(Slot::PluginsInstalled) {
        "use brocanth::plugin_context::Base;\n"
    } else {
        "use brocanth::plugin_context::{After, Base};\n"
    });
    out.push_str(concat!(
        "use brocanth::routing::Next;\n",
        "\n",
        "use crate::pages::{Ctx, Response};\n",
        "\n",
    ));

    // -- register -----------------------------------------------------
    out.push_str(
        "/// Every plugin this application contains, in dependency order.\n\
         ///\n\
         /// A macro and not a function, and the reason is the builder's type:\n\
         /// registering a plugin that provides a context item widens\n\
         /// `AppBuilder`'s type-level provider list, so the type this chain ends\n\
         /// on is one nested associated type per plugin. A function would have to\n\
         /// spell that type out and re-spell it on every change. The macro expands\n\
         /// at its one call site in `src/app.rs`, where the compiler works it out\n\
         /// and nobody has to read it.\n\
         ///\n\
         /// Each registration is still one line, and a line a reader can delete.\n\
         macro_rules! register {\n\
        \x20   ($builder:expr) => {{\n",
    );
    // Whatever the chain needs in scope. These are `use` lines for crates in
    // the extern prelude, so they mean the same thing here and at the macro's
    // expansion site. Sorted, because rustfmt sorts them.
    let mut uses: Vec<String> = built.slot(Slot::AppUses).map(str::to_string).collect();
    if a.has("config") {
        uses.push("use brocanth_config::{AppBuilderConfigExt, Loader};\n".to_string());
    }
    uses.sort();
    for line in &uses {
        out.push_str(&deeper(&deeper(line)));
    }
    if !uses.is_empty() {
        out.push('\n');
    }
    // The whole chain is `let builder = builder..` one registration at a
    // time, and the last line hands it back. A project with no plugins has
    // neither, because `let builder = $builder; builder` is a rebinding
    // that says nothing.
    let registrations: Vec<&str> = built.slot(Slot::AppPlugins).collect();
    let head = if registrations.is_empty() {
        "$builder"
    } else {
        "let builder = $builder"
    };
    if a.has("config") {
        out.push_str(&format!(
            "        // The core takes already-built values; finding the files, layering\n\
            \x20       // them and typing each section is `brocanth-config`'s job, and it is\n\
            \x20       // the only reason a TOML parser is in the tree.\n\
            \x20       let config = Loader::new(crate::app::APP)\n\
            \x20           .load()\n\
            \x20           .map_err(|e| e.to_string())?;\n\
            \x20       {head}.with_config(config){end}\n",
            end = if registrations.is_empty() { "" } else { ";" },
        ));
    } else {
        out.push_str(&format!(
            "        // No file lookup, no merge order, no TOML parser: the core takes\n\
            \x20       // values that are already built, so each plugin below is handed a\n\
            \x20       // literal the compiler checks. Add the config plugin when a file is\n\
            \x20       // wanted; nothing else changes.\n\
            \x20       {head}{end}\n",
            end = if registrations.is_empty() { "" } else { ";" },
        ));
    }
    for line in &registrations {
        out.push_str(line);
    }
    if !registrations.is_empty() {
        out.push_str("        builder\n");
    }
    out.push_str("    }};\n}\npub(crate) use register;\n\n");

    // -- the provider list, as a type ---------------------------------
    //
    // `register!` is a macro because the builder's type is not worth
    // spelling. `App` is a struct, and a struct field needs a type to name,
    // so the one piece of that type anything else needs is written out
    // here: the list of what is on the context.
    //
    // Built from the same slot order the chain above uses, which is why
    // `snippet::plugin` writes both. A plugin that provides nothing folds
    // away, because `NoContext::Install<E>` is `E`.
    // One alias per plugin rather than one nested type, so every line is
    // short. The nested form is a single 200-character type by the third
    // plugin, and rustfmt wraps it in a shape a generator would have to
    // reimplement to stay byte-identical.
    out.push_str(PROVIDER_LIST_DOC);
    let mut held = String::from("Base");
    for line in built.slot(Slot::PluginsInstalled) {
        // `cache crate::plugins::cache::Cache`. The key leads because the
        // alias is named after it: a key is unique by construction and a
        // type name is not, so two modules whose plugin types share a
        // final segment used to emit one `type WithCache` twice.
        let (key, path) = line
            .trim_end()
            .split_once(' ')
            .unwrap_or(("", line.trim_end()));
        let name = format!("With{}", upper_camel(key));
        out.push_str(&format!("type {name} = After<{path}, {held}>;\n"));
        held = name;
    }
    out.push_str(&format!("pub type Installed = {held};\n\n"));
    out.push_str(HOOK_RECORD_DOC);

    // -- announce -----------------------------------------------------
    out.push_str(
        "/// What this binary puts on the context, said once at startup.\n\
         ///\n\
         /// The lines are a demonstration and they are load-bearing: the installed\n\
         /// provider set lives in the application's *type*, so an `extract` for a\n\
         /// plugin nobody registered does not compile. That is the modularity claim,\n\
         /// checked by the compiler rather than written in a README.\n\
         ///\n\
         /// A handler reads the same values through `Ctx::provided`, which carries\n\
         /// the same check. This runs at startup so a misconfiguration is a failure\n\
         /// to start rather than a failure on the first request that needed it.\n\
         ///\n\
         /// A macro for the reason `register!` is one: the context's type is the\n\
         /// builder's type, and neither is worth spelling.\n\
         macro_rules! announce {\n\
        \x20   ($app:expr) => {{\n",
    );
    if built.slot_is_empty(Slot::AppContext) {
        out.push_str(
            "        // Nothing in this binary puts a value on the context.\n\
            \x20       let _ = $app;\n",
        );
    } else {
        out.push_str("        let cx = $app.context();\n");
        for line in built.slot(Slot::AppContext) {
            out.push_str(&deeper(line));
        }
    }
    out.push_str("    }};\n}\npub(crate) use announce;\n\n");

    // -- layer --------------------------------------------------------
    out.push_str(
        "/// One middleware layer, as the stack stores it.\n\
         ///\n\
         /// A name rather than the type written out: clippy calls the bare\n\
         /// form a very complex type, and it is also the type `src/app.rs`\n\
         /// would have to spell to hold one.\n\
         pub type Layer = fn(&mut Ctx, Next<'_, Ctx, Response>) -> Response;\n\
         \n\
         /// The function behind a middleware name the application mounted.\n\
         ///\n\
         /// Mounting is `src/app.rs`'s decision and stays there: the pattern is a\n\
         /// decision and the decision is the application's. Which function answers\n\
         /// to `\"request_log\"` is not a decision, so it is here, where adding a\n\
         /// plugin can write it.\n\
         pub fn layer(name: &str) -> Option<Layer> {\n",
    );
    if built.slot_is_empty(Slot::PluginsLayers) {
        out.push_str(
            "    // Nothing in this binary offers middleware, so nothing can be\n\
            \x20   // mounted and nothing is ever looked up.\n\
            \x20   let _ = name;\n\
            \x20   None\n",
        );
    } else {
        out.push_str("    match name {\n");
        let mut arms: Vec<&str> = built.slot(Slot::PluginsLayers).collect();
        arms.sort_unstable();
        for arm in arms {
            out.push_str(arm);
        }
        out.push_str("        _ => None,\n    }\n");
    }
    out.push_str("}\n");

    // -- the grace period ---------------------------------------------
    //
    // Here and not in `src/app.rs`, because where it comes from varies with
    // the module list: a file with the config plugin, the constant without.
    out.push('\n');
    if a.has("config") {
        out.push_str(&GRACE_FROM_CONFIG.replace("@UPPER@", &a.ident().to_uppercase()));
    } else {
        out.push_str(GRACE_CONST);
    }
    out
}

/// `plugins::shutdown_grace` with the config plugin.
const GRACE_FROM_CONFIG: &str = r#"/// `[server] shutdown_grace_secs`, or
/// [`SHUTDOWN_GRACE`](crate::app::SHUTDOWN_GRACE) when it is not set.
pub fn shutdown_grace() -> Result<std::time::Duration, String> {
    let config = brocanth_config::Loader::new(crate::app::APP)
        .load()
        .map_err(|e| e.to_string())?;
    let Some(value) = config
        .raw_section("server")
        .and_then(|server| server.get("shutdown_grace_secs"))
    else {
        return Ok(crate::app::SHUTDOWN_GRACE);
    };
    // An integer in a file, and a string in `$@UPPER@_SERVER__SHUTDOWN_GRACE_SECS`.
    value
        .as_integer()
        .and_then(|secs| u64::try_from(secs).ok())
        .or_else(|| value.as_str().and_then(|secs| secs.trim().parse().ok()))
        .map(std::time::Duration::from_secs)
        .ok_or_else(|| {
            format!(
                "`[server] shutdown_grace_secs` is {}, and it must be a whole number of seconds",
                value.display()
            )
        })
}
"#;

/// `plugins::shutdown_grace` without it.
const GRACE_CONST: &str = r#"/// How long a stop waits: [`SHUTDOWN_GRACE`](crate::app::SHUTDOWN_GRACE).
///
/// This project has no configuration file, so the constant is the answer.
/// `brocanth add config` makes it `[server] shutdown_grace_secs`.
pub fn shutdown_grace() -> Result<std::time::Duration, String> {
    Ok(crate::app::SHUTDOWN_GRACE)
}
"#;

fn config_toml(a: &Answers, built: &Built) -> String {
    let mut out = format!(
        "# The configuration of this environment.\n\
         #\n\
         # The framework owns `[app]` and `[server]`. Every other section is claimed by\n\
         # the plugin instance registered under that name in src/main.rs, and a section\n\
         # nothing claims stops the build rather than being silently ignored -- a\n\
         # misspelt section must never build clean.\n\
         #\n\
         # Layered, highest precedence first: ${upper}_CONFIG, /etc/{ident}/{env}.toml,\n\
         # ./{ident}_{env}.toml, ./config/{env}.toml. ${upper}_ENV picks the\n\
         # environment; without it, {env}.\n\
         \n\
         [app]\n\
         name = {title:?}\n\
         \n\
         [server]\n\
         # Seconds a stop (SIGTERM or Ctrl-C) waits for open requests, then for the\n\
         # plugins. Keep it below the orchestrator's kill timeout: Kubernetes waits 30.\n\
         shutdown_grace_secs = 25\n",
        upper = a.ident().to_uppercase(),
        ident = a.ident(),
        env = brocanth_config::DEFAULT_ENV,
        title = a.title,
    );
    for section in built.slot(Slot::ConfigSections) {
        out.push_str(section);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(picks: &[&'static str]) -> Answers {
        Answers {
            name: "my-blog".into(),
            title: "My Blog".into(),
            picks: picks.to_vec(),
            local: None,
            git: false,
        }
    }

    /// Every shape the generator can be asked for, named.
    ///
    /// Not every combination -- there are 256 and most differ in nothing --
    /// but every one that exercises a branch: the empty project, the full
    /// one, the default, and each way the rsx family can be asked for.
    fn shapes() -> Vec<(&'static str, Vec<&'static str>)> {
        vec![
            ("core-only", vec![]),
            // A plugin and no config file. Its own shape because the
            // literal branch of every plugin module runs only here, and
            // for as long as it was unsnapshotted it did not compile.
            ("plugin-no-config", vec!["logging"]),
            ("default", DEFAULT.to_vec()),
            ("everything", available()),
            // A plugin that builds everything it provides in `configure`,
            // and the one module whose settings carry a string the plugin
            // has to translate. No config file either, so the literal
            // branch of both is here.
            ("realtime", vec!["realtime"]),
            // The queue, with a config file, which is the shape that opens
            // a pool and starts a worker from a TOML section.
            ("jobs", vec!["config", "jobs"]),
            // The two modules that mount routes, and nothing else: no
            // config file and no renderer, so the admin's own rsx is the
            // only template engine in the tree and `mounted` is written
            // with no other block around it.
            ("admin", vec!["admin", "resource"]),
            ("rsx", vec!["rsx"]),
            // The second renderer, alone: a project that answers JSON and
            // has no HTML engine at all. The shape that would catch
            // anything in `src/pages/mod.rs` that only works because rsx
            // happened to be there too.
            ("jt", vec!["jt"]),
            // And the two together, which is the claim the renderer seam
            // makes: a page tree holds `index.rsx` beside `status.jt`, and
            // each engine compiles its own extension out of the same tree.
            ("jt-rsx", vec!["jt", "rsx"]),
            // `commands` and not because this shape is about a command
            // line: the CSS build is a subcommand, so tailwind cannot be
            // written without one. `rsx-tailwind` declares it.
            ("rsx-tailwind", vec!["rsx", "rsx-tailwind", "commands"]),
            ("rsx-htmx", vec!["rsx", "rsx-htmx"]),
            (
                "rsx-tailwind-htmx",
                vec!["rsx", "rsx-tailwind", "rsx-htmx", "commands"],
            ),
        ]
    }

    /// One shape as one string: every file it writes, in order, with its path
    /// above it.
    ///
    /// One file per shape rather than a directory per shape, because the
    /// thing a reader does with this is read a diff, and a diff of one file
    /// shows a line moving between two generated files as one change.
    fn snapshot(picks: &[&'static str]) -> String {
        let mut out = String::new();
        for (path, contents) in written(picks) {
            out.push_str(&format!("===== {path} =====\n{contents}"));
            if !contents.ends_with('\n') {
                out.push('\n');
            }
        }
        out
    }

    /// Every Rust file this generator writes, through rustfmt.
    ///
    /// A scaffolded project must be clean on its own first `cargo fmt`, or
    /// the generator is handing a developer a diff they did not write. Two
    /// bugs that lived in shapes nobody had generated by hand -- a doubled
    /// blank line, and `mod` declarations out of alphabetical order -- are
    /// why this is a test and not a habit.
    ///
    /// Skipped, loudly, where rustfmt is absent, because a check that
    /// silently does nothing is worse than no check.
    #[test]
    fn every_generated_rust_file_is_already_formatted() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        if Command::new("rustfmt").arg("--version").output().is_err() {
            eprintln!("skipped: rustfmt is not on PATH");
            return;
        }
        let mut unformatted = Vec::new();
        for (name, picks) in shapes() {
            for (path, source) in written(&picks) {
                if !path.ends_with(".rs") {
                    continue;
                }
                // `--emit stdout` and not `--check`: `--check` prints a diff
                // and exits 0, so an *empty* stdout is the clean signal --
                // and a file rustfmt could not parse also has an empty
                // stdout. A generated file with a stray brace in it read as
                // perfectly formatted for exactly as long as that was the
                // test. Reformatting and comparing has no such hole.
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
                if !done.status.success() {
                    unformatted.push(format!("{name}: {path} (rustfmt could not read it)"));
                    continue;
                }
                // Some rustfmt builds label the stream with a `<stdin>:`
                // line first; this one does not. Both are handled, because
                // the test must not start passing because it stopped
                // reading the output.
                let formatted = String::from_utf8_lossy(&done.stdout);
                let formatted = formatted
                    .strip_prefix("<stdin>:\n")
                    .unwrap_or(formatted.as_ref());
                if formatted != source {
                    unformatted.push(format!("{name}: {path}"));
                }
            }
        }
        assert!(
            unformatted.is_empty(),
            "rustfmt rejects these:\n  {}",
            unformatted.join("\n  ")
        );
    }

    #[test]
    fn the_formatting_check_notices_a_file_that_does_not_parse() {
        // The test above is only worth having if it fails on the input it
        // used to pass, so this pins the two facts it now rests on. A test
        // that merely passes is not evidence that it would fail.
        use std::io::Write;
        use std::process::{Command, Stdio};

        if Command::new("rustfmt").arg("--version").output().is_err() {
            eprintln!("skipped: rustfmt is not on PATH");
            return;
        }
        const BROKEN: &str = "fn main() { let x = 1; }}\n";

        // One: `--check` is the invocation that cannot tell. It says
        // nothing on stdout for a file it cannot parse, which is exactly
        // what it says for a file that is already formatted.
        let checked = rustfmt(&["--edition", "2021", "--check"], BROKEN);
        assert!(
            checked.1.is_empty(),
            "this is why an empty stdout was not enough to tell"
        );

        // Two: `--emit stdout` is the invocation that can. It fails, and
        // the status is what says so.
        let emitted = rustfmt(&["--edition", "2021", "--emit", "stdout"], BROKEN);
        assert!(
            !emitted.0,
            "rustfmt must fail on input it cannot parse, or the check above \
             is reading a success as a clean file"
        );

        fn rustfmt(args: &[&str], input: &str) -> (bool, String) {
            let mut child = Command::new("rustfmt")
                .args(args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("rustfmt runs");
            child
                .stdin
                .take()
                .expect("a pipe")
                .write_all(input.as_bytes())
                .expect("rustfmt reads");
            let done = child.wait_with_output().expect("rustfmt finishes");
            (
                done.status.success(),
                String::from_utf8_lossy(&done.stdout).into_owned(),
            )
        }
    }

    /// The whole generator, held still.
    ///
    /// Every refactor of this file is a refactor of what other people's
    /// projects are made of, and the only honest test of one is that the
    /// bytes did not move. Rewrite the snapshots with:
    ///
    /// ```text
    /// BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- generator_writes
    /// ```
    ///
    /// Read the diff before committing it. A blessed snapshot nobody read is
    /// a test that asserts whatever the bug does.
    /// What the chosen modules contributed, for a test that reads a file
    /// the generator assembles from slots. `Built::new()` is the empty
    /// scaffold and reads as "no module wrote anything", which since the
    /// renderers moved out of this file is a project with no templates.
    fn built_for(a: &Answers) -> Built {
        contributions(a).expect("these picks are a project")
    }

    #[test]
    fn a_registration_without_a_provider_list_entry_is_refused_by_name() {
        // What `snippet::plugin` exists to prevent. Writing only the
        // chain line gives a generated `src/app.rs` that fails on
        // `App { context }` with a nested associated type naming nothing.
        use brocanth_scaffold::Scaffold;
        let mut built = Scaffold::new();
        built.now_writing("stripe");
        built
            .at(Slot::AppPlugins)
            .block("        let builder = builder.plugin(Stripe::new(s));\n");

        let listed: Vec<&str> = built.slot_owners(Slot::PluginsInstalled).collect();
        let unpaired: Vec<&str> = built
            .slot_owners(Slot::AppPlugins)
            .zip(built.slot(Slot::AppPlugins))
            .filter(|(_, block)| block.lines().any(registers))
            .map(|(owner, _)| owner)
            .filter(|owner| !listed.contains(owner))
            .collect();
        assert_eq!(unpaired, ["stripe"]);
    }

    #[test]
    fn commented_guidance_in_the_chain_is_not_a_registration() {
        // `brocanth-middleware` writes five commented registrations at
        // the place they would be uncommented, and must not be reported.
        assert!(registers(
            "        let builder = builder.plugin(X::new(s));"
        ));
        assert!(!registers(
            "        // let builder = builder.plugin(X::new(s));"
        ));
        assert!(!registers("        // Five edge hooks, none of them on."));
    }

    #[test]
    fn an_alias_is_named_after_the_key_because_two_types_may_share_a_name() {
        // `crate::plugins::cache::Cache` and a third-party
        // `crate::plugins::billing::Cache` both end in `Cache`, and the
        // generated project had two `type WithCache` lines.
        assert_eq!(upper_camel("cache"), "Cache");
        assert_eq!(upper_camel("billing"), "Billing");
        assert_eq!(upper_camel("file-storage"), "FileStorage");
        assert_eq!(upper_camel("admin_model"), "AdminModel");
    }

    #[test]
    fn the_route_assertion_is_emitted_in_the_shape_rustfmt_wants() {
        // The threshold read `max_width` and rustfmt breaks a macro call
        // on the width of its arguments, so every route list between the
        // two was emitted one-line and reformatted. `--with openapi` is
        // in that window, and shipped a project failing `cargo fmt`.
        for picks in [vec!["openapi", "input", "config"], vec!["jt"], vec![]] {
            let a = answers(&picks);
            let pages = pages_mod_rs(&a, &built_for(&a));
            let line = pages
                .lines()
                .find(|l| l.contains("assert_eq!(patterns,"))
                .unwrap_or("");
            assert!(
                line.is_empty() || line.len() <= 72,
                "one-line form too wide for rustfmt: {} chars\n{line}",
                line.len()
            );
        }
    }

    #[test]
    fn a_module_that_mounts_routes_reaches_the_one_table() {
        // The seam a declarative routes file and a route bundle both use.
        // Nothing in the menu writes it yet, so the test writes the line a
        // module would and reads the file the generator makes.
        let a = answers(&["rsx"]);
        let mut built = contributions(&a).expect("a project");
        built.now_writing("records");
        built
            .at(Slot::RouterMounts)
            .line("    crate::plugins::records::mount(routes, \"/api\")?;");

        let pages = pages_mod_rs(&a, &built);
        assert!(
            pages.contains("fn mounted(") && pages.contains("records::mount(routes, \"/api\")"),
            "the module's line did not reach `mounted`:\n{pages}"
        );
        // And the macro is told to call it. Without the word the function
        // would be written and never run, which is the failure this pair
        // of changes exists to make impossible.
        assert!(
            pages.contains("pages!(\"src/pages\", mounts)"),
            "the macro was not told to call `mounted`:\n{pages}"
        );
    }

    #[test]
    fn a_project_that_mounts_nothing_is_not_told_to_call_mounted() {
        // `mounted` is a function the macro calls and does not write, so a
        // project with no mounts must not be asked for one.
        let a = answers(&["rsx"]);
        let pages = pages_mod_rs(&a, &built_for(&a));
        assert!(!pages.contains("fn mounted("), "{pages}");
        assert!(!pages.contains(", mounts"), "{pages}");
    }

    #[test]
    fn the_generator_writes_what_it_wrote_yesterday() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
        let bless = std::env::var_os("BROCANTH_BLESS").is_some();
        if bless {
            std::fs::create_dir_all(&dir).expect("golden directory");
        }
        let mut wrong = Vec::new();
        for (name, picks) in shapes() {
            let have = snapshot(&picks);
            let file = dir.join(format!("{name}.txt"));
            if bless {
                std::fs::write(&file, &have).expect("write snapshot");
                continue;
            }
            match std::fs::read_to_string(&file) {
                Ok(want) if want == have => {}
                Ok(want) => wrong.push(format!(
                    "{name}: {} lines expected, {} written; first difference at line {}",
                    want.lines().count(),
                    have.lines().count(),
                    want.lines()
                        .zip(have.lines())
                        .position(|(a, b)| a != b)
                        .map_or_else(|| "the end".to_string(), |n| (n + 1).to_string())
                )),
                Err(e) => wrong.push(format!("{name}: {} ({e})", file.display())),
            }
        }
        assert!(
            wrong.is_empty(),
            "the generator changed:\n  {}\n\nRun with BROCANTH_BLESS=1 and read the diff.",
            wrong.join("\n  ")
        );
    }

    /// Everything a shape writes. The modules have to agree for a project
    /// to exist at all, so a test that cannot get this far has already
    /// found the bug.
    fn written(picks: &[&'static str]) -> Vec<(String, String)> {
        scaffold(&answers(picks)).expect("the modules agree")
    }

    /// The view a module is given, for a test that calls one directly.
    ///
    /// Leaked because `Project` borrows its key list and a test wants one
    /// expression; the leak is a few bytes in a process that is about to
    /// end.
    fn project(picks: &'static [&'static str]) -> brocanth_scaffold::Project<'static> {
        let known: &'static [&'static str] = Box::leak(all().into_boxed_slice());
        brocanth_scaffold::Project::new("my-blog", "My Blog", picks, known)
    }

    /// What the modules contributed, for a generator that takes it.
    fn built(picks: &[&'static str]) -> Built {
        contributions(&answers(picks)).expect("the modules agree")
    }

    fn paths(picks: &[&'static str]) -> Vec<String> {
        written(picks).into_iter().map(|(p, _)| p).collect()
    }

    #[test]
    fn a_name_is_a_cargo_package_name() {
        assert!(check_name("my-blog").is_ok());
        assert!(check_name("blog_2").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("My Blog").is_err());
        assert!(check_name("../escape").is_err());
        assert!(check_name("2blog").is_err());
    }

    #[test]
    fn the_default_title_is_a_starting_point() {
        assert_eq!(crate::ui::title_case("my-blog"), "My Blog");
        assert_eq!(crate::ui::title_case("brocanth"), "Brocanth");
        assert_eq!(crate::ui::title_case("a_b-c"), "A B C");
    }

    #[test]
    fn the_core_alone_writes_no_plugin_and_no_config() {
        let paths = paths(&[]);
        // `src/plugins/mod.rs` is there, with three empty answers in it, so
        // that `src/app.rs` reads the same whatever was chosen. What must
        // not be there is a plugin.
        assert!(
            !paths
                .iter()
                .any(|p| p.starts_with("src/plugins/") && p != "src/plugins/mod.rs"),
            "{paths:?}"
        );
        assert!(!paths.iter().any(|p| p.starts_with("config/")), "{paths:?}");
        // The route table is not optional: it is the core.
        assert!(paths.contains(&"src/pages/mod.rs".to_string()));
    }

    #[test]
    fn a_plugin_writes_its_file_and_nobody_elses() {
        // A directory, not a file: `remove` deletes a directory, and a
        // developer who adds a second file beside the first moves nothing.
        let paths = paths(&["logging"]);
        assert!(paths.contains(&"src/plugins/logging/mod.rs".to_string()));
        assert!(!paths.iter().any(|p| p == "src/plugins/logging.rs"));
        assert!(!paths.iter().any(|p| p.starts_with("src/plugins/sessions")));
        assert!(!paths.iter().any(|p| p.starts_with("src/plugins/model")));
    }

    #[test]
    fn the_manifest_holds_exactly_what_was_chosen() {
        // The §8 claim, as an assertion rather than a sentence in a README.
        // Dependency *lines*, not mentions: the comment above `[dependencies]`
        // names every crate that is absent, which is the point of it.
        let core = cargo_toml(&answers(&[]), &built(&[]));
        let declared = |manifest: &str, name: &str| {
            manifest
                .lines()
                .any(|l| l.starts_with(&format!("{name} = ")))
        };
        assert!(declared(&core, "brocanth"), "{core}");
        assert!(!declared(&core, "serde"), "{core}");
        assert!(!declared(&core, "brocanth-config"), "{core}");
        assert!(
            !declared(&core, "brocanth-model"),
            "an ORM nobody asked for: {core}"
        );

        // Logging alone still links nothing new: its settings are a literal.
        let logging = cargo_toml(&answers(&["logging"]), &built(&["logging"]));
        assert!(!declared(&logging, "serde"), "{logging}");
        assert!(!declared(&logging, "brocanth-config"), "{logging}");

        let configured = cargo_toml(
            &answers(&["logging", "config"]),
            &built(&["logging", "config"]),
        );
        assert!(declared(&configured, "brocanth-config"), "{configured}");
        assert!(declared(&configured, "serde"), "{configured}");

        let with_model = cargo_toml(&answers(&["model"]), &built(&["model"]));
        assert!(declared(&with_model, "brocanth-model"), "{with_model}");
    }

    #[test]
    fn the_manifest_says_what_is_absent_and_why() {
        let core = cargo_toml(&answers(&[]), &built(&[]));
        assert!(
            core.contains("Not here, because nothing asked for it"),
            "{core}"
        );
        assert!(
            core.contains("no sea-orm anywhere in the lock file"),
            "{core}"
        );
        let everything = cargo_toml(&answers(&available()), &built(&available()));
        assert!(
            !everything.contains("Not here, because"),
            "nothing is absent, so nothing is listed: {everything}"
        );
    }

    #[test]
    fn the_config_plugin_is_what_puts_a_deserialiser_in_the_source() {
        // "A feature that is off must mean the code is absent, not that a
        // branch returns early." Two scaffolds, and the difference is a derive.
        let configured = crate::modules::logging::plugin_rs(&project(&["logging", "config"]));
        assert!(configured.contains("serde::Deserialize"), "{configured}");
        assert!(configured.contains("deny_unknown_fields"), "{configured}");

        let literal = crate::modules::logging::plugin_rs(&project(&["logging"]));
        assert!(!literal.contains("serde"), "{literal}");
    }

    #[test]
    fn app_names_every_plugin_and_mounts_what_no_plugin_may() {
        let picks = ["logging", "sessions", "config"];
        let a = answers(&picks);
        // Every registration is named, and named in the generated file, not
        // in `src/app.rs`: that one is the developer's and `brocanth add`
        // must never have to write into it.
        let registry = plugins_mod_rs(&a, &built(&picks));
        for line in [
            ".plugin_from_config::<crate::plugins::logging::Logging>(\"logging\")",
            ".plugin_from_config::<crate::plugins::sessions::Sessions>(\"sessions\")",
        ] {
            assert!(registry.contains(line), "{registry}");
        }
        let main = app_rs(&a, &built(&picks));
        assert!(!main.contains("plugin_from_config"), "{main}");
        // The mount is in the application's setup block, never in the plugin
        // and never in the generated file: a pattern is a decision.
        assert!(
            main.contains("setup.router().mount(\"/{*rest}\", \"request_log\")"),
            "{main}"
        );
        assert!(!registry.contains(".mount("), "{registry}");
        let plugin = crate::modules::logging::plugin_rs(&project(&["logging"]));
        assert!(
            !plugin.contains(".mount("),
            "a plugin must not mount: {plugin}"
        );
        assert!(plugin.contains("catalog.offer("), "{plugin}");
    }

    #[test]
    fn a_core_only_app_reads_no_file_and_registers_nothing() {
        let registry = plugins_mod_rs(&answers(&[]), &built(&[]));
        assert!(!registry.contains("Loader::new"), "{registry}");
        // `.plugin(` and not `.plugin`: `App` has a `plugins` field now, and
        // an assertion that matches it is an assertion about nothing.
        assert!(!registry.contains(".plugin("), "{registry}");
        assert!(!registry.contains(".plugin_from_config"), "{registry}");
        assert!(!registry.contains("brocanth_config"), "{registry}");
        // And no `pub mod` line, so the directory holds this file alone.
        assert!(!registry.contains("pub mod "), "{registry}");
    }

    #[test]
    fn src_app_rs_says_the_same_thing_whatever_was_chosen() {
        // The whole of why the three slots moved. `brocanth add` rewrites
        // what the generator owns; `src/app.rs` is the developer's, so a
        // module set it varies with is a module nobody can add later.
        //
        // The mounts are the exception and they are the point: a pattern is
        // a decision, so it is in the file where decisions go.
        let strip = |picks: &[&'static str]| {
            app_rs(&answers(picks), &built(picks))
                .lines()
                .filter(|l| !l.contains("setup.router().mount("))
                .filter(|l| !l.contains("Nothing in this binary offers middleware"))
                .filter(|l| !l.contains("let _ = setup.router();"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let core = strip(&[]);
        for picks in [
            vec!["logging"],
            vec!["config", "logging", "sessions", "model"],
            vec!["config", "jobs"],
            vec!["realtime"],
        ] {
            assert_eq!(strip(&picks), core, "src/app.rs moved for {picks:?}");
        }
    }

    #[test]
    fn without_the_config_plugin_a_plugin_is_built_from_a_literal() {
        let registry = plugins_mod_rs(&answers(&["logging"]), &built(&["logging"]));
        // Already wrapped, because it is longer than rustfmt's chain_width
        // and a project must survive its own first `cargo fmt`.
        assert!(
            registry.contains(".plugin(crate::plugins::logging::Logging::new(\n")
                && registry.contains("crate::plugins::logging::Settings::default(),"),
            "{registry}"
        );
        assert!(!registry.contains("plugin_from_config"), "{registry}");
        // `Plugin::new` is a trait method, so this branch -- and only this
        // branch -- needs the trait in scope.
        assert!(
            registry.contains("use brocanth::plugin_api::Plugin;"),
            "{registry}"
        );
    }

    #[test]
    fn the_generated_config_names_only_sections_a_plugin_claims() {
        // `validate_sections` rejects a section nothing claims, so a scaffold
        // that wrote `[sessions]` without the sessions plugin would fail on its
        // own first run. The real crate is asked, not a reimplementation.
        for picks in [vec!["config"], vec!["config", "logging"], available()] {
            let a = answers(&picks);
            let toml = config_toml(&a, &built(&picks));
            let config = brocanth_config::Config::from_toml(a.name.as_str(), "development", &toml)
                .unwrap_or_else(|e| panic!("{picks:?}: {e}\n{toml}"));
            // The instances the generated `main` registers: one per plugin,
            // plus `config` itself, which `with_config` adds.
            let mut claimed: Vec<String> = instances(&built(&picks))
                .iter()
                .map(|p| (*p).to_string())
                .collect();
            claimed.push("config".to_string());
            config
                .validate_sections(&claimed)
                .unwrap_or_else(|e| panic!("{picks:?}: {e}\n{toml}"));
        }
    }

    #[test]
    fn the_chooser_reports_a_line_that_would_have_produced_the_same_answers() {
        // What `--debug` prints after the chooser closes. Every answer the
        // chooser collects has a flag, and this is that sentence made usable.
        let mut answers = answers(&["logging", "commands"]);
        answers.name = "blog".to_string();
        answers.title = "Blog".to_string();
        answers.git = true;

        let line = answers.as_flags();
        assert_eq!(
            line,
            "brocanth create blog --title Blog --with logging,commands --git"
        );
    }

    #[test]
    fn a_title_with_a_space_survives_being_pasted_into_a_shell() {
        let mut answers = answers(&[]);
        answers.name = "my_app".to_string();
        answers.title = "My App".to_string();
        answers.git = false;

        // `--core-only` rather than an empty `--with`, because an empty list
        // is a flag that does nothing and reads like a mistake.
        assert_eq!(
            answers.as_flags(),
            "brocanth create my_app --title 'My App' --core-only --no-git"
        );
    }

    #[test]
    fn a_quote_in_a_title_cannot_break_out_of_the_quoting() {
        let mut answers = answers(&[]);
        answers.name = "x".to_string();
        answers.title = "it's".to_string();
        answers.git = false;
        assert!(
            answers.as_flags().contains(r"'it'\''s'"),
            "{}",
            answers.as_flags()
        );
    }

    #[test]
    fn the_agent_brief_names_every_capability_this_project_does_not_have() {
        // The load-bearing half of AGENTS.md. A model that cannot see what is
        // absent assumes the framework has it, writes against it, and gets a
        // compile error it then tries to fix by guessing harder. Naming the
        // gap is what turns that into "add the dependency first".
        let only_logging = written(&["logging"]);
        let brief = &only_logging
            .iter()
            .find(|(p, _)| p == "AGENTS.md")
            .expect("every project gets an agent brief")
            .1;

        assert!(brief.contains("`logging`"), "{brief}");
        for absent in ["`config`", "`sessions`", "`model`", "`commands`"] {
            assert!(
                brief.contains(absent),
                "{absent} is absent from the binary and unnamed in the brief:\n{brief}"
            );
        }

        // And the opposite end: with everything chosen there is nothing to
        // warn about, and the brief must not invent a gap.
        let everything = written(&available());
        let full = &everything
            .iter()
            .find(|(p, _)| p == "AGENTS.md")
            .expect("an agent brief")
            .1;
        assert!(
            full.contains("Nothing. Every capability was chosen."),
            "{full}"
        );
    }

    #[test]
    fn the_table_is_the_directory_and_names_nothing() {
        // There is no list of routes to go out of step, because there is no
        // list: `pages!` reads the directory. So what this checks is that
        // nothing crept back in.
        let a = answers(&available());
        let table = pages_mod_rs(&a, &built_for(&a));
        // `mounts` too: `admin` and `resource` each offer a `mount`, so the
        // table has a second source and the macro is told to call it.
        assert!(
            table.contains(r#"brocanth_pages::pages!("src/pages", describe, mounts);"#),
            "{table}"
        );
        // Code, not prose: the comment beside the templates explains why the
        // generated `pub mod index` has to be nested, and says the words.
        let code: Vec<&str> = table
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect();
        assert!(
            !code.iter().any(|l| l.contains("routes.file(")),
            "a row was written: {table}"
        );
        assert!(
            !code.iter().any(|l| l.trim() == "pub mod index;"),
            "a mod was written: {table}"
        );

        // And the files it will find are the ones the scaffold writes.
        let files = scaffold(&a).expect("the modules agree");
        for page in ["index.rs", "about.rs", "users/[id].rs"] {
            assert!(
                files.iter().any(|(p, _)| p == &format!("src/pages/{page}")),
                "{page} is not written"
            );
        }
    }

    #[test]
    fn the_route_of_every_generated_page_is_what_the_framework_says_it_is() {
        use brocanth::routing::route_for_file;
        for (file, expected) in [
            ("index.rs", "/"),
            ("about.rs", "/about"),
            ("users/[id].rs", "/users/{id}"),
        ] {
            assert_eq!(route_for_file(file, "").unwrap().pattern(), expected);
        }
    }

    #[test]
    fn without_rsx_nothing_names_a_template() {
        let plain = paths(&["logging", "config", "commands"]);
        assert!(
            !plain.iter().any(|p| p.ends_with(".rsx")),
            "a template nobody asked for: {plain:?}"
        );
        assert!(!plain.iter().any(|p| p.contains("layout")));

        let picks: &[&'static str] = &["logging", "config", "commands"];
        let a = answers(picks);
        assert!(
            !cargo_toml(&a, &built(picks))
                .lines()
                .any(|l| l.starts_with("rsx = ")),
            "no template engine in the tree"
        );
        // And the bridge is not in the source either, so there is no function
        // to call and no `rsx::` path to copy from.
        assert!(!pages_mod_rs(&a, &built_for(&a)).contains("pub fn html("));
        assert!(!main_with_commands_rs(&a).contains("mod views;"));
    }

    #[test]
    fn a_template_sits_beside_the_page_it_renders() {
        // The Crystal page_router's layout: the markup for a route is the
        // file next to it, so there is no mirror tree to keep in step.
        let files = paths(&["rsx"]);
        for page in ["index", "about"] {
            assert!(files.contains(&format!("src/pages/{page}.rs")), "{files:?}");
            assert!(
                files.contains(&format!("src/pages/{page}.rsx")),
                "{page}.rs has no {page}.rsx beside it: {files:?}"
            );
        }
        // Including down a directory, where the file name is not an
        // identifier and the two still match.
        assert!(files.contains(&"src/pages/users/[id].rs".to_string()));
        assert!(files.contains(&"src/pages/users/[id].rsx".to_string()));
        // And the layouts, which are templates with no route of their own.
        assert!(files.contains(&"src/pages/layout.rsx".to_string()));
        assert!(files.contains(&"src/pages/users/layout.rsx".to_string()));
        assert!(
            !files.iter().any(|p| p.starts_with("src/views/")),
            "nothing lives in a parallel tree: {files:?}"
        );

        // Every page reaches the template beside it, under the one module
        // the macro writes.
        let a = answers(&["rsx"]);
        for (page, view) in [
            (index_rs(&a), "views::index::Index"),
            (about_rs(&a), "views::about::About"),
            (user_rs(&a), "views::users::_id_::User"),
        ] {
            assert!(page.contains(view), "{page}");
        }
    }

    #[test]
    fn the_templates_get_a_module_that_cannot_collide_with_the_pages() {
        // `load_dir!` emits `pub mod index` for `index.rsx` and `pages!`
        // emits one for `index.rs`. Both land in this file, so one of them
        // has to be nested or the scaffold does not compile.
        let pages = pages_mod_rs(&answers(&["rsx"]), &built_for(&answers(&["rsx"])));
        assert!(
            pages.contains("pub mod views {\n    rsx::load_dir!(\"src/pages\");\n}"),
            "{pages}"
        );
        // Both macros read the same directory, so both would write
        // `pub mod index` into the same file. One of them is nested.
        assert!(
            pages.contains("brocanth_pages::pages!(\"src/pages\");"),
            "{pages}"
        );
    }

    #[test]
    fn nothing_the_scaffold_writes_names_a_layout() {
        // The Crystal page_router's rule: which layout wraps a page is a
        // property of where the page is. `html!` reads that off the
        // directory tree, so a chain written down anywhere would be a chain
        // that can disagree with the tree.
        // The templates' half of this is in `modules::rsx`, beside the
        // templates. This half is the handlers, which are written here.
        let a = answers(&["rsx"]);
        for page in [index_rs(&a), about_rs(&a), user_rs(&a)] {
            assert!(
                !page.contains("Layout"),
                "a handler names a layout:\n{page}"
            );
        }
        let pages = pages_mod_rs(&a, &built_for(&a));
        assert!(
            !pages.contains("layout::Layout"),
            "the chain is written down somewhere: {pages}"
        );

        // Every page goes through the same import and the same call,
        // whichever directory it is in and however deep the nesting there.
        assert!(pages.contains("pub use brocanth_pages::html;"), "{pages}");
        for page in [index_rs(&a), about_rs(&a), user_rs(&a)] {
            assert!(page.contains("use crate::pages::{html, "), "{page}");
            // On one line or wrapped over several, which is rustfmt's call
            // and not this test's: what matters is that the context is the
            // first thing handed to the macro.
            let call = page.split("html!(").nth(1).unwrap_or_default();
            assert!(
                call.trim_start().starts_with("ctx,"),
                "the context is not passed: {page}"
            );
        }
    }

    #[test]
    fn tailwind_and_htmx_are_part_of_rsx_and_the_flags_say_so() {
        // The chooser cannot produce an orphan -- it hides a child whose
        // parent is off -- so this is only reachable from the flags, where
        // it is nearly always a forgotten word.
        let bad = orphans(&["rsx-tailwind"]).expect_err("tailwind alone is not a project");
        let text = format!("{bad:?}");
        assert!(text.contains("part of"), "{text}");
        assert!(
            text.contains("--with rsx,rsx-tailwind"),
            "the fix is the message: {text}"
        );

        orphans(&["rsx", "rsx-tailwind", "rsx-htmx"]).expect("together they are fine");
        orphans(&DEFAULT).expect("the default set has no children in it");
    }

    #[test]
    fn a_second_binary_is_a_section_only_where_cargo_cannot_find_it() {
        // Two shapes, and the difference is not cosmetic: a `[[bin]]` for a
        // file cargo already discovered is the duplicate-name error, and no
        // `[[bin]]` for one it cannot see is a file that never compiles.
        let a = answers(&[]);

        let mut found = Built::new();
        found.now_writing("admin");
        found.binary("admin", "src/bin/admin.rs");
        let manifest = cargo_toml(&a, &found);
        assert!(!manifest.contains("[[bin]]"), "cargo finds it: {manifest}");
        assert!(
            manifest.contains("default-run = \"my-blog\""),
            "two binaries and no default: {manifest}"
        );

        let mut hidden = Built::new();
        hidden.now_writing("admin");
        hidden.binary("admin", "src/admin/main.rs");
        let manifest = cargo_toml(&a, &hidden);
        assert!(
            manifest.contains("[[bin]]\nname = \"admin\"\npath = \"src/admin/main.rs\""),
            "cargo cannot see it: {manifest}"
        );

        // And nothing at all when no module asked for one.
        assert!(!cargo_toml(&a, &built(&[])).contains("[[bin]]"));
        assert!(!cargo_toml(&a, &built(&[])).contains("default-run"));
    }

    #[test]
    fn a_pick_that_cannot_stand_alone_names_what_it_needs() {
        // `rsx-tailwind` writes `src/commands/css.rs`, so a project with no
        // command line has nowhere to put it. The flat command line has to
        // say that rather than write a project that does not compile.
        let bad = unmet(&["rsx", "rsx-tailwind"]).expect_err("tailwind needs a command line");
        let text = format!("{bad:?}");
        assert!(text.contains("rsx-tailwind"), "{text}");
        assert!(text.contains("commands"), "{text}");

        unmet(&["rsx", "rsx-tailwind", "commands"]).expect("together they are fine");
        unmet(&DEFAULT).expect("the default shape stands on its own");
        for (name, picks) in shapes() {
            unmet(&picks).unwrap_or_else(|_| panic!("the `{name}` shape is not a project"));
        }
    }

    #[test]
    fn every_generated_toml_line_starts_in_the_first_column() {
        // A stray indent in an assembled string is valid TOML and still
        // wrong: the reader cannot tell a continuation from a mistake.
        for a in [answers(&["rsx", "rsx-tailwind", "commands"]), answers(&[])] {
            let built = contributions(&a).expect("a project");
            for line in mise_toml(&built).lines() {
                assert_eq!(line, line.trim_start(), "indented: {line:?}");
            }
        }
    }

    #[test]
    fn a_module_this_build_cannot_make_is_refused_before_anything_is_written() {
        // Twice over, because there are two front doors and a third way in
        // for a test. The flags refuse by name with the reason; the module
        // itself refuses if it is somehow reached anyway, so nothing can
        // write half a project.
        let flagged = unavailable(&["realtime", "realtime-sse"])
            .expect_err("a transport this build has no core support for");
        assert_eq!(flagged.len(), 1);

        let reached = contributions(&answers(&["realtime", "realtime-sse"]))
            .expect_err("the module refuses even when it is called");
        assert!(
            format!("{reached:?}").contains("hook contract"),
            "the refusal does not say why: {reached:?}"
        );
    }

    #[test]
    fn nothing_the_menu_cannot_make_is_listed_as_merely_absent() {
        // The receipt means "you did not ask for this". A module nobody
        // could have asked for does not belong in it, in either file.
        let core = cargo_toml(&answers(&[]), &built(&[]));
        let brief = agents_md(&answers(&[]));
        for key in all() {
            if about(key).blocked.is_none() {
                continue;
            }
            assert!(
                !core.contains(key),
                "{key} is in the manifest's absent list"
            );
            assert!(!brief.contains(key), "{key} is in the brief's absent list");
        }
    }

    #[test]
    fn the_absent_column_keeps_a_gap_whatever_a_module_is_called() {
        // The column was a hardcoded 17 until a crate was named in exactly
        // seventeen characters, and then the receipt read
        // `brocanth-realtimeno broker`. A contributed module can be called
        // anything, so the width is measured.
        for picks in [vec![], vec!["realtime"], DEFAULT.to_vec()] {
            let manifest = cargo_toml(&answers(&picks), &built(&picks));
            for line in manifest.lines().filter(|l| l.starts_with("#   ")) {
                let reason = &line[4..];
                let name = reason.split_whitespace().next().unwrap_or_default();
                assert!(
                    reason[name.len()..].starts_with("  "),
                    "no gap after the name: {line:?}"
                );
            }
        }
    }

    #[test]
    fn the_flags_add_a_choice_and_never_imply_one() {
        // `--with rsx` is rsx. The chooser is where tailwind and htmx come
        // on by themselves, because there they are on the screen and can be
        // unticked; a flag that quietly brings two more cannot be read.
        assert!(!DEFAULT.contains(&"rsx-tailwind"));
        assert!(!DEFAULT.contains(&"rsx-htmx"));
        let args = CreateArgs {
            name: Some("blog".into()),
            title: None,
            with: vec!["rsx".to_string()],
            core_only: false,
            brocanth_path: None,
            git: false,
            no_git: true,
            force: false,
        };
        assert_eq!(picked(&args), vec!["rsx"]);
    }

    #[test]
    fn a_link_is_built_rather_than_spelled() {
        // The one URL the scaffold writes by hand was the redirect. It is
        // the typed helper now, so the generated project demonstrates the
        // thing rather than only documenting it.
        for page in [user_rs(&answers(&[])), user_rs(&answers(&["rsx"]))] {
            assert!(page.contains("routes::users::id::path(id)"), "{page}");
            assert!(!page.contains(r#"format!("/users/{id}")"#), "{page}");
        }
    }

    #[test]
    fn the_macros_that_read_the_tree_are_not_a_choice() {
        // Every project has a route table and the table is the directory,
        // so the macro that reads it is in every manifest. It is the one
        // dependency 8 does not ask about, because a proc macro runs at
        // build time and puts no bytes in the binary.
        let declared = |picks: &[&'static str], name: &str| {
            cargo_toml(&answers(picks), &built(picks))
                .lines()
                .any(|l| l.starts_with(&format!("{name} = ")))
        };
        assert!(declared(&["rsx"], "brocanth-pages"));
        assert!(declared(&["logging", "config"], "brocanth-pages"));
        assert!(declared(&[], "brocanth-pages"));
    }

    #[test]
    fn a_page_can_opt_out_of_every_layout() {
        // A fragment -- an htmx swap, a partial -- is not a document, and
        // wrapping one in a shell would be wrong rather than merely verbose.
        let pages = pages_mod_rs(&answers(&["rsx"]), &built_for(&answers(&["rsx"])));
        assert!(
            pages.contains("pub fn render(content: impl rsx::Template)"),
            "{pages}"
        );
    }

    #[test]
    fn a_handler_reaches_a_template_through_one_function_and_no_other() {
        let a = answers(&["rsx"]);
        let pages = pages_mod_rs(&a, &built_for(&a));
        assert!(pages.contains("pub use brocanth_pages::html;"));
        assert!(pages.contains("pub fn render(content: impl rsx::Template)"));
        // It returns the handler's own Result, which is what lets a template
        // that bails propagate instead of becoming a blank 200.
        assert!(pages.contains("-> Result<Response, Error>"), "{pages}");

        // So a page body is the call, with no `Ok(` wrapping it: that is the
        // whole of the ergonomics claim, and it is checked rather than
        // described.
        let index = index_rs(&a);
        assert!(
            index.contains("    html!(ctx, views::index::Index { title: TITLE })"),
            "{index}"
        );
        // One place spells the application's name, and the layout reads it.
        assert!(
            pages_mod_rs(&a, &built_for(&a))
                .contains(&format!("pub const TITLE: &str = {:?};", a.title)),
            "the title is not declared once"
        );
        assert!(!index.contains("Ok(html("), "{index}");

        // And not every response goes through it. The redirect does not.
        let user = user_rs(&a);
        assert!(user.contains("Response::empty(303)"), "{user}");
    }

    #[test]
    fn the_bridge_lands_before_the_test_module() {
        // `items_after_test_module` is a clippy error, and the three-part
        // assembly of this file exists for no other reason.
        let pages = pages_mod_rs(&answers(&["rsx"]), &built_for(&answers(&["rsx"])));
        let html = pages
            .find("pub use brocanth_pages::html;")
            .expect("the bridge is written");
        let tests = pages.find("#[cfg(test)]").expect("the file has tests");
        assert!(html < tests, "the bridge is after the test module");
    }

    #[test]
    fn rsx_is_fetched_rather_than_pathed_even_with_a_local_checkout() {
        // `--brocanth-path` points into the brocanth checkout and rsx is not
        // in it, so a path would be wrong in a way cargo reports late.
        let mut a = answers(&["rsx"]);
        a.local = Some("/somewhere/brocanth/crates/brocanth".to_string());
        let manifest = cargo_toml(&a, &built(&["rsx"]));
        let line = manifest
            .lines()
            .find(|l| l.starts_with("rsx = "))
            .unwrap_or_else(|| panic!("{manifest}"));
        assert!(line.contains("git = "), "{line}");
        assert!(!line.contains("path = "), "{line}");
    }

    /// The scaffold, built for real.
    ///
    /// Ignored because it runs cargo and wants the registry; it is the test
    /// that matters most all the same, so it is a command rather than a
    /// paragraph: `cargo test -p brocanth-cli -- --ignored`.
    #[test]
    #[ignore = "runs cargo build over a generated application"]
    fn the_scaffold_compiles() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let brocanth = crates
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-scaffold-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = Answers {
            name: "scaffold-test".into(),
            title: "Scaffold Test".into(),
            picks: vec!["logging", "config", "sessions", "rsx", "jt"],
            local: Some(brocanth.display().to_string()),
            git: false,
        };
        for (path, contents) in scaffold(&a).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        // The context reaches a handler, asserted from inside the project.
        // `sessions` is the one generated plugin that provides a value, so
        // this is the whole claim: a handler reads what a plugin published,
        // and a `Ctx` with no application behind it says so.
        let about = root.join("src/pages/about.rs");
        let mut page = std::fs::read_to_string(&about).expect("the scaffold wrote about.rs");
        page.push_str(CONTEXT_REACHES_A_HANDLER);
        std::fs::write(&about, page).unwrap();

        // `test` and not `build`: a generated project carries its own test
        // suite, and `--with openapi` wrote one that failed from the day it
        // landed because nothing here ever ran it.
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .arg("test")
            .current_dir(&root)
            .output()
            .expect("run cargo");
        assert!(
            output.status.success(),
            "the scaffold did not compile:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// SIGTERM in the middle of a request, sent to a generated server.
    ///
    /// Two runs of one binary. With the default grace period the request
    /// finishes, the client reads all of it, and the process exits 0. With
    /// `[server] shutdown_grace_secs` at 1 (through the environment) the
    /// same request outlives the grace period, and the exit is not 0: that
    /// is how an orchestrator sees a drain that cut somebody off.
    ///
    /// `cargo test -p brocanth-cli -- --ignored`.
    #[test]
    #[ignore = "runs cargo build over a generated application"]
    #[cfg(unix)]
    fn sigterm_drains_the_open_request_and_exits_zero() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let brocanth = crates
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-drain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = Answers {
            name: "drain-test".into(),
            title: "Drain Test".into(),
            picks: vec!["config", "commands"],
            local: Some(brocanth.display().to_string()),
            git: false,
        };
        for (path, contents) in scaffold(&a).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        // A page that takes three seconds. The handler is synchronous and
        // holds its worker, which is the case the stop thread is for.
        std::fs::write(root.join("src/pages/slow.rs"), SLOW_PAGE).unwrap();
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .arg("build")
            .current_dir(&root)
            .output()
            .expect("run cargo");
        assert!(
            output.status.success(),
            "the scaffold did not build:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let binary = root.join("target/debug/drain-test");

        let (status, response, log) = slow_request_then_sigterm(&binary, None);
        assert!(
            response.starts_with("HTTP/1.1 200") && response.ends_with("slept"),
            "the request in flight did not finish:\n{response}\n{log}"
        );
        assert_eq!(status.code(), Some(0), "{log}");
        assert!(log.contains("SIGTERM: stopping"), "{log}");
        assert!(log.contains("shutdown (signal) finished"), "{log}");

        let (status, response, log) = slow_request_then_sigterm(&binary, Some("1"));
        assert_ne!(status.code(), Some(0), "{log}");
        assert!(!response.ends_with("slept"), "{response}\n{log}");
        assert!(log.contains("did not finish in 1s"), "{log}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Starts `binary serve` on a free port, sends one slow request, sends
    /// SIGTERM while it runs, and returns the exit status, the response
    /// and the server's stderr.
    #[cfg(unix)]
    fn slow_request_then_sigterm(
        binary: &Path,
        grace: Option<&str>,
    ) -> (std::process::ExitStatus, String, String) {
        use std::io::{BufRead, BufReader, Read, Write};

        let mut command = Command::new(binary);
        command
            .args(["serve", "--addr", "127.0.0.1:0"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        if let Some(secs) = grace {
            command.env("DRAIN_TEST_SERVER__SHUTDOWN_GRACE_SECS", secs);
        }
        let mut child = command.spawn().expect("start the server");
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut log = String::new();
        let addr = loop {
            let mut line = String::new();
            if stderr.read_line(&mut line).unwrap() == 0 {
                panic!("the server stopped before it listened:\n{log}");
            }
            log.push_str(&line);
            if let Some(at) = line.trim().split("listening on http://").nth(1) {
                break at.to_string();
            }
        };
        let rest = std::thread::spawn(move || {
            let mut tail = String::new();
            let _ = stderr.read_to_string(&mut tail);
            tail
        });

        let client = std::thread::spawn(move || {
            let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
            stream
                .write_all(b"GET /slow HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut response = Vec::new();
            let _ = stream.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        });
        // The request is in the handler by now: it sleeps for three seconds.
        std::thread::sleep(std::time::Duration::from_millis(500));
        let killed = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .expect("run kill");
        assert!(killed.success());

        let status = child.wait().expect("wait for the server");
        let response = client.join().unwrap();
        log.push_str(&rest.join().unwrap());
        (status, response, log)
    }

    /// The page `sigterm_drains_the_open_request_and_exits_zero` requests.
    const SLOW_PAGE: &str = r#"//! `/slow` -- a request that is still running when SIGTERM arrives.

use crate::pages::{Ctx, Error, IntoResponse};

pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    std::thread::sleep(std::time::Duration::from_secs(3));
    Ok("slept")
}
"#;

    /// Appended to the generated `about.rs` by `the_scaffold_compiles`.
    ///
    /// Not written into every project: it names `sessions::Session`, which
    /// exists only when that module was chosen, and a scaffold that writes
    /// a test about a plugin a project does not have is a scaffold that
    /// does not compile.
    const CONTEXT_REACHES_A_HANDLER: &str = r#"
#[cfg(test)]
mod context {
    use crate::pages::Ctx;
    use crate::plugins::sessions::Session;

    fn bare() -> Ctx {
        Ctx::new(
            brocanth::routing::Verb::Get,
            "/about",
            brocanth::routing::Params::empty(),
        )
    }

    #[test]
    fn a_handler_reads_what_a_plugin_published() {
        let app = crate::app::build().expect("the application builds");
        let ctx = bare().with_plugins(app.core.context());
        let session = ctx
            .provided::<Session, _>()
            .expect("a dispatched request has a context");
        assert_eq!(session.cookie, "_scaffold_test_session");
    }

    #[test]
    fn a_context_built_by_hand_has_no_plugins_and_does_not_pretend() {
        assert!(bare().provided::<Session, _>().is_none());
    }
}
"#;

    /// The same, for the shape that reads a body.
    ///
    /// Separate because it is the one that proves the entrance: the
    /// `impl Ctx` the `input` module writes into `src/pages/mod.rs` has to
    /// compile against the `Ctx` the generator writes above it, and
    /// nothing else in this file checks that the two agree.
    ///
    /// `cargo test -p brocanth-cli -- --ignored`.
    #[test]
    #[ignore = "runs cargo build over a generated application"]
    fn the_entrance_compiles() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let brocanth = crates
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-entrance-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = Answers {
            name: "entrance-test".into(),
            title: "Entrance Test".into(),
            picks: vec![
                "logging", "config", "commands", "input", "openapi", "client",
            ],
            local: Some(brocanth.display().to_string()),
            git: false,
        };
        for (path, contents) in scaffold(&a).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        // A page that reads a body, which is the whole point of the module
        // and the one thing the generator does not write for you.
        std::fs::write(root.join("src/pages/signup.rs"), SIGNUP_PAGE).unwrap();
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            // The page this test adds is a route, so the project's own
            // "the table is the shape of the directory" test disagrees
            // with its recorded list. Run the module this test wrote.
            .args(["test", "surface"])
            .current_dir(&root)
            .output()
            .expect("run cargo");
        assert!(
            output.status.success(),
            "the entrance did not hold:\n{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The two modules that mount routes, built and tested for real.
    ///
    /// `admin` writes a `mount` into the one table and a test asserting
    /// `/admin` answers 403; `resource` writes the page-triple impls, which
    /// only the compiler can check against the generated `Ctx`, `Response`
    /// and `Error`. The project's own `cargo test` runs both, and its
    /// route-table test lists `/admin`, so a mount that did not reach the
    /// table fails here.
    ///
    /// `cargo test -p brocanth-cli -- --ignored`.
    #[test]
    #[ignore = "runs cargo build over a generated application"]
    fn the_backoffice_compiles() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let brocanth = crates
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-backoffice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = Answers {
            name: "backoffice-test".into(),
            title: "Backoffice Test".into(),
            picks: vec!["logging", "config", "rsx", "admin", "resource"],
            local: Some(brocanth.display().to_string()),
            git: false,
        };
        for (path, contents) in scaffold(&a).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .arg("test")
            .current_dir(&root)
            .output()
            .expect("run cargo");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "the backoffice did not hold:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Ran, not merely compiled: a filter typo or a `cfg` would pass
        // `cargo test` with the admin's own test never reached.
        assert!(
            stdout.contains("the_admin_is_mounted_and_ships_locked ... ok"),
            "{stdout}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A handler written the way the module's documentation says to write
    /// one. If this stops compiling, the documentation is wrong.
    const SIGNUP_PAGE: &str = r##"//! `/signup`.

use crate::pages::{Ctx, Error, IntoResponse, Json, Response, Sent};

/// `pub`, because the table `pages!` writes names this type from one
/// module up. The same rule the handler follows.
#[derive(zod::Entry)]
pub struct Signup {
    #[text(min = 3, max = 16, slug)]
    #[label("Handle")]
    #[says("Pick 3 to 16 lowercase letters, digits or hyphens")]
    pub handle: String,

    #[int(min = 13, max = 130)]
    pub age: i64,

    pub subscribed: bool,
}

#[derive(zod::Entry)]
pub struct Created {
    #[text(uuid)]
    pub id: String,
    pub handle: String,
}

/// The signature is the declaration. A second parameter is the body, read
/// and checked before this runs; a `Json<T>` return is what the document
/// publishes as the answer.
pub fn handle_post(_ctx: &mut Ctx, body: Signup) -> Result<Json<Created>, Error> {
    Ok(Json(Created {
        id: "0191a3f0-0000-7000-8000-000000000000".to_string(),
        handle: body.handle,
    }))
}

/// The other half: every fault, for a page that draws the form again.
pub fn handle_put(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    match ctx.read::<Signup>() {
        Sent::Good(signup) => Ok(Response::ok(signup.handle)),
        Sent::Faulty(faults) => {
            let said: Vec<String> = faults
                .by_name()
                .into_iter()
                .map(|(name, says)| format!("{name}: {}", says.join("; ")))
                .collect();
            Ok(Response::new(422, said.join("\n")))
        }
        Sent::Unreadable(why) => Ok(Response::new(why.status(), why.to_string())),
    }
}

/// What the generated project proves about itself: the table `pages!`
/// wrote, asserted from inside the project rather than from the text the
/// generator hoped it would write.
#[cfg(test)]
mod surface {
    use crate::pages;

    #[test]
    fn the_signature_of_a_handler_is_what_the_table_says() {
        let rows = pages::described();
        let signup = rows
            .iter()
            .find(|row| row.path == "/signup" && row.verb.as_str() == "POST")
            .expect("the page is a route");

        let takes = signup.takes.as_ref().expect("a second parameter is a body");
        assert_eq!(takes.name(), Some("Signup"));
        assert!(takes.object().unwrap().fields.iter().any(|f| f.name == "handle"));

        let gives = signup.gives.as_ref().expect("a Json<T> return is an answer");
        assert_eq!(gives.name(), Some("Created"));

        let plain = rows
            .iter()
            .find(|row| row.path == "/signup" && row.verb.as_str() == "PUT")
            .expect("the other verb");
        assert!(plain.takes.is_none(), "one parameter, so no body in the table");
        assert!(plain.gives.is_none(), "impl IntoResponse says nothing, on purpose");
    }

    #[test]
    fn the_document_describes_the_body_without_anybody_writing_it_down() {
        // The same three pieces `src/plugins/openapi/mod.rs` puts
        // together, without driving a plugin lifecycle by hand: the route
        // table, the description table, and the builder.
        use brocanth_openapi::{Schema, SpecBuilder};

        let router = pages::router().expect("the pages produce a table");
        let mut spec = SpecBuilder::new("Entrance Test", "1.0.0").routes_from_router(&router);
        for route in pages::described() {
            if let Some(node) = &route.takes {
                spec = spec.describing_node(node);
            }
            if let Some(node) = &route.gives {
                spec = spec.describing_node(node);
            }
            let takes = route.takes.as_ref().and_then(|node| node.name());
            let gives = route.gives.as_ref().and_then(|node| node.name());
            spec = spec.describe(route.path, route.verb, move |mut op| {
                if let Some(name) = takes {
                    op = op.json_body(Schema::reference(name));
                }
                if let Some(name) = gives {
                    // A description is required and may not be empty, which
                    // the document's own validator says before anything is
                    // served. One sentence, the same for every operation,
                    // and `.describe(..)` is where a better one goes.
                    op = op.json_response(200, "The answer", Schema::reference(name));
                }
                op
            });
        }

        let json = spec
            .build()
            .expect("the document is valid")
            .to_json()
            .to_json();

        assert!(json.contains("\"Signup\""), "the request body is a component: {json}");
        assert!(json.contains("\"Created\""), "the answer is a component");
        assert!(json.contains("post_signup"), "the operation is there");
        assert!(
            json.contains("Pick 3 to 16 lowercase letters"),
            "the field's own sentence is its description"
        );
        assert!(
            json.contains("\"^[a-z0-9]+(-[a-z0-9]+)*$\""),
            "and the slug rule is the pattern a client validates with"
        );
    }

    #[test]
    fn the_command_line_is_the_same_table_with_flags_on_it() {
        let routes = pages::described();
        let tree = brocanth_client::command("api", &routes);

        let operation = tree
            .find_subcommand("signup-post")
            .expect("one subcommand per operation");
        let help = operation.clone().render_help().to_string();
        assert!(help.contains("--handle"), "a field is a flag: {help}");
        assert!(
            help.contains("Pick 3 to 16 lowercase letters"),
            "and the field's own sentence is its help: {help}"
        );

        let matches = tree
            .try_get_matches_from([
                "api",
                "--base",
                "https://example.test",
                "signup-post",
                "--handle",
                "ada-lovelace",
                "--age",
                "36",
            ])
            .expect("the flags parse");
        let call = brocanth_client::plan(&routes, &matches).expect("one request");

        assert_eq!(call.url, "https://example.test/signup");
        assert_eq!(call.verb.as_str(), "POST");
        assert_eq!(
            call.body.as_deref(),
            Some(r#"{"handle":"ada-lovelace","age":36,"subscribed":false}"#)
        );
    }

    #[test]
    fn the_command_line_refuses_a_number_the_schema_refuses() {
        let routes = pages::described();
        let error = brocanth_client::command("api", &routes)
            .try_get_matches_from(["api", "signup-post", "--handle", "ada-lovelace", "--age", "200"])
            .expect_err("130 is the maximum the handler accepts");
        assert!(error.to_string().contains("200"), "{error}");
    }

    #[test]
    fn an_operation_names_itself_the_same_way_twice() {
        let rows = pages::described();
        let signup = rows.iter().find(|row| row.verb.as_str() == "POST").unwrap();
        assert_eq!(signup.id(), "post_signup");
        assert_eq!(signup.command(), "signup-post");
    }

    #[test]
    fn a_body_that_breaks_a_rule_is_refused_before_the_handler_runs() {
        use brocanth::routing::{Dispatch, Verb};

        let router = pages::router().expect("the pages produce a table");
        let Dispatch::Found { handler, matched } = router.dispatch(Verb::Post, "/signup") else {
            panic!("POST /signup is a route");
        };
        let mut ctx = pages::Ctx::new(Verb::Post, "/signup", matched.params()).sent(
            "",
            b"handle=A&age=9".to_vec(),
            vec![(
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            )],
        );

        // A match and not `unwrap_err`: that one wants the Ok type to be
        // `Debug`, and a `Response` holding a rendered page has no reason
        // to be.
        let Err(refused) = handler(&mut ctx) else {
            panic!("the body broke two rules and was not refused");
        };
        assert_eq!(refused.status, 422);
        assert!(refused.message.contains("handle"), "{}", refused.message);
        assert!(refused.message.contains("age"), "both faults: {}", refused.message);
    }

    #[test]
    fn a_body_that_passes_reaches_the_handler_and_comes_back_as_json() {
        use brocanth::routing::{Dispatch, Verb};

        let router = pages::router().expect("the pages produce a table");
        let Dispatch::Found { handler, matched } = router.dispatch(Verb::Post, "/signup") else {
            panic!("POST /signup is a route");
        };
        let mut ctx = pages::Ctx::new(Verb::Post, "/signup", matched.params()).sent(
            "",
            b"handle=ada-lovelace&age=36&subscribed=on".to_vec(),
            vec![(
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            )],
        );

        let answered = handler(&mut ctx).expect("the body passed");
        assert_eq!(answered.status, 200);
        assert!(answered.body.contains("\"handle\":\"ada-lovelace\""), "{}", answered.body);
        assert!(answered
            .headers
            .iter()
            .any(|(name, value)| name == "content-type" && value == "application/json"));
    }
}
"##;
}
