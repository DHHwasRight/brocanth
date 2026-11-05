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

use brocanth_scaffold::{Dep, Kind, Scaffold as Built, Setting, Slot, Source};
use clap::Args;

use crate::tui::{Choice, Draft};

use crate::commands::Outcome;
use crate::ui::{Cell, Diagnostic, Table, Ui, ACCENT, DIM, STRONG};

/// Where the generated `Cargo.toml` points for the framework itself.
const REPOSITORY: &str = "https://github.com/DHHwasRight/brocanth.git";

/// The environment a new project's first config file is for:
/// `brocanth_config::DEFAULT_ENV`, spelled here because this tool links no
/// configuration loader. A test holds the two equal.
pub const DEFAULT_ENV: &str = "development";

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
    /// Print the chooser's questions, options and defaults, and create nothing
    ///
    /// The same questions the chooser asks, read from the same manifests,
    /// with each module's needs, conflicts and recommendations. An agent
    /// puts these to the person before it runs `create --with`.
    #[arg(long, conflicts_with_all = ["name", "with", "core_only"])]
    pub questions: bool,
    /// With --questions: print JSON instead of text
    #[arg(long, requires = "questions")]
    pub json: bool,
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
                // The categories lead, so `--help` reads as the module map
                // does: what kind of seam each one hooks into.
                let kinds = m
                    .categories
                    .iter()
                    .map(|c| c.name())
                    .collect::<Vec<_>>()
                    .join(", ");
                let help = format!("[{kinds}] {help}");
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
///
/// `observability` and not `logging`: a deployed application needs a
/// request log a machine reads, a probe that tells ready from alive, and
/// counters. Traces wait for a collector. The image and the checks on every
/// push come too, each a choice the wizard can untick.
pub const DEFAULT: [&str; 11] = [
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

/// One error per pair of picks that cannot stand together.
///
/// The rule is not here. It is [`brocanth_scaffold::clash`], over the two
/// manifests: a `conflicts` entry in either, or a module that needs a
/// server beside one that compiles the project to files. The chooser asks
/// the same function, so the two front doors refuse the same pairs.
pub fn exclusive(picks: &[&str]) -> Result<(), Vec<Diagnostic>> {
    // A key this CLI does not have is somebody else's diagnostic.
    let manifests: Vec<_> = picks
        .iter()
        .filter_map(|key| crate::modules::find(key).map(|m| m.manifest()))
        .collect();
    let mut bad = Vec::new();
    for (i, a) in manifests.iter().enumerate() {
        for b in &manifests[i + 1..] {
            if let Some(clash) = brocanth_scaffold::clash(a, b) {
                bad.push(
                    Diagnostic::error(clash.about, clash.message)
                        .note(clash.why)
                        .help(clash.help),
                );
            }
        }
    }
    if bad.is_empty() {
        Ok(())
    } else {
        Err(bad)
    }
}

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
    if args.questions {
        crate::questions::print(ui, args.json, &DEFAULT, None);
        return Ok(ExitCode::SUCCESS);
    }
    // A name on the command line means nobody was asked: see `ask`.
    let driven = args.name.is_some();
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
        // The optional dependencies that were present: what this module
        // wrote differently because of another choice.
        for (other, what) in &about(pick).adapts_to {
            if answers.has(other) {
                contents.row(vec![
                    Cell::styled("", DIM),
                    Cell::styled(format!("with {other}: {what}"), DIM),
                ]);
            }
        }
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

    // The chooser put every question to a person. A run driven by flags
    // asked nobody, so what the chosen modules recommend is said here,
    // where the agent that typed the flags reads it.
    if driven {
        crate::questions::warn_wanted(ui, &answers.picks);
    }

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
    selection(&picked(args), Door::Create)?;

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
        let answers = chooser(ui, args, local)?;
        selection(&answers.picks, Door::Create)?;
        return Ok(answers);
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

/// Which front door a selection came through, so a refusal can say what to
/// type at that door: `--with a,b` to `create`, `brocanth add a` to `add`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    Create,
    Add,
}

impl Door {
    /// The help line that brings `missing` into a selection of `picks`.
    fn bring(self, picks: &[&str], missing: &str, pick: &str) -> String {
        match self {
            Door::Create => {
                let mut with: Vec<&str> = picks.iter().copied().chain([missing]).collect();
                with.sort_unstable();
                with.dedup();
                format!("--with {}", with.join(","))
            }
            Door::Add => format!("brocanth add {missing} {pick}"),
        }
    }
}

/// Every rule a selection of modules must keep, in one place.
///
/// `create` (flags and chooser), `add` and [`scaffold`] itself all call it,
/// so no front door -- and no test that drives the generator directly --
/// can write a project that a refusal would have stopped. A child without
/// its parent and a module without what it needs are refused, never filled
/// in: a flag that quietly adds a module is a flag nobody can read.
pub fn selection(picks: &[&str], door: Door) -> Result<(), Vec<Diagnostic>> {
    unavailable(picks)?;
    orphans(picks, door)?;
    unmet(picks, door)?;
    exclusive(picks)
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
pub fn orphans(picks: &[&str], door: Door) -> Result<(), Vec<Diagnostic>> {
    let bad: Vec<Diagnostic> = picks
        .iter()
        .filter_map(|pick| {
            let parent = about(pick).parent?;
            if picks.contains(&parent) {
                return None;
            }
            Some(
                Diagnostic::error(*pick, format!("`{pick}` is part of `{parent}`"))
                    .note(about(pick).because)
                    .help(door.bring(picks, parent, pick)),
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
pub fn unmet(picks: &[&str], door: Door) -> Result<(), Vec<Diagnostic>> {
    let bad: Vec<Diagnostic> = picks
        .iter()
        .flat_map(|pick| {
            let missing: Vec<(&'static str, &'static str)> = about(pick)
                .needs
                .into_iter()
                .filter(|(need, _)| !picks.contains(need))
                .collect();
            missing.into_iter().map(move |(need, why)| {
                Diagnostic::error(
                    *pick,
                    format!("`{pick}` cannot be written without `{need}`"),
                )
                .note(why)
                .help(door.bring(picks, need, pick))
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
    let mut clashes = crate::tui::clashes(&all()).into_iter();
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
                // The main category, which groups the expert view.
                category: about(pick)
                    .categories
                    .first()
                    .map_or(String::new(), |c| c.name().to_string()),
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
                // Ticking a static site takes off what needs a server, and
                // the other way round; so does a pair a manifest declares.
                clashes: clashes.next().unwrap_or_default(),
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
    framework_settings(&mut built);

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
    wrong.extend(node_clashes(&built));
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

/// The owner the framework's own settings are recorded under. Not a module
/// key, so no module's sections include them.
const FRAMEWORK: &str = "brocanth";

/// The settings the framework reads itself: `[server]`.
///
/// Declared the way a module declares its own, so `[server]
/// shutdown_grace_secs` gets a flag, a variable and a line in the settings
/// report from the same code (`docs/module-flags.md`, "The `[server]`
/// section"). Every project serves, so every project declares it. After the
/// modules, so a module that declares the same key is the one named.
fn framework_settings(built: &mut Built) {
    built.now_writing(FRAMEWORK);
    built.setting(
        "server",
        Setting::integer(
            "shutdown_grace_secs",
            25,
            "Seconds a stop waits for open requests, then for the plugins.",
        ),
    );
    built.settings_type("server", "crate::plugins::ServerSettings");
}

/// Whether a module declared a setting. The framework always declares
/// `[server]`, so [`has_settings`] is true for every project; this is the
/// question of whether a plugin reads what this run was given.
fn module_settings(built: &Built) -> bool {
    built
        .settings()
        .any(|(by, _, setting)| by != FRAMEWORK && setting.kind != Kind::List)
}

/// Every file this project is, with the header and digest on the ones the
/// generator will come back and rewrite.
///
/// `brocanth create` writes the result into a new directory;
/// [`crate::commands::add`] writes the parts of it that a project without
/// the new module did not already have. One generator, two front doors --
/// this repository has already paid twice for two of them drifting apart.
pub fn scaffold(a: &Answers) -> Result<Vec<(String, String)>, Vec<Diagnostic>> {
    // The front doors check first, with their own help line. This is the
    // floor under them: nothing reaches the generator that they would refuse.
    selection(&a.picks, Door::Create)?;
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
            mise_toml(&built, a.has("commands")),
        ),
        (
            "src/main.rs".to_string(),
            if a.has("commands") {
                main_with_commands_rs(a, &built, has_settings(&built))
            } else {
                main_serve_only_rs(a, &built, has_settings(&built))
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
    // The developer's: written when it is missing, so `brocanth add` gives
    // an older project one the first time a module declares a flag, and
    // never touches a rename somebody wrote.
    if has_settings(&built) {
        files.push(("src/flags.rs".to_string(), FLAGS_RS.to_string()));
    }
    if a.has("config") {
        files.push((
            format!("config/{}.toml", DEFAULT_ENV),
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
        if has_settings(&built) {
            files.push((
                "src/commands/settings.rs".to_string(),
                SETTINGS_COMMAND_RS.to_string(),
            ));
        }
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

    // The node packages the modules asked for, in one file, so a second
    // module that needs node adds a line and does not clash on the file.
    if !built.slot_is_empty(Slot::NodeDependencies) {
        if let Some((taken, _)) = files.iter().find(|(p, _)| p == "package.json") {
            return Err(vec![Diagnostic::error(
                "module",
                format!("a module writes {taken}, which the scaffold writes from `Slot::NodeDependencies`"),
            )
            .help("call `brocanth_scaffold::snippet::node_dependency` instead of writing the file")]);
        }
        files.push(("package.json".to_string(), package_json(a, &built)));
    }

    // The brief goes last because it is the one file about all the others,
    // and the listing `create` prints reads in this order.
    files.push((
        "AGENTS.md".to_string(),
        // Whatever the modules have to say to a model reading the project,
        // appended after the sections every project gets. The slot was
        // declared and never read until a module needed to explain why it
        // had registered nothing.
        agents_md(a, &built),
    ));
    Ok(files)
}

/// The node packages in [`Slot::NodeDependencies`], one `name version`
/// line each, sorted by name and without repeats.
fn node_packages(built: &Built) -> Vec<(&str, &str, &str)> {
    let mut packages: Vec<(&str, &str, &str)> = built
        .slot_owners(Slot::NodeDependencies)
        .zip(built.slot(Slot::NodeDependencies))
        .map(|(owner, line)| {
            let (name, version) = line.trim().split_once(' ').unwrap_or((line.trim(), "*"));
            (name, version.trim(), owner)
        })
        .collect();
    packages.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    packages.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    packages
}

/// One error for each node package that two modules ask for at two
/// versions. npm takes one version of a package, so the generator cannot
/// pick one for them.
fn node_clashes(built: &Built) -> Vec<Diagnostic> {
    let packages = node_packages(built);
    packages
        .windows(2)
        .filter(|pair| pair[0].0 == pair[1].0)
        .map(|pair| {
            Diagnostic::error(
                "module",
                format!(
                    "`{}` asks for the node package `{}` at `{}`, and `{}` asks for it at `{}`",
                    pair[0].2, pair[0].0, pair[0].1, pair[1].2, pair[1].1
                ),
            )
            .note("`package.json` holds one version of a package")
            .help("make the two modules name one version range")
        })
        .collect()
}

/// `package.json`, from the packages the modules asked for.
///
/// Every package is a dev dependency: node builds an asset, and the binary
/// that ships does not run node.
fn package_json(a: &Answers, built: &Built) -> String {
    let packages = node_packages(built);
    let lines: Vec<String> = packages
        .iter()
        .map(|(name, version, _)| format!("    {name:?}: {version:?}"))
        .collect();
    format!(
        "{{\n  \"name\": {name:?},\n  \"version\": \"0.0.0\",\n  \"private\": true,\n  \
         \"description\": \"The node packages the brocanth modules of this project build with.\",\n  \
         \"devDependencies\": {{\n{}\n  }}\n}}\n",
        lines.join(",\n"),
        name = a.name,
    )
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
        // `adapted_to` is the optional dependencies that were present, so
        // the file says which other choice shaped this one's code: removing
        // that one changes what this one wrote.
        let m = about(key);
        out.push_str(&format!(
            "\n[modules.{key}]\nkind = [{kind}]\nfiles = [{files}]\nconfig = [{sections}]\n\
             crates = [{crates}]\nadapted_to = [{adapted}]\n",
            kind = quoted(
                &m.categories
                    .iter()
                    .map(|c| c.name().to_string())
                    .collect::<Vec<_>>()
            ),
            adapted = quoted(
                &m.adapts_to
                    .iter()
                    .filter(|(other, _)| a.has(other))
                    .map(|(other, _)| other.to_string())
                    .collect::<Vec<_>>()
            ),
            files = quoted(&attributed(a, files, key)),
            sections = quoted(&sections_of(built, key)),
            crates = quoted(
                &about(key).crates.iter().fold(Vec::new(), |mut names, dep| {
                    // A crate the binary and the build script both take
                    // is one crate.
                    if !names.iter().any(|n: &String| n == dep.name) {
                        names.push(dep.name.to_string());
                    }
                    names
                })
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
    // A module that declared its settings named its sections: `[x]`, and
    // `[x.sub]` for each subsection. That is the answer, not a parse.
    let mut out: Vec<String> = Vec::new();
    for (by, section, setting) in built.settings() {
        if by != key {
            continue;
        }
        let name = match setting.key.rsplit_once('.') {
            Some((sub, _)) => format!("{section}.{sub}"),
            None => section.to_string(),
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    if !out.is_empty() {
        return out;
    }
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
fn agents_md(a: &Answers, built: &Built) -> String {
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

    // What the modules have to say to a model reading the project, after
    // "Adding a route" and before the sections every project gets: a rule
    // about this project's views or settings is closer to the code than
    // the house style is. Each module writes its own (`rsx` its views).
    let sections: String = built.slot(Slot::AgentsSections).collect();

    let commands = if a.has("commands") {
        let mut out = String::from(
            "\n## Tasks\n\nA command is a `clap::Args` struct with `#[cmd]` on it, in its own file under\n\
             `src/commands/`, plus a `pub mod` line at the end of `src/commands/mod.rs`,\n\
             below the line that starts `// Your commands`. `brocanth add` rewrites the\n\
             lines above it and keeps yours. The command becomes a subcommand\n\
             automatically — registration happens at link time.\n\n\
             ```\nbrocanth --help      # lists this project's commands too\nbrocanth serve\n",
        );
        out.push_str("```\n");
        out
    } else {
        String::new()
    };

    // The modules are the person's decisions. An agent that adds one in
    // silence made a choice that was not its own, so the brief says so,
    // and lists what the modules here recommend and the project lacks.
    let mut decisions = String::from(
        "\n## Decisions that belong to the person\n\n\
         The modules in this project are the person's choice. Do not add or\n\
         remove one on your own. Before you run `brocanth add`, show the person\n\
         the question: `brocanth add --questions` lists each module with its\n\
         default, what it needs and what it recommends. Give your\n\
         recommendation, and wait for the answer.\n",
    );
    let wanted = crate::questions::wanted(&a.picks);
    if !wanted.is_empty() {
        decisions.push_str(
            "\nNot chosen, and recommended by a module that is here. Ask about each:\n\n",
        );
        for w in &wanted {
            for (by, why) in &w.by {
                decisions.push_str(&format!("- `{}`, for `{by}`: {why}\n", w.key));
            }
        }
    }

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
         A handler may be `pub async fn` when it waits on something, such as a\n\
         database: the table awaits it and the wait holds no thread. A page that\n\
         waits on nothing stays a plain `fn`. An async handler is dropped at an\n\
         `.await` when the client goes away, so do not leave a write half done\n\
         across one.\n\
         \n\
         **Adding a file does not rebuild on its own** -- nothing referenced it\n\
         yet. Run `touch src/pages/mod.rs` after creating a page or a layout.\n\
         Editing or deleting one is noticed.\n\
         \n\
         A handler returns a value and never writes a response itself. That is\n\
         what makes a half-written body on an error impossible rather than\n\
         unlikely.\n\
         {sections}\
         {commands}\
         {decisions}\
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
        sections = sections,
        commands = commands,
        decisions = decisions,
    )
}

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
        .chain(has_settings(built).then(|| "pub mod settings;".to_string()))
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
            // A build dependency is another line in another table, so it
            // merges only with its own kind.
            match out
                .iter_mut()
                .find(|held| held.name == dep.name && held.build == dep.build)
            {
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
                    for with in dep.with {
                        if !held.with.contains(&with) {
                            held.with.push(with);
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
    let mut picked = merged(&a.picks).expect("the registry does not disagree with itself");
    // serde is the one registry crate that both a module and this file may
    // ask for, so it is held back and written once, after the others.
    let serde = serde_dep(&mut picked, needs_serde(a, built));
    // What a module's `build.rs` uses, in a table of its own.
    let (for_build, mut picked): (Vec<Dep>, Vec<Dep>) = picked.into_iter().partition(|d| d.build);
    let build_deps = if for_build.is_empty() {
        String::new()
    } else {
        format!(
            "\n[build-dependencies]\n{}\n",
            for_build
                .iter()
                .map(|dep| source_of(a, dep))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    // Every project gets the server line below. A module that needs one of
    // its features (`realtime-websocket` needs `websocket`) asks for the
    // crate, and the features join that line rather than a second one.
    let mut server = core("brocanth-server").feature("bridge");
    if let Some(at) = picked.iter().position(|dep| dep.name == "brocanth-server") {
        for feature in picked.remove(at).features {
            if !server.features.contains(&feature) {
                server.features.push(feature);
            }
        }
    }
    for mut dep in picked {
        // Accounts go to Postgres when the project already has a Postgres
        // database. A manifest is static, so this one feature depends on
        // the other picks here; `brocanth_auth::scaffold::POSTGRES_WITH` is
        // the rule, and the plugin file it writes follows the same rule.
        if dep.name == "brocanth-auth"
            && brocanth_auth::scaffold::POSTGRES_WITH
                .iter()
                .any(|key| a.has(key))
        {
            dep = dep.feature("postgres");
        }
        // A feature that exists for another module, on when that module is.
        for (key, feature) in dep.with.clone() {
            if a.has(key) && !dep.features.contains(&feature) {
                dep = dep.feature(feature);
            }
        }
        deps.push(source_of(a, &dep));
    }
    // The macros that read `src/pages`: the route table always, and the
    // layouts when there are any. Compile-time only -- a proc macro puts no
    // bytes in the binary, which is why this one is not a choice.
    deps.push(source_of(a, &core("brocanth-pages")));
    {
        // Every project serves, so every project gets a socket and a reactor.
        // `bridge` for the two conversions `handle` makes around the hooks.
        deps.push(source_of(a, &server));
        deps.push(
            "tokio = { version = \"1\", features = [\"rt-multi-thread\", \"macros\", \"signal\", \"sync\", \"time\"] }  # the reactor `serve` runs on, and the signals that stop it"
                .to_string(),
        );
    }
    if let Some(serde) = &serde {
        deps.push(source_of(a, serde));
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
    if serde.is_none() {
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
         {build_deps}\
         {bins}",
        name = a.name,
        build_deps = build_deps,
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
/// forget. The instance is named after the module, or after the section
/// its declared settings name.
// Only a test asks, today. It is a function rather than a line inside that
// test because the next thing to ask is the generated `main`, and the
// question is about the project rather than about the assertion.
#[cfg(test)]
fn instances(built: &Built) -> Vec<&str> {
    // A plugin with declared settings is registered under its section,
    // which for a library's hooking plugin is not the module key: `csrf`
    // for `middleware-csrf`.
    let mut out: Vec<&str> = built.slot_owners(Slot::AppPlugins).collect();
    for (_, section, _) in built.settings() {
        if !out.contains(&section) {
            out.push(section);
        }
    }
    out
}

/// Whether anything in this project deserialises.
///
/// Only the config plugin makes that possible, and only a module that
/// claimed a config section makes it necessary: a plugin built from a
/// literal in `main` needs no deserialiser, and the scaffold derives none.
fn needs_serde(a: &Answers, built: &Built) -> bool {
    a.has("config") && !built.slot_is_empty(Slot::ConfigSections)
}

/// The project's serde line, if anything wants one.
///
/// Takes serde out of `picked`, where a module put it, and merges what the
/// config plugin needs into it. Without this, a module that derives serde
/// beside `config` writes the crate twice, and the same module without
/// `config` does not compile.
fn serde_dep(picked: &mut Vec<Dep>, for_config: bool) -> Option<Dep> {
    let asked = picked
        .iter()
        .position(|dep| dep.name == "serde")
        .map(|at| picked.remove(at));
    if !for_config {
        return asked;
    }
    let mut serde = Dep::version("serde", "1")
        .feature("derive")
        .note("the config plugin reads typed sections");
    for feature in asked.into_iter().flat_map(|dep| dep.features) {
        if !serde.features.contains(&feature) {
            serde.features.push(feature);
        }
    }
    Some(serde)
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
         use brocanth::plugin_hooks::{{self as hooks, Begin, RequestState}};\n\
         use brocanth::plugin_routes::BoxFuture;\n\
         use brocanth::plugin_shutdown::ShutdownReason;\n\
         use brocanth::routing::{{AsyncMiddlewareStack, Dispatch, Router, Verb}};\n\
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
         /// Where `serve` listens when nothing says else: `${upper}_ADDR`, or\n\
         /// `127.0.0.1:3000`. The container image sets `0.0.0.0:3000`.\n\
         pub fn default_addr() -> String {{\n\
        \x20   std::env::var({addr_var:?}).unwrap_or_else(|_| \"127.0.0.1:3000\".to_string())\n\
         }}\n\
         \n\
         /// What a request needs, built once and shared by every connection.\n\
         pub struct App {{\n\
        \x20   pub router: Router<Handler>,\n\
        \x20   pub stack: AsyncMiddlewareStack<Ctx, Response>,\n\
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
        name = a.name,
        addr_var = format!("{}_ADDR", a.ident().to_uppercase()),
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
        \x20   let mut stack: AsyncMiddlewareStack<Ctx, Response> = AsyncMiddlewareStack::new();\n\
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

    out.push_str(&serve_fns());
    out.push_str(
        "/// One request: the request hooks, the table, then the response hooks.\n\
         ///\n\
         /// A request hook may answer instead of the page -- a CSRF refusal, a\n\
         /// 429 -- and then the page never runs. It may also rewrite the request\n\
         /// first, so the table reads the request as the hooks left it. The\n\
         /// response hooks see every answer, the 404 and the 405 included.\n\
         ///\n\
         /// `begin_async` and not `begin`: it runs a hook that waits as well as\n\
         /// one that does not. With no hook that waits, it is ready on its first\n\
         /// poll.\n\
         pub async fn handle(app: &App, request: brocanth_server::Request) -> brocanth_server::Response {\n\
        \x20   let Ok(mut request) = brocanth_server::bridge::to_core(request) else {\n\
        \x20       // The path and the query do not make a URI. The table was not\n\
        \x20       // asked, so this is not a 404.\n\
        \x20       return brocanth_server::Response::text(400, \"Bad Request\");\n\
        \x20   };\n\
        \x20   let mut state = RequestState::new();\n\
        \x20   let outcome = match app.core.begin_async(&mut request, &mut state).await {\n\
        \x20       Begin::Dispatch => Ok(sent(dispatch(app, &request, &mut state).await)),\n\
        \x20       Begin::Answered(outcome) => outcome,\n\
        \x20   };\n\
        \x20   let response = app.core.finish(&mut request, &mut state, outcome);\n\
        \x20   brocanth_server::bridge::from_core(response)\n\
         }\n\
         \n\
         /// The table, then the chain, then the handler.\n\
         ///\n\
         /// The chain runs only for a route that matched. A 404 has no route and\n\
         /// therefore no context. `state` is what the request hooks wrote; the\n\
         /// page reads it through `Ctx::hooked`, and it goes back afterwards so\n\
         /// the response hooks read what the page left.\n\
         ///\n\
         /// `async` because a page may be: the chain and the page are awaited\n\
         /// here, on the worker, and a page that waits on a database does not\n\
         /// hold a thread while it does.\n\
         pub async fn dispatch(app: &App, request: &hooks::Request, state: &mut RequestState) -> Response {\n\
        \x20   let Some(verb) = Verb::from_http(request.method()) else {\n\
        \x20       return Response::new(501, \"Not Implemented\");\n\
        \x20   };\n\
        \x20   let path = request.uri().path();\n\
        \x20   match app.router.dispatch(verb, path) {\n\
        \x20       Dispatch::Found { handler, matched } => {\n\
        \x20           let mut ctx = @CTX@;\n\
        \x20           // The one place a failed handler becomes a response --\n\
        \x20           // what Echo calls an HTTPErrorHandler. It sits inside\n\
        \x20           // the chain's terminal, so a layer still sees what the\n\
        \x20           // error turned into and can rewrite it.\n\
        \x20           //\n\
        \x20           // `handler(c)` is called before the future is built: the\n\
        \x20           // future may borrow the context and nothing else.\n\
        \x20           let response = app\n\
        \x20               .stack\n\
        \x20               .run(path, &mut ctx, |c: &mut Ctx| -> BoxFuture<'_, Response> {\n\
        \x20                   let answer = handler(c);\n\
        \x20                   Box::pin(\n\
        \x20                       async move { answer.await.unwrap_or_else(IntoResponse::into_response) },\n\
        \x20                   )\n\
        \x20               })\n\
        \x20               .await;\n\
        \x20           *state = ctx.take_state();\n\
        \x20           response\n\
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
    let args = "\x20               request.uri().query().unwrap_or(\"\"),\n\
         \x20               request.body().clone(),\n\
         \x20               headers(request),\n";
    let deeper: String = args.lines().map(|line| format!("    {line}\n")).collect();
    // `.with_plugins` last, and always: a handler reads what a plugin put
    // on the context, and `Ctx::new` cannot build one because the
    // providers live in the application. This is the one place that has
    // both.
    out.replace(
        "@CTX@",
        &format!(
            "Ctx::new(verb, path, matched.params())\n\
             \x20               .with_plugins(app.core.context_for(request, state))\n\
             \x20               .with_state(std::mem::take(state))\n\
             \x20               .sent(\n{deeper}\x20               )"
        ),
    )
}

/// The grace period, `serve`, and the stop that follows it, in `src/app.rs`.
///
/// The same text whatever the module list is: `src/app.rs` is the
/// developer's, and `brocanth add` does not rewrite it. Where the grace
/// period comes from does vary -- the files, the environment and the flags
/// with the config plugin, the environment and the flags without -- so that
/// question goes to `plugins::shutdown_grace`, which the generator owns.
fn serve_fns() -> String {
    SERVE_FN.to_string() + "\n"
}

/// `serve`, the drain, and the signals. ADR 2 in the generated project.
const SERVE_FN: &str = r#"/// How long a stop waits for open requests and then for the plugins.
///
/// Kubernetes sends SIGTERM, waits `terminationGracePeriodSeconds` (30 by
/// default) and then sends SIGKILL. 25 seconds leaves 5 for the exit.
/// `[server] shutdown_grace_secs` overrides it: `--server.shutdown_grace_secs`,
/// `$<APP>_SERVER__SHUTDOWN_GRACE_SECS`, or with the config plugin the file.
/// See `plugins::shutdown_grace`.
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
    // that opens a connection pool builds it in `before_start`
    // with no connection, and the pool spawns its upkeep task on
    // the current runtime. Building the application first would
    // leave it with no runtime to spawn on.
    let app = {
        let _guard = runtime.enter();
        build()?
    };
    let names = app.plugins.join(", ");
    plugins::say!(info, "plugins: [{names}]"; plugins = %names);

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
    // Every readiness check once, before the socket: the same checks
    // `/readyz` runs. The build waits for no I/O, so a wrong password or
    // an unreachable database stops the start here, with the plugin's
    // message and its `help:` line, and not at the first request. A
    // check can wait for a pool's acquire timeout, so a signal ends the
    // wait.
    tokio::select! {
        checked = app.core.readiness().check_all() => {
            checked.map_err(|not_ready| not_ready.to_string())?;
        }
        _ = stop.recv() => {
            let report = app.core.shutdown(ShutdownReason::Signal, Some(grace));
            plugins::say!(info, "{report}");
            return Ok(());
        }
    }

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
        let report = app.core.shutdown(ShutdownReason::Failed, None);
        plugins::say!(error, "{report}");
        return Err(e.to_string());
    }

    // The probes and `/metrics` on a port of their own, when the
    // configuration names one.
    plugins::side_listeners(&app).await?;

    let app = Arc::new(app);
    // `plugins::wrap` puts whatever observes a request around it.
    let handler = plugins::wrap(&app, {
        let app = Arc::clone(&app);
        move |request| {
            let app = Arc::clone(&app);
            async move { handle(&app, request).await }
        }
    });
    let mut serving = tokio::spawn(server.run(handler));
    plugins::say!(info, "listening on http://{bound}"; addr = %bound);

    // Phase 1: stop accepting, and give the open requests the grace period.
    let mut failed = tokio::select! {
        event = stop.recv() => {
            let signal = match event {
                Some(Stop::Signal(signal)) => signal,
                _ => "stop",
            };
            plugins::say!(
                info,
                "{signal}: stopping; open requests have {grace:?} to finish";
                signal,
                ?grace
            );
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
    // Not ready from the first instant of the stop (ADR 2): `/readyz`
    // answers 503, and a plugin that awaits `Readiness::draining` stops
    // taking work now -- the job worker stops claiming -- while the
    // listener still drains.
    app.core.readiness().drain();
    if failed.is_none() {
        // With `observe`, `/readyz` fails first, and the listener closes
        // after `[observability] drain_delay_secs`. Inside the grace period.
        plugins::stop_accepting(&app, stopper).await;
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
            plugins::say!(info, "{report}");
            if failed.is_none() && (!report.is_clean() || report.overran()) {
                failed = Some(format!("the plugins did not stop cleanly in {grace:?}"));
            }
        }
        None => plugins::say!(
            warn,
            "a request still holds the application, so no plugin stopped"
        ),
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
/// The `mod` lines of `src/main.rs`: the generator's own and each
/// `Slot::MainMods` line, sorted by name as rustfmt sorts them. An entry
/// is `(name, text)`, because a module with an attribute sorts by its
/// name and not by the `#`.
fn crate_mods(own: &[(&str, &str)], built: &Built) -> String {
    let theirs: Vec<(String, String)> = built
        .slot(Slot::MainMods)
        .map(|line| {
            let line = line.trim();
            let name = line
                .trim_start_matches("pub ")
                .trim_start_matches("mod ")
                .trim_end_matches(';');
            (name.to_string(), line.to_string())
        })
        .collect();
    let mut all: Vec<(String, String)> = own
        .iter()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .chain(theirs)
        .collect();
    all.sort();
    all.dedup();
    all.into_iter().map(|(_, text)| text + "\n").collect()
}

fn main_serve_only_rs(a: &Answers, built: &Built, flags: bool) -> String {
    if flags {
        return main_serve_only_with_flags_rs(a, built);
    }
    format!(
        r##"//! {title} -- a brocanth application.
//!
//! No command line was asked for, so this binary does one thing. Everything it
//! contains is in `src/app.rs`; `brocanth add commands` turns this into a
//! command line with `serve` as one of its commands.

{mods}
use std::process::ExitCode;

fn main() -> ExitCode {{
    match app::serve(&app::default_addr()) {{
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {{
            // Through `say!`: a JSON line when the log subscriber is up,
            // text when the failure came before it.
            plugins::say!(error, "error: {{message}}");
            ExitCode::FAILURE
        }}
    }}
}}
"##,
        title = a.title,
        mods = crate_mods(
            &[
                ("app", "mod app;"),
                ("pages", "mod pages;"),
                ("plugins", "mod plugins;")
            ],
            built
        ),
    )
}

/// `src/main.rs` with no command line: it reads the declared flags, refuses
/// anything else, and serves.
fn main_serve_only_with_flags_rs(a: &Answers, built: &Built) -> String {
    format!(
        r##"//! {title} -- a brocanth application.
//!
//! No command line was asked for, so this binary does one thing. It takes the
//! flags its settings declare, which `--help` lists and `src/flags.rs`
//! renames, and nothing else. Everything it contains is in `src/app.rs`;
//! `brocanth add commands` turns this into a command line with `serve` as one
//! of its commands.

{mods}
use std::process::ExitCode;

fn main() -> ExitCode {{
    let served = plugins::read_line().and_then(|helped| {{
        if helped {{
            Ok(())
        }} else {{
            app::serve(&app::default_addr())
        }}
    }});
    match served {{
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {{
            // Through `say!`: a JSON line when the log subscriber is up,
            // text when the failure came before it.
            plugins::say!(error, "error: {{message}}");
            ExitCode::FAILURE
        }}
    }}
}}
"##,
        title = a.title,
        mods = crate_mods(
            &[
                ("app", "mod app;"),
                ("flags", "mod flags;"),
                ("pages", "mod pages;"),
                ("plugins", "mod plugins;")
            ],
            built
        ),
    )
}

/// `src/main.rs` when the project has a command line: it is the command line.
fn main_with_commands_rs(a: &Answers, built: &Built, flags: bool) -> String {
    let mut own = vec![
        ("app", "mod app;"),
        ("commands", "mod commands;"),
        ("pages", "mod pages;"),
        ("plugins", "mod plugins;"),
    ];
    if flags {
        own.push(("flags", "mod flags;"));
    }
    // Outside `src/`, where `migrate make` writes, so the path is
    // spelled. Only with a command line: `migrate` is what reads it.
    if a.has("model") {
        own.push((
            "migrations",
            "#[path = \"../migrations/mod.rs\"]\nmod migrations;",
        ));
    }
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

{mods}
use std::process::ExitCode;

fn main() -> ExitCode {{
    match run() {{
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {{
            // Through `say!`: a JSON line when `serve` got as far as the log
            // subscriber, text for a failure before it or another command.
            plugins::say!(error, "error: {{e:#}}");
            ExitCode::FAILURE
        }}
    }}
}}

fn run() -> brocanth_command::anyhow::Result<()> {{
{run}
}}
"##,
        // The flags the modules declared, as arguments every command takes.
        run = if flags {
            "    brocanth_command::main_with(brocanth_command::Context::boot()?, plugins::flag_args)"
        } else {
            "    brocanth_command::main(brocanth_command::Context::boot()?)"
        },
        title = a.title,
        mods = crate_mods(&own, built),
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
    #[arg(long, default_value_t = crate::app::default_addr())]
    pub addr: String,
}

impl Command for Serve {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        crate::app::serve(&self.addr).map_err(anyhow::Error::msg)
    }
}
"#;

fn mise_toml(built: &Built, commands: bool) -> String {
    // With `commands` the binary is a command line, and one given no
    // subcommand prints its help: the server is `serve`, as in the
    // Dockerfile. Without it the binary only serves.
    let serve = if commands {
        "cargo run -- serve"
    } else {
        "cargo run"
    };
    let mut out = MISE_TOML.replace("@SERVE@", serve);
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
run = "@SERVE@"

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

use brocanth::plugin_body::{BodyStream, Streaming};
use brocanth::plugin_context::{Context, ContextItem, Has};
use brocanth::plugin_hooks::RequestState;
use brocanth::plugin_routes::BoxFuture;
use brocanth::routing::{BuildError, Params, Router, Verb};

use crate::plugins;

/// What a handler produces.
///
/// The framework owns rendering: a handler returns a value and never writes a
/// response itself, so a half-written body on an error is not something that
/// can happen. Echo and Gin hand the handler a writer and take an error back,
/// and both carry the scar -- once a 200 has gone out, the central error
/// handler cannot take it back. Here there is nothing to take back.
///
/// The one exception is opt-in: [`Response::stream`] and [`Response::events`]
/// answer with a body that is written after the handler returns. A stream
/// that fails halfway cannot become a 500, because the status is already on
/// the wire. Use it for server-sent events and large downloads, and nowhere
/// else.
pub struct Response {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
    /// A body the server polls after the handler returns. `None` for every
    /// page that answers with `body`, which is nearly all of them; when it
    /// is `Some`, `body` is ignored. Set it with [`Response::stream`].
    pub stream: Option<Streaming>,
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
            stream: None,
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

    /// A body written as it is produced: the first bytes reach the client
    /// before the last exist.
    ///
    /// The response hooks see the headers and never the body. The server
    /// tells the stream to end when it stops (`BodyStream::finish`), so a
    /// long-lived stream should implement that, or a deploy waits for it.
    #[allow(dead_code)]
    pub fn stream(status: u16, content_type: &str, stream: impl BodyStream) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_string(), content_type.to_string())],
            stream: Some(Streaming::new(stream)),
            ..Default::default()
        }
    }

    /// Server-sent events: `text/event-stream`, and the two headers that stop
    /// a cache or a proxy from holding the events back.
    ///
    /// `brocanth_server::sse::channel` makes the stream from async code:
    ///
    /// ```ignore
    /// let (events, stream) = brocanth_server::sse::channel(16);
    /// tokio::spawn(async move {
    ///     // `Err` when the client left or the server is stopping.
    ///     let _ = events.send(Event::new("hello").with_event("greeting")).await;
    /// });
    /// Ok(Response::events(stream.heartbeat(Duration::from_secs(15))))
    /// ```
    #[allow(dead_code)]
    pub fn events(stream: impl BodyStream) -> Self {
        Self::stream(200, "text/event-stream", stream)
            .with_header("cache-control", "no-cache")
            .with_header("x-accel-buffering", "no")
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
        if withheld {
            // So the message does reach the logs: a database error that came
            // up through `?` says here what failed, and the client sees none
            // of it.
            let (status, message) = (self.status, &self.message);
            crate::plugins::say!(error, "error {status}: {message}"; status, %message);
        }
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
            stream: None,
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
    plugins: Option<Context<plugins::Installed>>,
    /// What the request hooks wrote for this request: a `CsrfToken`, a
    /// client address. Empty for a context built by hand.
    state: RequestState,
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
            plugins: None,
            state: RequestState::new(),
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

    /// What the request hooks wrote. `src/app.rs` hands it over before the
    /// page runs and takes it back after, for the response hooks.
    #[must_use]
    pub fn with_state(mut self, state: RequestState) -> Self {
        self.state = state;
        self
    }

    /// The hook state back, leaving this context an empty one.
    pub fn take_state(&mut self) -> RequestState {
        std::mem::take(&mut self.state)
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
    }

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
    /// A value a request hook wrote for this request, by its type.
    ///
    /// `ctx.hooked::<brocanth_middleware::CsrfToken>()` is the token a form
    /// carries. `None` when no registered hook wrote one, and always for a
    /// context built by hand.
    pub fn hooked<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.state.get::<T>()
    }

    /// The same value, to change. `None` when no hook wrote one.
    ///
    /// The state goes back to the hooks when the page returns, so a change
    /// made here is what `after_dispatch` and the response hooks read: a
    /// request hook puts a per-request log record on the state, and the
    /// page fills in the fields only it knows.
    pub fn hooked_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.state.get_mut::<T>()
    }

    /// Puts a value on the request state for the hooks that run after the
    /// page, and returns the value it replaced.
    ///
    /// For a value that no request hook wrote first. A hook that reads it
    /// must also accept `None`: a 404, a 405 and a request that a request
    /// hook answered run no page.
    pub fn hook_insert<T: Send + Sync + 'static>(&mut self, value: T) -> Option<T> {
        self.state.insert(value)
    }

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
/// own type and a `fn` pointer can only hold one of them. [`page`] and
/// [`page_async`] are what unify them, and they run once per route at
/// startup rather than per request.
///
/// It returns a future, so a page may wait -- on a database, on another
/// service -- without holding a thread. A page that waits on nothing is
/// still a plain `fn`, and [`page`] hands back its answer already finished.
pub type Handler =
    Box<dyn for<'a> Fn(&'a mut Ctx) -> BoxFuture<'a, Result<Response, Error>> + Send + Sync>;

/// Puts a `pub fn handle` in the table whatever it returns.
///
/// Generic over the return type, so `-> Result<impl IntoResponse, Error>` on
/// the handler and one concrete `Handler` in the table are the same thing.
/// The answer is ready when the handler returns; the future around it is
/// only there so a sync page and an async page share one table.
pub fn page<H, T>(handler: H) -> Handler
where
    H: Fn(&mut Ctx) -> Result<T, Error> + Send + Sync + 'static,
    T: IntoResponse,
{
    Box::new(move |ctx| {
        let answered = handler(ctx).map(IntoResponse::into_response);
        Box::pin(std::future::ready(answered))
    })
}

/// Puts a `pub async fn handle` in the table.
///
/// `pages!` writes the call, with a closure that awaits the page and turns
/// its answer into a `Response`. The closure is a `fn` pointer and not a
/// generic, because the page's future borrows the context and a generic
/// cannot name that borrow.
///
/// `allow(dead_code)`: nothing calls it until a page is `async`.
#[allow(dead_code)]
pub fn page_async(
    handler: for<'a> fn(&'a mut Ctx) -> BoxFuture<'a, Result<Response, Error>>,
) -> Handler {
    Box::new(handler)
}

"##;

/// A page's answer, as the server sends it. Always written: both shapes of
/// `serve` go through it, so one place turns a `Response` into bytes.
const SENT_FN: &str = r#"/// Turns a page's answer into the response the hooks see.
///
/// `text/html` unless the page names its own `content-type`, which then
/// replaces it rather than joining it. Every other header is appended,
/// because `set-cookie` has to repeat. A page that answered with a stream
/// keeps it: the hooks see its headers, and the server polls its body.
fn sent(answered: Response) -> hooks::Response {
    match answered.stream {
        Some(stream) => {
            brocanth_server::bridge::streaming(answered.status, answered.headers, stream)
        }
        None => brocanth_server::bridge::buffered(
            answered.status,
            answered.headers,
            answered.body.into_bytes(),
        ),
    }
}

/// The request headers as text, for the page.
///
/// A value that is not UTF-8 is dropped: a page reads a header as `&str`,
/// and a lossy copy would say something the client did not send.
fn headers(request: &hooks::Request) -> Vec<(String, String)> {
    request
        .headers()
        .iter()
        .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_string())))
        .collect()
}

"#;

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

    #[test]
    fn a_page_writes_what_the_hooks_read_after_it() {
        // A request hook's record, as `after_dispatch` would find it.
        struct Record(Vec<&'static str>);
        let mut state = RequestState::new();
        state.insert(Record(vec!["hook"]));
        let mut ctx = Ctx::new(Verb::Get, "/", Params::empty()).with_state(state);

        let record = ctx.hooked_mut::<Record>().expect("the hook wrote it");
        record.0.push("page");
        assert!(ctx.hook_insert(7_u32).is_none(), "nothing was there");

        let state = ctx.take_state();
        assert_eq!(state.get::<Record>().expect("kept").0, ["hook", "page"]);
        assert_eq!(state.get::<u32>(), Some(&7));
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

/// The header over the generated `Hooks` alias.
///
/// `NoHooks` when nothing registers with `plugin_hooking`, and one `Both`
/// per hooking plugin otherwise, built from [`Slot::PluginsHooks`] the way
/// `Installed` is built from [`Slot::PluginsInstalled`]. It is written down
/// rather than inferred for the reason `Installed` is: `App` is a struct
/// and a struct field needs a type to name.
const HOOK_RECORD_DOC: &str = "\
/// Which hook families this binary installed, as a type.
///
/// One `Both` per plugin registered with `plugin_hooking`, innermost first,
/// which is exactly the record `register!` builds. A hook family nobody
/// installed costs nothing, and this type is how the compiler knows: every
/// hook entry point opens with a constant `if` on this record.
";

/// The `Hooks` alias and the aliases it is built from.
/// `say!` with `observability-logs`: a `tracing` event, so production gets a
/// JSON line with fields, through the subscriber the observability plugin
/// installed.
const SAY_LOGGED: &str = r#"/// A line about the application itself: startup, the listener, the stop.
///
/// A `tracing` event, because `observability-logs` is installed: the line is
/// JSON in production and text elsewhere, on stdout, like the request lines.
/// The fields after `;` are the event's fields, in `tracing`'s shorthand
/// (`addr`, `%addr`, `?addr`), and name variables already in scope.
///
/// ```ignore
/// plugins::say!(info, "listening on http://{bound}"; %bound);
/// ```
///
/// Before the observability plugin installs its subscriber (a config file
/// that does not parse, a plugin that fails before it) and with
/// `install_subscriber = false`, an event would go nowhere. Then this is a
/// line on stderr, as it is without `observability-logs`.
macro_rules! say {
    ($level:ident, $message:literal $(; $($field:tt)+)?) => {
        if brocanth_observe::tracing::dispatcher::has_been_set() {
            brocanth_observe::tracing::$level!($($($field)+,)? $message)
        } else {
            eprintln!("{}: {}", crate::app::APP, format_args!($message))
        }
    };
}
pub(crate) use say;

"#;

/// `say!` without `observability-logs`: no subscriber exists, so a line on
/// stderr. The fields are accepted and dropped, so a call site is the same
/// text in both.
const SAY_PRINTED: &str = r#"/// A line about the application itself: startup, the listener, the stop.
///
/// A line on stderr, prefixed with the application name. The level and the
/// fields after `;` are what `observability-logs` would record; without it
/// there is no subscriber, so they are dropped and the message is printed.
///
/// ```ignore
/// plugins::say!(info, "listening on http://{bound}"; %bound);
/// ```
macro_rules! say {
    ($level:ident, $message:literal $(; $($field:tt)+)?) => {
        eprintln!("{}: {}", crate::app::APP, format_args!($message))
    };
}
pub(crate) use say;

"#;

fn hook_record(built: &Built) -> String {
    let mut out = String::from(HOOK_RECORD_DOC);
    let mut held = String::from("NoHooks");
    let mut names: Vec<String> = Vec::new();
    let mut aliases = String::new();
    for ty in built.slot(Slot::PluginsHooks) {
        let ty = ty.trim_end();
        let last = ty.rsplit("::").next().unwrap_or(ty);
        let mut name = format!("Hooked{}", upper_camel(last));
        while names.contains(&name) {
            name.push('2');
        }
        names.push(name.clone());
        let line = format!("type {name} = Both<<{ty} as HookSource>::Families, {held}>;\n");
        if line.len() > 101 {
            aliases.push_str(&format!(
                "type {name} =\n    Both<<{ty} as HookSource>::Families, {held}>;\n"
            ));
        } else {
            aliases.push_str(&line);
        }
        held = name;
    }
    if names.is_empty() {
        out.push_str("pub type Hooks = brocanth::plugin_hooks::NoHooks;\n");
    } else {
        // The aliases above the doc comment, so the comment stays on `Hooks`.
        out = aliases + "\n" + &out;
        out.push_str(&format!("pub type Hooks = {held};\n"));
    }
    out
}

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
    if !built.slot_is_empty(Slot::PluginsHooks) {
        out.push_str("use brocanth::plugin_hooks::{Both, HookSource, NoHooks};\n");
    }
    out.push_str(concat!(
        "use brocanth::plugin_routes::BoxFuture;\n",
        "use brocanth::routing::AsyncNext;\n",
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
        // `plugins::config` loads the file, so the chain needs only the
        // extension. Every project declares `[server]`, so it always has
        // `plugins::config`.
        uses.push("use brocanth_config::AppBuilderConfigExt;\n".to_string());
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
            "        // The files, the environment and, once `src/flags.rs` turns them\n\
            \x20       // on, the flags: `brocanth-config` layers them and types each\n\
            \x20       // section, and it is the only reason a TOML parser is in the tree.\n\
            \x20       let config = crate::plugins::config()?;\n\
            \x20       {head}.with_config(config){end}\n",
            end = if registrations.is_empty() { "" } else { ";" },
        ));
    } else if !a.has("config") && module_settings(built) {
        out.push_str(&format!(
            "        // No file and no TOML parser. Each plugin below starts from its\n\
            \x20       // defaults and takes what this run was given on top: the\n\
            \x20       // environment, then the flags.\n\
            \x20       let given = crate::plugins::given()?;\n\
            \x20       {head}{end}\n",
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
        // rustfmt breaks after `=` past 100 columns, so this does too.
        let line = format!("type {name} = After<{path}, {held}>;");
        if line.len() > 100 {
            out.push_str(&format!("type {name} =\n    After<{path}, {held}>;\n"));
        } else {
            out.push_str(&line);
            out.push('\n');
        }
        held = name;
    }
    out.push_str(&format!("pub type Installed = {held};\n\n"));
    out.push_str(&hook_record(built));

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

    // -- say ----------------------------------------------------------
    //
    // Every line the binary writes about itself at startup and at stop goes
    // through one macro, so the choice between a log event and a line of
    // text is made once, here, from the module list. `src/app.rs` and the
    // `announce!` lines call it and never change when the list does.
    out.push_str(if a.has("observability-logs") {
        SAY_LOGGED
    } else {
        SAY_PRINTED
    });

    // -- layer --------------------------------------------------------
    out.push_str(
        "/// One middleware layer, as the stack stores it.\n\
         ///\n\
         /// A name rather than the type written out: clippy calls the bare\n\
         /// form a very complex type, and it is also the type `src/app.rs`\n\
         /// would have to spell to hold one.\n\
         ///\n\
         /// A layer may wait, so it returns a future. The lifetime is named\n\
         /// because the future borrows both the context and the rest of the\n\
         /// chain; see `sessions::layer` for the shape of one.\n\
         pub type Layer = for<'a> fn(&'a mut Ctx, AsyncNext<'a, Ctx, Response>) -> BoxFuture<'a, Response>;\n\
         \n\
         /// The function behind a middleware name the application mounted.\n\
         ///\n\
         /// Mounting is `src/app.rs`'s decision and stays there: the pattern is a\n\
         /// decision and the decision is the application's. Which function answers\n\
         /// to `\"session\"` is not a decision, so it is here, where adding a\n\
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

    // -- the flags ----------------------------------------------------
    //
    // Here and not in `src/flags.rs`, because the list follows from the
    // module list. The renames are the developer's, so they are there.
    if has_settings(built) {
        out.push('\n');
        out.push_str(&flags_rs(a, built));
    }

    // -- the grace period ---------------------------------------------
    //
    // Here and not in `src/app.rs`, because where it comes from varies with
    // the module list: the configuration with the config plugin, the
    // environment and the flags without.
    out.push('\n');
    out.push_str(SERVER_SETTINGS);
    out.push('\n');
    out.push_str(if a.has("config") {
        GRACE_FROM_CONFIG
    } else {
        GRACE_PLAIN
    });

    // -- serving ------------------------------------------------------
    //
    // What `serve` does around the handler: wrap it, open side listeners,
    // and stop. Here for the same reason: the module list changes it, and
    // `src/app.rs` is not rewritten. Each wrap is a module's
    // `Slot::ServerWrap` line; this file only composes them.
    out.push('\n');
    out.push_str(&serving(built));
    out
}

/// `wrap`, `side_listeners` and `stop_accepting`, composed from the
/// `Slot::ServerWrap` lines. Without one, the plain three, unchanged since
/// before the slot existed.
fn serving(built: &Built) -> String {
    let wraps: Vec<&str> = built.slot(Slot::ServerWrap).map(str::trim).collect();
    if wraps.is_empty() {
        return SERVING_PLAIN.to_string();
    }
    // `next` is wrapped innermost first, so the first line is the
    // outermost wrap: it sees a request first and the response last.
    let wrapped: String = wraps
        .iter()
        .rev()
        .map(|ty| format!("    let next = <{ty} as ServerWrap>::wrap(app, next);\n"))
        .collect();
    let listeners: String = wraps
        .iter()
        .map(|ty| format!("    <{ty} as ServerWrap>::side_listeners(app).await?;\n"))
        .collect();
    let stops: String = wraps
        .iter()
        .map(|ty| format!("    <{ty} as ServerWrap>::before_stop(app).await;\n"))
        .collect();
    let list: String = wraps.iter().map(|ty| format!("/// - `{ty}`\n")).collect();
    SERVING_WRAPPED
        .replace("@LIST@", &list)
        .replace("@WRAPPED@", &wrapped)
        .replace("@LISTENERS@", &listeners)
        .replace("@STOPS@", &stops)
}

/// `wrap`, `side_listeners` and `stop_accepting` around the server wraps
/// the modules declared.
const SERVING_WRAPPED: &str = r#"/// What the server is handed for one request.
pub type Answer = brocanth::plugin_routes::BoxFuture<'static, brocanth_server::Response>;

/// A handler, as one server wrap hands it to the next.
pub type Handler = std::sync::Arc<dyn Fn(brocanth_server::Request) -> Answer + Send + Sync>;

/// Something around the whole server: it sees every request, a static
/// file included, outside the request hooks.
///
/// A module writes a type that implements it and names the type in
/// `Slot::ServerWrap`. Only `wrap` is required.
pub trait ServerWrap {
    /// `next`, with this wrap around it.
    fn wrap(app: &std::sync::Arc<crate::app::App>, next: Handler) -> Handler;

    /// Listeners of its own, beside the main one. Called once, after the
    /// plugins started.
    fn side_listeners(
        app: &crate::app::App,
    ) -> impl std::future::Future<Output = Result<(), String>> {
        let _ = app;
        async { Ok(()) }
    }

    /// Called when a stop begins, before the listener closes. A wrap that
    /// must keep answering for a while -- a readiness probe that fails
    /// first -- waits here.
    fn before_stop(app: &crate::app::App) -> impl std::future::Future<Output = ()> {
        let _ = app;
        async {}
    }
}

/// What `serve` answers a request with: `next`, inside each server wrap.
/// The first is the outermost: it sees a request first and the response
/// last.
///
@LIST@pub fn wrap<F, Fut>(
    app: &std::sync::Arc<crate::app::App>,
    next: F,
) -> impl Fn(brocanth_server::Request) -> Answer + Send + Sync + 'static
where
    F: Fn(brocanth_server::Request) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = brocanth_server::Response> + Send + 'static,
{
    let next: Handler = std::sync::Arc::new(move |request| -> Answer { Box::pin(next(request)) });
@WRAPPED@    move |request| next(request)
}

/// Each wrap's own listeners, in order.
pub async fn side_listeners(app: &crate::app::App) -> Result<(), String> {
@LISTENERS@    Ok(())
}

/// The first half of a stop: each wrap's `before_stop`, in order, then the
/// listener closes.
pub async fn stop_accepting(app: &crate::app::App, stopper: brocanth_server::Stopper) {
@STOPS@    stopper.stop();
}
"#;

/// The same three, with nothing to wrap.
const SERVING_PLAIN: &str = r#"/// What the server is handed for one request.
pub type Answer = brocanth::plugin_routes::BoxFuture<'static, brocanth_server::Response>;

/// What `serve` answers a request with: `next`, as it is. `brocanth add
/// observe` logs and counts each request here, and answers the probes.
pub fn wrap<F, Fut>(
    app: &std::sync::Arc<crate::app::App>,
    next: F,
) -> impl Fn(brocanth_server::Request) -> Answer + Send + Sync + 'static
where
    F: Fn(brocanth_server::Request) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = brocanth_server::Response> + Send + 'static,
{
    let _ = app;
    move |request| -> Answer { Box::pin(next(request)) }
}

/// No second listener: nothing here serves probes or metrics.
pub async fn side_listeners(app: &crate::app::App) -> Result<(), String> {
    let _ = app;
    Ok(())
}

/// The first half of a stop: the listener closes at once.
pub async fn stop_accepting(app: &crate::app::App, stopper: brocanth_server::Stopper) {
    let _ = app;
    stopper.stop();
}
"#;

/// `[server]` as a struct, so the framework's settings have a type with a
/// `Default`, as each plugin's settings do. The report and the readers
/// without a file start from it.
const SERVER_SETTINGS: &str = r#"/// `[server]`: the settings the framework reads itself. It declares them as
/// a module declares its own, so each has a flag, a variable and a line in
/// the settings report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSettings {
    /// Seconds a stop waits for open requests, then for the plugins.
    pub shutdown_grace_secs: u64,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            shutdown_grace_secs: crate::app::SHUTDOWN_GRACE.as_secs(),
        }
    }
}
"#;

/// `plugins::shutdown_grace` with the config plugin.
const GRACE_FROM_CONFIG: &str = r#"/// `[server] shutdown_grace_secs` from the files, the environment and the
/// flags, or [`SHUTDOWN_GRACE`](crate::app::SHUTDOWN_GRACE) when none sets it.
pub fn shutdown_grace() -> Result<std::time::Duration, String> {
    let config = config()?;
    let Some(value) = config
        .raw_section("server")
        .and_then(|server| server.get("shutdown_grace_secs"))
    else {
        return Ok(crate::app::SHUTDOWN_GRACE);
    };
    // An integer in a file, a flag or a variable; a string when a layer
    // could not type it.
    value
        .as_integer()
        .and_then(|secs| u64::try_from(secs).ok())
        .or_else(|| value.as_str().and_then(|secs| secs.trim().parse().ok()))
        .map(std::time::Duration::from_secs)
        .ok_or_else(|| {
            format!(
                "`[server] shutdown_grace_secs` is {}, and it must be a whole number of seconds\nhelp: set it to a whole number of seconds, such as 25",
                value.display()
            )
        })
}
"#;

/// `plugins::shutdown_grace` without it.
const GRACE_PLAIN: &str = r#"/// `[server] shutdown_grace_secs` from the environment and the flags, or
/// [`SHUTDOWN_GRACE`](crate::app::SHUTDOWN_GRACE) when neither sets it.
pub fn shutdown_grace() -> Result<std::time::Duration, String> {
    let server = server_settings(&given()?)?;
    Ok(std::time::Duration::from_secs(server.shutdown_grace_secs))
}
"#;

/// Whether anything declared a setting the binary reads itself: one that is
/// not a list, which only the file holds. True for every generated project,
/// because the framework declares `[server] shutdown_grace_secs`; a `Built`
/// made without [`contributions`] can still say no.
fn has_settings(built: &Built) -> bool {
    !declared(built).is_empty()
}

/// Every declared setting but the lists, as `(section, setting)`, in module
/// order.
fn declared(built: &Built) -> Vec<(&str, &Setting)> {
    built
        .settings()
        .filter(|(_, _, setting)| setting.kind != Kind::List)
        .map(|(_, section, setting)| (section, setting))
        .collect()
}

/// The sections that declared settings, in the order they were declared.
fn declared_sections<'a>(declared: &[(&'a str, &Setting)]) -> Vec<&'a str> {
    let mut sections: Vec<&str> = Vec::new();
    for (section, _) in declared {
        if !sections.contains(section) {
            sections.push(section);
        }
    }
    sections
}

/// The settings half of `src/plugins/mod.rs`: the list, the flag names after
/// `src/flags.rs`, the readers, and the report.
fn flags_rs(a: &Answers, built: &Built) -> String {
    let config = a.has("config");
    let declared = declared(built);
    let optional = declared.iter().any(|(_, setting)| setting.optional);
    let mut out = String::from(SETTING_STRUCT_HEAD);
    if config || optional {
        out.push_str(
            "    /// A value that means `None` as well as an empty one does.\n\
            \x20   pub none_as: Option<&'static str>,\n",
        );
    }
    out.push_str(if config {
        "    /// Whether a flag's value stays a string whatever it looks like.\n\
        \x20   pub text: bool,\n"
    } else {
        "    /// The environment variable that sets it.\n\
        \x20   pub env: &'static str,\n"
    });
    out.push_str(
        "}\n\
         \n\
         /// Every setting the framework and the modules declared. `brocanth add`\n\
         /// rewrites this list; `src/flags.rs` renames or hides a flag.\n\
         pub const SETTINGS: &[Setting] = &[",
    );
    // rustfmt folds a list of one struct onto the `&[`, one level shallower.
    let one = declared.len() == 1;
    let pad = if one { "" } else { "    " };
    if !one {
        out.push('\n');
    }
    for (section, setting) in &declared {
        let mut entry = format!(
            "Setting {{\n\
            \x20   path: \"{section}.{key}\",\n\
            \x20   help: {help:?},\n\
            \x20   flag: {flag},\n",
            key = setting.key,
            help = setting.full_help(),
            flag = setting.flag,
        );
        if config || optional {
            entry.push_str(&match setting.none_as {
                Some(alias) => format!("    none_as: Some({alias:?}),\n"),
                None => "    none_as: None,\n".to_string(),
            });
        }
        if config {
            let text = setting.kind == Kind::Text;
            entry.push_str(&format!("    text: {text},\n"));
        } else {
            entry.push_str(&format!(
                "    env: \"{}_{}__{}\",\n",
                a.ident().to_uppercase(),
                section.replace('-', "_").to_uppercase(),
                setting.key.replace('.', "__").to_uppercase(),
            ));
        }
        entry.push('}');
        for line in entry.lines() {
            out.push_str(pad);
            out.push_str(line);
            out.push('\n');
        }
        // The last line pushed is the `}`; a list of many ends it with `,`.
        if !one {
            out.pop();
            out.push_str(",\n");
        }
    }
    if one {
        out.pop();
        out.push_str("];\n\n");
    } else {
        out.push_str("];\n\n");
    }
    out.push_str(SETTING_GIVEN);
    out.push_str(SETTING_NAMES);
    out.push_str(if config {
        FLAG_NAMES_CONFIG
    } else {
        FLAG_NAMES_PLAIN
    });
    if config {
        out.push_str(REFUSE_OFF_FLAGS);
    }
    out.push_str(if config {
        GIVEN_HEAD_CONFIG
    } else {
        GIVEN_HEAD_PLAIN
    });
    out.push_str(GIVEN_FLAGS);
    if config {
        out.push_str(CONFIG_FN);
    }
    if a.has("commands") {
        out.push_str(&FLAG_ARGS.replace(
            "@REFUSE@",
            if config {
                "    refuse_off_flags().map_err(anyhow::Error::msg)?;\n"
            } else {
                ""
            },
        ));
    } else {
        out.push_str(&READ_LINE.replace(
            "@REFUSE@",
            if config {
                "    refuse_off_flags()?;\n"
            } else {
                ""
            },
        ));
    }

    // Each setting's default, from the plugin's own `Default`.
    out.push_str(
        "\n/// Each setting as the plugin's `Default` says, before any layer.\n\
         fn defaults() -> Vec<(&'static str, String)> {\n",
    );
    out.push_str(&field_list(&declared, |section| {
        format!("{}::default()", built.settings_type_of(section))
    }));
    out.push_str("}\n");

    if config {
        out.push_str(CONFIG_REPORT);
    } else {
        out.push_str(PARSED);
        if optional {
            out.push_str(OPTIONAL);
        }
        for section in declared_sections(&declared) {
            out.push_str(&section_settings(
                section,
                &built.settings_type_of(section),
                &declared,
            ));
        }
        out.push_str(
            "\n/// Each setting as the plugin holds it, after every layer.\n\
             fn merged(given: &[Given]) -> Result<Vec<(&'static str, String)>, String> {\n",
        );
        let list = field_list(&declared, |section| {
            format!("{}_settings(given)?", section.replace('-', "_"))
        });
        // The same list, handed back in an `Ok`.
        out.push_str(&list.replacen("    vec![", "    Ok(vec![", 1).replacen(
            "]\n",
            "])\n",
            list.matches("]\n").count().min(1),
        ));
        out.push_str("}\n");
        out.push_str(PLAIN_REPORT);
    }
    out
}

/// The body of `defaults` or `merged`: one binding per section, built by
/// `source`, and a `vec!` of `(path, Debug of the field)`.
fn field_list(declared: &[(&str, &Setting)], source: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    for section in declared_sections(declared) {
        out.push_str(&format!(
            "    let {} = {};\n",
            section.replace('-', "_"),
            source(section)
        ));
    }
    let entries: Vec<String> = declared
        .iter()
        .map(|(section, setting)| {
            let path = format!("\"{section}.{}\"", setting.key);
            let value = format!(
                "format!(\"{{:?}}\", {}.{})",
                section.replace('-', "_"),
                setting.key
            );
            // rustfmt's `fn_call_width` is 60, and a tuple obeys it.
            if path.len() + 2 + value.len() > 60 {
                format!("(\n            {path},\n            {value},\n        )")
            } else {
                format!("({path}, {value})")
            }
        })
        .collect();
    // rustfmt keeps a short one-element list on one line.
    let single = format!("    vec![{}]\n", entries.join(", "));
    if entries.len() == 1 && single.len() <= 4 + 60 + 7 {
        out.push_str(&single);
    } else if let [(section, setting)] = declared {
        // One tuple too long for a line: rustfmt hugs it to the brackets,
        // `vec![(` and `)]`, one level shallower than in a list of many.
        out.push_str(&format!(
            "    vec![(\n\
            \x20       \"{section}.{key}\",\n\
            \x20       format!(\"{{:?}}\", {ident}.{key}),\n\
            \x20   )]\n",
            key = setting.key,
            ident = section.replace('-', "_"),
        ));
    } else {
        out.push_str("    vec![\n");
        for entry in entries {
            out.push_str(&format!("        {entry},\n"));
        }
        out.push_str("    ]\n");
    }
    out
}

/// `<section>_settings`: a section's defaults, with what this run was given
/// applied in order.
fn section_settings(section: &str, ty: &str, declared: &[(&str, &Setting)]) -> String {
    let ident = section.replace('-', "_");
    let mine: Vec<&Setting> = declared
        .iter()
        .filter(|(s, _)| *s == section)
        .map(|(_, setting)| *setting)
        .collect();
    let mut out = format!(
        "\n/// `[{section}]`, as this binary runs it without a file: the plugin's\n\
         /// defaults, then the environment, then the flags.\n"
    );
    let signature =
        format!("pub fn {ident}_settings(given: &[Given]) -> Result<{ty}, String> {{\n");
    if signature.len() > 101 {
        out.push_str(&format!(
            "pub fn {ident}_settings(\n    given: &[Given],\n) -> Result<{ty}, String> {{\n"
        ));
    } else {
        out.push_str(&signature);
    }
    out.push_str(&format!(
        "    let mut settings = {ty}::default();\n\
        \x20   for given in given {{\n"
    ));
    let reader = |setting: &Setting| {
        if setting.optional {
            "optional"
        } else {
            "parsed"
        }
    };
    // A match with one arm and a wildcard is clippy's `single_match`.
    if let [only] = mine.as_slice() {
        out.push_str(&format!(
            "        if given.setting.path == \"{section}.{key}\" {{\n\
            \x20           settings.{key} = {read}(given)?;\n\
            \x20       }}\n",
            key = only.key,
            read = reader(only),
        ));
    } else {
        out.push_str("        match given.setting.path {\n");
        for setting in &mine {
            let arm = format!(
                "            \"{section}.{key}\" => settings.{key} = {read}(given)?,\n",
                key = setting.key,
                read = reader(setting),
            );
            if arm.trim_end().len() > 100 {
                out.push_str(&format!(
                    "            \"{section}.{key}\" => {{\n\
                    \x20               settings.{key} = {read}(given)?\n\
                    \x20           }}\n",
                    key = setting.key,
                    read = reader(setting),
                ));
            } else {
                out.push_str(&arm);
            }
        }
        out.push_str("            _ => {}\n        }\n");
    }
    out.push_str("    }\n    Ok(settings)\n}\n");
    out
}

/// `src/flags.rs`: the developer's half of the flags.
const FLAGS_RS: &str = r#"//! The command-line flags of this binary, and the one place to rename them.
//!
//! A module declares each of its settings once. From that the binary takes a
//! flag, `--<module>.<key>=VALUE`, an environment variable,
//! `$<APP>_<MODULE>__<KEY>`, and, with the `config` module, a key in
//! `config/<env>.toml`. `brocanth add` rewrites the list in
//! `src/plugins/mod.rs`. This file is yours: it is written when it is missing
//! and never again.

/// Whether a project with a config file reads the flags too.
///
/// Off: with the `config` module, the file and the environment are the
/// sources, and a flag is refused with a pointer to this line. On: a flag
/// outranks both, for one run. Without the `config` module the flags are
/// always read, and this is not.
#[allow(dead_code)]
pub const WITH_CONFIG: bool = false;

/// Flags under another name: the key as the module declares it, and the flag
/// this binary answers to instead, without `--`. An empty name hides the
/// flag.
///
/// ```text
/// ("sessions.cookie", "cookie"),
/// ("storage.root", ""),
/// ```
pub const RENAMES: &[(&str, &str)] = &[];
"#;

/// `src/commands/settings.rs`: the report, as a command.
const SETTINGS_COMMAND_RS: &str = r#"//! `settings` -- every settings layer on its own, then the merged values.
//!
//! Ordinary project code, like `serve`: rename it or delete it, and delete
//! its line in `src/commands/mod.rs`.

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Show each settings layer, lowest precedence first, then the merged values.
#[cmd]
#[derive(clap::Args)]
pub struct Settings {}

impl Command for Settings {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        print!(
            "{}",
            crate::plugins::settings_report().map_err(anyhow::Error::msg)?
        );
        Ok(())
    }
}
"#;

const SETTING_STRUCT_HEAD: &str = r#"/// One declared setting, by the key it sets.
pub struct Setting {
    /// The key, as `config/<env>.toml` spells it. It is also the flag's name,
    /// unless `src/flags.rs` renames it.
    pub path: &'static str,
    /// The line `--help` prints.
    pub help: &'static str,
    /// Whether a flag may set it. A secret has none: every user on the host
    /// can read a process's arguments.
    pub flag: bool,
"#;

const SETTING_GIVEN: &str = r#"/// One value this run was given, above the defaults.
pub struct Given {
    /// What it sets.
    pub setting: &'static Setting,
    /// Where it came from, as typed: `--cookie`, or `$MY_APP_SESSIONS__COOKIE`.
    pub from: String,
    /// The value, as typed. Empty is `None` for an `Option` setting.
    pub value: String,
}

/// The name each flag answers to after `src/flags.rs`, whether the flags are
/// read or not.
///
/// A rename of a key no flag sets, a name that is not spelled as a flag, and
/// two flags under one name each stop the binary: each would otherwise be a
/// flag that does nothing, or does something else.
fn names() -> Result<Vec<(&'static Setting, &'static str)>, String> {
    let flags = || SETTINGS.iter().filter(|setting| setting.flag);
    for (path, _) in crate::flags::RENAMES {
        if !flags().any(|setting| setting.path == *path) {
            let known: Vec<&str> = flags().map(|setting| setting.path).collect();
            return Err(format!(
                "src/flags.rs renames `{path}`, and no flag sets it\nhelp: the flags are {}",
                known.join(", ")
            ));
        }
    }
    let mut names: Vec<(&'static Setting, &'static str)> = Vec::new();
    for setting in flags() {
        let name = crate::flags::RENAMES
            .iter()
            .find(|(path, _)| *path == setting.path)
            .map_or(setting.path, |(_, name)| *name);
        if name.is_empty() {
            continue;
        }
        let spelled = name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
        let taken = ["help", "version", "settings"].contains(&name);
        if !spelled || name.starts_with(['-', '.', '_']) || taken {
            return Err(format!(
                "src/flags.rs names a flag `{name}`\nhelp: a flag is lower case, digits, `.`, `_` and `-`, starts with a letter or a digit, and is not `help`, `version` or `settings`"
            ));
        }
        if let Some((other, _)) = names.iter().find(|(_, taken)| *taken == name) {
            return Err(format!(
                "`--{name}` would set both `{}` and `{}`\nhelp: rename one of them in src/flags.rs",
                other.path, setting.path
            ));
        }
        names.push((setting, name));
    }
    Ok(names)
}

"#;

const SETTING_NAMES: &str = "";

const FLAG_NAMES_CONFIG: &str = r#"/// The flags this run reads: none unless `src/flags.rs` turns them on,
/// because with a config file the file is the source.
pub fn flag_names() -> Result<Vec<(&'static Setting, &'static str)>, String> {
    if !crate::flags::WITH_CONFIG {
        return Ok(Vec::new());
    }
    names()
}

"#;

const FLAG_NAMES_PLAIN: &str = r#"/// The flags this run reads: every one, because there is no file.
pub fn flag_names() -> Result<Vec<(&'static Setting, &'static str)>, String> {
    names()
}

"#;

const REFUSE_OFF_FLAGS: &str = r#"/// A flag given while the flags are off, refused by name with what to do
/// instead. Silently ignoring it would run with a value nobody chose.
pub fn refuse_off_flags() -> Result<(), String> {
    if crate::flags::WITH_CONFIG {
        return Ok(());
    }
    let names = names()?;
    for arg in std::env::args_os().skip(1) {
        let arg = arg.to_string_lossy();
        if arg == "--" {
            break;
        }
        let Some(written) = arg.strip_prefix("--") else {
            continue;
        };
        let name = written.split_once('=').map_or(written, |(name, _)| name);
        if let Some((setting, _)) = names.iter().find(|(_, known)| *known == name) {
            let (section, key) = setting.path.rsplit_once('.').unwrap_or(("", setting.path));
            return Err(format!(
                "`--{name}` is off: this project reads its settings from config/<env>.toml\nhelp: set WITH_CONFIG = true in src/flags.rs, or set `{key}` under [{section}] in config/<env>.toml"
            ));
        }
    }
    Ok(())
}

"#;

const GIVEN_HEAD_CONFIG: &str = r#"/// The flags this run was given, in the order typed. The environment is not
/// here: `brocanth-config` reads it, under the file and over the flags.
///
/// `--name=VALUE` and `--name VALUE`. Every other argument is left alone: it
/// is a command's, and the command line's own parser has already refused what
/// nobody declared.
pub fn given() -> Result<Vec<Given>, String> {
    let mut given = Vec::new();
"#;

const GIVEN_HEAD_PLAIN: &str = r#"/// What this run was given above the defaults, lowest first: each variable
/// that is set, then each flag in the order typed. A later value for one key
/// replaces an earlier one.
///
/// `--name=VALUE` and `--name VALUE`. Every other argument is left alone: it
/// is a command's, and the command line's own parser has already refused what
/// nobody declared.
pub fn given() -> Result<Vec<Given>, String> {
    let mut given = Vec::new();
    for setting in SETTINGS {
        if let Some(value) = std::env::var_os(setting.env) {
            given.push(Given {
                setting,
                from: format!("${}", setting.env),
                value: value.to_string_lossy().into_owned(),
            });
        }
    }
"#;

const GIVEN_FLAGS: &str = r#"    let names = flag_names()?;
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if arg == "--" {
            break;
        }
        let Some(written) = arg.strip_prefix("--") else {
            continue;
        };
        let (name, inline) = match written.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (written, None),
        };
        let Some((setting, name)) = names.iter().copied().find(|(_, known)| *known == name) else {
            continue;
        };
        let value = match inline {
            Some(value) => value,
            None => rest
                .next()
                .cloned()
                .ok_or_else(|| format!("--{name} needs a value\nhelp: write --{name}=VALUE"))?,
        };
        given.push(Given {
            setting,
            from: format!("--{name}"),
            value,
        });
    }
    Ok(given)
}
"#;

const CONFIG_FN: &str = r#"
/// The configuration this run reads: the files, the environment, and the
/// flags on top once `src/flags.rs` turns them on.
pub fn config() -> Result<brocanth_config::Config, String> {
    refuse_off_flags()?;
    let mut loader = brocanth_config::Loader::new(crate::app::APP);
    for setting in SETTINGS {
        if let Some(alias) = setting.none_as {
            loader = loader.none_as(setting.path, alias);
        }
    }
    for given in given()? {
        let flag = given.from.trim_start_matches('-');
        loader = loader.flag(given.setting.path, flag, &given.value, given.setting.text);
    }
    loader.load().map_err(|e| e.to_string())
}
"#;

const FLAG_ARGS: &str = r#"
/// The flags, as arguments every command takes.
///
/// A name a command already takes is refused here: `clap` panics on it in a
/// debug build and lets one of the two win in a release one.
pub fn flag_args(
    command: brocanth_command::clap::Command,
) -> brocanth_command::anyhow::Result<brocanth_command::clap::Command> {
    use brocanth_command::{anyhow, clap};

@REFUSE@    let mut command = command;
    for (setting, name) in flag_names().map_err(anyhow::Error::msg)? {
        let taken = command
            .get_subcommands()
            .flat_map(|sub| sub.get_arguments())
            .any(|arg| arg.get_long() == Some(name));
        if taken {
            anyhow::bail!(
                "`--{name}` sets `{}` and is also an argument of a command\nhelp: rename it in src/flags.rs",
                setting.path
            );
        }
        command = command.arg(
            clap::Arg::new(name)
                .long(name)
                .value_name("VALUE")
                .help(setting.help)
                .global(true)
                .action(clap::ArgAction::Append),
        );
    }
    Ok(command)
}
"#;

const READ_LINE: &str = r#"
/// Refuses anything on the command line that is not a flag, and answers
/// `--help` and `--settings`. `true` when it printed one of those, so `main`
/// stops there.
///
/// This binary has no command line of its own: `brocanth add commands` gives
/// it one, and `clap` does this.
pub fn read_line() -> Result<bool, String> {
@REFUSE@    let names = flag_names()?;
    let mut rest = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned());
    while let Some(arg) = rest.next() {
        if arg == "--help" || arg == "-h" {
            println!("Flags:");
            for (setting, name) in &names {
                println!("  --{name} <VALUE>  {}", setting.help);
            }
            println!("  --settings  every settings layer, then the merged values");
            return Ok(true);
        }
        if arg == "--settings" {
            print!("{}", settings_report()?);
            return Ok(true);
        }
        let written = arg.strip_prefix("--").unwrap_or_default();
        let name = written.split_once('=').map_or(written, |(name, _)| name);
        if name.is_empty() || !names.iter().any(|(_, known)| *known == name) {
            return Err(format!(
                "unknown argument `{arg}`\nhelp: `--help` lists the flags"
            ));
        }
        // `--name VALUE`: the next argument is the value, not a flag.
        if !written.contains('=') {
            rest.next();
        }
    }
    Ok(false)
}
"#;

const CONFIG_REPORT: &str = r#"
/// Every settings layer on its own, lowest first, then the merged values.
/// A secret is masked in every layer.
pub fn settings_report() -> Result<String, String> {
    let config = config()?;
    let mut layers = config.layers().defaults(defaults());
    for setting in SETTINGS.iter().filter(|setting| !setting.flag) {
        layers = layers.secret(setting.path);
    }
    Ok(layers.to_string())
}
"#;

const PARSED: &str = r#"
/// One value, as the type of the field it sets.
fn parsed<T>(given: &Given) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    given
        .value
        .parse()
        .map_err(|e| format!("{} is {:?}: {e}", given.from, given.value))
}
"#;

const OPTIONAL: &str = r#"
/// One value for an `Option` field: empty, or the setting's declared alias,
/// is `None`, decided at this layer.
fn optional<T>(given: &Given) -> Result<Option<T>, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
{
    if given.value.is_empty() || given.setting.none_as == Some(given.value.as_str()) {
        Ok(None)
    } else {
        parsed(given).map(Some)
    }
}
"#;

const PLAIN_REPORT: &str = r#"
/// Every settings layer on its own, lowest first, then the merged values.
/// A secret is masked in every layer.
pub fn settings_report() -> Result<String, String> {
    let given = given()?;
    let secret = |path: &str| SETTINGS.iter().any(|s| s.path == path && !s.flag);
    let shown = |path: &str, value: String| {
        if secret(path) {
            "(secret)".to_string()
        } else {
            value
        }
    };
    let width = SETTINGS.iter().map(|s| s.path.len()).max().unwrap_or(0);
    let mut out = String::from("layers, lowest precedence first\n\n   1  defaults\n");
    for (path, value) in defaults() {
        out.push_str(&format!(
            "        {path:<width$}  = {}\n",
            shown(path, value)
        ));
    }
    for (n, label, prefix) in [(2, "environment", "$"), (3, "flags", "--")] {
        out.push_str(&format!("\n  {n:>2}  {label}\n"));
        let mine: Vec<&Given> = given
            .iter()
            .filter(|g| g.from.starts_with(prefix))
            .collect();
        if mine.is_empty() {
            out.push_str("        (sets nothing)\n");
        }
        for g in mine {
            let value = shown(g.setting.path, format!("{:?}", g.value));
            let path = g.setting.path;
            out.push_str(&format!("        {path:<width$}  = {value}  {}\n", g.from));
        }
    }
    out.push_str("\nmerged\n");
    for (path, value) in merged(&given)? {
        let layer = match given.iter().rev().find(|g| g.setting.path == path) {
            Some(g) if g.from.starts_with('$') => "2 environment",
            Some(_) => "3 flags",
            None => "1 defaults",
        };
        let value = shown(path, value);
        out.push_str(&format!(
            "  {path:<w$}  = {value}  {layer}\n",
            w = width + 6
        ));
    }
    Ok(out)
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
        env = DEFAULT_ENV,
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

    #[test]
    fn the_first_config_file_is_the_one_the_loader_reads_by_default() {
        assert_eq!(DEFAULT_ENV, brocanth_config::DEFAULT_ENV);
    }

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
    /// Every module that can stand together. `logging` is the one module
    /// `observability-logs` replaces, and `static` refuses every module that
    /// needs a server, so neither is in it, nor `beacon`, which counts what
    /// `analytics` counts. `available()` is not a project:
    /// the generator refuses it.
    fn everything() -> Vec<&'static str> {
        available()
            .into_iter()
            .filter(|key| !["logging", "static", "beacon"].contains(key))
            .collect()
    }

    fn shapes() -> Vec<(&'static str, Vec<&'static str>)> {
        vec![
            ("core-only", vec![]),
            // A plugin and no config file. Its own shape because the
            // literal branch of every plugin module runs only here, and
            // for as long as it was unsnapshotted it did not compile.
            ("plugin-no-config", vec!["logging"]),
            // Declared settings and no config file: the flags are the only
            // source, read by `main` with no command line, and by `clap`
            // with one.
            ("flags", vec!["sessions", "storage"]),
            ("flags-commands", vec!["sessions", "commands"]),
            ("default", DEFAULT.to_vec()),
            // Everything that can stand together: `logging` is the one
            // module `observe` replaces.
            ("everything", everything()),
            // The pages as files, with only what runs outside a server.
            (
                "static",
                vec![
                    "static",
                    "config",
                    "commands",
                    "rsx",
                    "rsx-tailwind",
                    "rsx-htmx",
                    "ci",
                    "ci-github",
                    "gtag",
                ],
            ),
            // `robots` has no shape here: its own crate generates its two
            // shapes through `brocanth_cli::testing` and keeps the goldens
            // in `crates/brocanth-robots/tests/golden/`.
            // The consent banner and the tag, with the policy that allows
            // them and no config file: the literal branch of a hooking
            // plugin, and the one adjustment a second module makes to the
            // security headers.
            (
                "gtag",
                vec!["middleware", "middleware-security-headers", "gtag"],
            ),
            // Translations with the consent banner and rsx, and no config
            // file: a `build.rs`, a `[build-dependencies]` table, the
            // banner's keys in the catalogs and `gtag`'s `i18n` feature.
            ("i18n", vec!["rsx", "gtag", "i18n"]),
            // With a config file and jt: the `[i18n]` section.
            ("i18n-config", vec!["config", "jt", "i18n"]),
            // A plugin that builds everything it provides in `configure`,
            // and the one module whose settings carry a string the plugin
            // has to translate. No config file either, so the literal
            // branch of both is here.
            ("realtime", vec!["realtime"]),
            // The broker's transport: a stream over a mailbox, and a page
            // that authorises its topics. With a config file, so the
            // broker's limits come from `[realtime]`.
            ("realtime-sse", vec!["config", "realtime", "realtime-sse"]),
            // The other transport: an upgrade, a loop over one socket, and
            // a page that checks the Origin. Its server line gains the
            // `websocket` feature.
            (
                "realtime-websocket",
                vec!["config", "realtime", "realtime-websocket"],
            ),
            // The queue, with a config file, which is the shape that opens
            // a pool and starts a worker from a TOML section.
            ("jobs", vec!["config", "jobs"]),
            // The pool, with a config file: a `[db]` section, a plugin that
            // opens it in `before_start`, and `ctx.db()` on the context.
            ("db", vec!["config", "db"]),
            // The pool with a command line and no ORM: a `migrate` command
            // over the SQL files in migrations/, reading `[db]` from the file.
            ("db-migrate", vec!["config", "db", "commands"]),
            // The same with no config file: the URL comes from the flags and
            // the environment.
            ("db-migrate-flags", vec!["db", "commands"]),
            // Page views over that pool, with the admin, so the generated
            // `mount` carries the analytics tab and the hook record names a
            // plugin from a library crate.
            ("analytics", vec!["config", "db", "analytics", "admin"]),
            // Its static side: a site exported to files whose pages carry
            // the beacon snippet.
            ("beacon", vec!["static", "config", "commands", "beacon"]),
            // The ORM with no command line: no `migrate` command, and the
            // `migrate` tool in `src/bin/migrate.rs` still builds and runs
            // on its own.
            ("model", vec!["config", "db", "model"]),
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

    /// The golden shapes, built and tested for real, against this checkout.
    ///
    /// A snapshot proves the generator wrote what it wrote yesterday, not
    /// that what it wrote compiles. Minutes, not milliseconds:
    /// `cargo test -p brocanth-cli --bin brocanth -- --ignored golden_shapes`.
    /// `BROCANTH_SHAPES=default,admin` narrows it to those shapes. One
    /// target directory serves every shape, so the dependencies build once.
    #[test]
    #[ignore = "runs cargo build and cargo test over generated applications"]
    fn the_golden_shapes_build_and_pass_their_own_tests() {
        let brocanth = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let wanted = std::env::var("BROCANTH_SHAPES").ok();
        let base = std::env::temp_dir().join(format!("brocanth-shapes-{}", std::process::id()));
        // Each shape has a target directory of a few gigabytes. The guard
        // deletes them when the test ends, passed or failed, so a failed run
        // cannot fill the disk. `BROCANTH_SHAPES_KEEP=1` keeps them to debug.
        let _cleanup = TempDir(base.clone());
        let target = base.join("target");
        let mut failed = Vec::new();
        for (name, picks) in shapes() {
            if wanted
                .as_deref()
                .is_some_and(|list| !list.split(',').any(|w| w == name))
            {
                continue;
            }
            let root = base.join(name);
            let _ = std::fs::remove_dir_all(&root);
            let a = Answers {
                local: Some(brocanth.display().to_string()),
                ..answers(&picks)
            };
            for (path, contents) in scaffold(&a).expect("the modules agree") {
                let full = root.join(&path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, contents).unwrap();
            }
            // Clippy as the project's own CI runs it, then its tests, then
            // once more with a page that awaits and a page that streams: the
            // generator writes only sync pages, and those two are what a
            // developer adds next. The new pages change the route table, so
            // after them only the streaming page's own test runs.
            //
            // With a database, a third page needs it, and its test runs with
            // no database reachable: the build must finish on the
            // current-thread runtime of `#[tokio::test]`, and the page must
            // fail rather than hang. Every URL points at a closed port, and
            // the pool gives up after a second.
            let lint = &["clippy", "--all-targets", "--", "-D", "warnings"][..];
            let has_db = picks.contains(&"db");
            let streams = if has_db {
                &["test", "--", "pages::ticks", "pages::pinged"][..]
            } else {
                &["test", "--", "pages::ticks"][..]
            };
            // A project with `db` and no config file reads `[db]` from the
            // environment: the same closed port and short timeout.
            let closed_db: &[(&str, &str)] = if has_db && !picks.contains(&"config") {
                &[
                    ("MY_BLOG_DB__URL", "postgres://brocanth@127.0.0.1:1/none"),
                    ("MY_BLOG_DB__ACQUIRE_TIMEOUT", "1"),
                ]
            } else {
                &[]
            };
            for step in [lint, &["test"][..], &["async-page"][..], lint, streams] {
                if step == ["async-page"] {
                    std::fs::write(root.join("src/pages/waits.rs"), ASYNC_PAGE).unwrap();
                    std::fs::write(root.join("src/pages/ticks.rs"), STREAM_PAGE).unwrap();
                    if has_db {
                        std::fs::write(root.join("src/pages/pinged.rs"), DB_PAGE).unwrap();
                        // Without a config file, the same values come from
                        // the environment; see `closed_db` above.
                        let config = root.join("config/development.toml");
                        let Ok(text) = std::fs::read_to_string(&config) else {
                            continue;
                        };
                        let text = text
                            .replace(
                                "\"postgres://localhost/development\"",
                                "\"postgres://brocanth@127.0.0.1:1/none\"",
                            )
                            .replace("acquire_timeout = 30", "acquire_timeout = 1");
                        std::fs::write(&config, text).unwrap();
                    }
                    // Cargo cannot see a new page, which is documented; the
                    // table it lives in is rewritten so the build notices.
                    let table = root.join("src/pages/mod.rs");
                    let text = std::fs::read(&table).unwrap();
                    std::fs::write(&table, text).unwrap();
                    continue;
                }
                let output = std::process::Command::new(
                    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()),
                )
                .args(step)
                .env("CARGO_TARGET_DIR", &target)
                .envs(closed_db.iter().copied())
                .current_dir(&root)
                .output()
                .expect("run cargo");
                if !output.status.success() {
                    // Both streams: cargo writes to stderr, and a failing
                    // test writes its panic to stdout.
                    failed.push(format!(
                        "{name}: cargo {}\n{}\n{}",
                        step.join(" "),
                        String::from_utf8_lossy(&output.stderr),
                        String::from_utf8_lossy(&output.stdout)
                    ));
                    break;
                }
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n\n"));
    }

    /// Deletes a directory when dropped, a panic included, unless
    /// `BROCANTH_SHAPES_KEEP` is set.
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            if std::env::var_os("BROCANTH_SHAPES_KEEP").is_some() {
                eprintln!("kept {}", self.0.display());
                return;
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A page that holds the context across an `.await`.
    const ASYNC_PAGE: &str = r#"//! `/waits`.

use crate::pages::{Ctx, Error, IntoResponse};

pub async fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let before = ctx.path.clone();
    tokio::task::yield_now().await;
    Ok(format!("{before} {}", ctx.path))
}
"#;

    /// A page that answers with server-sent events from a channel, and a test
    /// that sends a request through the whole application -- the hooks, the
    /// table, `sent` -- and reads the stream that comes out.
    const STREAM_PAGE: &str = r#"//! `/ticks`.

use brocanth_server::sse::{self, Event};

use crate::pages::{Ctx, Error, IntoResponse, Response};

pub async fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let (events, stream) = sse::channel(4);
    tokio::spawn(async move {
        for n in 1..=3 {
            let tick = Event::new(n.to_string()).with_event("tick");
            if events.send(tick).await.is_err() {
                return;
            }
        }
    });
    Ok(Response::events(stream))
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::{Body, Request};

    #[tokio::test]
    async fn the_page_streams_its_events_through_the_application() {
        let app = crate::app::build().expect("the application builds");
        let response = crate::app::handle(&app, Request::new(Verb::Get, "/ticks")).await;
        assert_eq!(response.status, 200);
        assert_eq!(response.header("content-type"), Some("text/event-stream"));
        assert_eq!(response.header("cache-control"), Some("no-cache"));
        let Body::Stream(mut stream) = response.body else {
            panic!("the page answered with a stream");
        };
        let mut seen = String::new();
        while let Some(chunk) = std::future::poll_fn(|cx| stream.poll_chunk(cx)).await {
            seen.push_str(std::str::from_utf8(&chunk.expect("no error")).expect("utf-8"));
        }
        assert_eq!(
            seen,
            "event:tick\ndata:1\n\nevent:tick\ndata:2\n\nevent:tick\ndata:3\n\n"
        );
    }
}
"#;

    /// A page that needs the database, and a test that builds the whole
    /// application on `#[tokio::test]`'s current-thread runtime with no
    /// database reachable. The build waits for no I/O, so it finishes; the
    /// page fails with a 500 and does not hang; the startup gate names the
    /// pool with its `help:` line.
    const DB_PAGE: &str = r#"//! `/pinged`.

use crate::pages::{Ctx, Error, IntoResponse};

pub async fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    ctx.db().ping().await?;
    Ok("pong")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use brocanth::routing::Verb;
    use brocanth_server::Request;

    #[tokio::test]
    async fn with_no_database_the_build_finishes_and_the_page_fails() {
        let app = crate::app::build().expect("the application builds");
        let answer = tokio::time::timeout(
            Duration::from_secs(20),
            crate::app::handle(&app, Request::new(Verb::Get, "/pinged")),
        )
        .await
        .expect("the request ends");
        assert!(matches!(answer.status, 500 | 503), "{}", answer.status);
        let failed = tokio::time::timeout(Duration::from_secs(20), app.core.readiness().check_all())
            .await
            .expect("the gate ends")
            .expect_err("no database answers");
        assert!(failed.to_string().contains("help:"), "{failed}");
    }
}
"#;

    /// Everything a shape writes. The modules have to agree for a project
    /// to exist at all, so a test that cannot get this far has already
    /// found the bug.
    /// A module that asks for one node package, for the `package.json`
    /// tests.
    struct Node(&'static str, &'static str, &'static str);

    impl brocanth_scaffold::Module for Node {
        fn manifest(&self) -> brocanth_scaffold::Manifest {
            brocanth_scaffold::Manifest::new(self.0, "a node package")
                .category(brocanth_scaffold::Category::External)
                .outside_runtime()
        }

        fn contribute(&self, _: &brocanth_scaffold::Project, s: &mut Built) {
            brocanth_scaffold::snippet::node_dependency(s, self.1, self.2);
        }
    }

    const LINT: Node = Node("node-lint", "eslint", "^9.0.0");
    const LINT_AGAIN: Node = Node("node-lint-again", "eslint", "^9.0.0");
    const LINT_OLD: Node = Node("node-lint-old", "eslint", "^8.0.0");

    #[test]
    fn two_modules_that_need_node_share_one_package_json() {
        let files = crate::modules::with_extra(&[&LINT, &LINT_AGAIN], || {
            written(&[
                "rsx",
                "rsx-tailwind",
                "commands",
                "node-lint",
                "node-lint-again",
            ])
        });
        let package = &files
            .iter()
            .find(|(path, _)| path == "package.json")
            .expect("package.json")
            .1;
        assert!(
            package.contains(
                "  \"devDependencies\": {\n    \"@tailwindcss/cli\": \"^4.1.14\",\n    \
                 \"eslint\": \"^9.0.0\"\n  }"
            ),
            "sorted, and one line for a package two modules name:\n{package}"
        );
    }

    #[test]
    fn two_versions_of_one_node_package_are_refused_by_name() {
        let wrong = crate::modules::with_extra(&[&LINT, &LINT_OLD], || {
            scaffold(&answers(&["node-lint", "node-lint-old"]))
        })
        .expect_err("two versions of eslint");
        let text = wrong.iter().map(|d| d.message.clone()).collect::<String>();
        assert!(
            text.contains("node-lint") && text.contains("node-lint-old") && text.contains("eslint"),
            "{text}"
        );
        assert!(wrong
            .iter()
            .any(|d| d.notes.iter().any(|(kind, _)| *kind == "help")));
    }

    #[test]
    fn no_node_module_no_package_json() {
        assert!(!paths(&["rsx"]).contains(&"package.json".to_string()));
    }

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
    fn a_module_that_derives_serde_gets_it_once_with_or_without_config() {
        // The generated job derives serde. It used to arrive only with
        // `config`, so `--with jobs` alone did not compile.
        let count = |manifest: &str| {
            manifest
                .lines()
                .filter(|l| l.starts_with("serde = "))
                .count()
        };
        let alone = cargo_toml(&answers(&["jobs"]), &built(&["jobs"]));
        assert_eq!(count(&alone), 1, "{alone}");
        assert!(alone.contains("features = [\"derive\"]"), "{alone}");
        assert!(!alone.contains("nothing here is deserialised"), "{alone}");

        let configured = cargo_toml(&answers(&["config", "jobs"]), &built(&["config", "jobs"]));
        assert_eq!(count(&configured), 1, "{configured}");
    }

    /// `jobs` without `config`, built for real: the case the unit test above
    /// guards, through cargo. `cargo test -p brocanth-cli -- --ignored`.
    #[test]
    #[ignore = "runs cargo build over a generated application"]
    fn jobs_compiles_without_a_config_file() {
        let brocanth = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-jobs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = Answers {
            name: "jobs-test".into(),
            title: "Jobs Test".into(),
            picks: vec!["jobs"],
            local: Some(brocanth.display().to_string()),
            git: false,
        };
        for (path, contents) in scaffold(&a).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["build", "--all-targets"])
            .current_dir(&root)
            .output()
            .expect("run cargo");
        assert!(
            output.status.success(),
            "jobs without config did not compile:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let _ = std::fs::remove_dir_all(&root);
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
        let everything = cargo_toml(&answers(&everything()), &built(&everything()));
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
            ".plugin_hooking_from_config::<crate::plugins::logging::Logging>(\"logging\")",
            ".plugin_from_config::<crate::plugins::sessions::Sessions>(\"sessions\")",
        ] {
            assert!(registry.contains(line), "{registry}");
        }
        let main = app_rs(&a, &built(&picks));
        assert!(!main.contains("plugin_from_config"), "{main}");
        // The mount is in the application's setup block, never in the plugin
        // and never in the generated file: a pattern is a decision.
        assert!(
            main.contains("setup.router().mount(\"/users/{*rest}\", \"session\")"),
            "{main}"
        );
        assert!(!registry.contains(".mount("), "{registry}");
        let plugin = crate::modules::sessions::plugin_rs(&project(&["sessions"]));
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
        let registry = plugins_mod_rs(&answers(&["sessions"]), &built(&["sessions"]));
        // Already wrapped, because it is longer than rustfmt's chain_width
        // and a project must survive its own first `cargo fmt`. The literal
        // is the default, with the environment and the flags on top.
        assert!(
            registry.contains(".plugin(crate::plugins::sessions::Sessions::new(\n")
                && registry.contains("crate::plugins::sessions_settings(&given)?,")
                && registry.contains("crate::plugins::sessions::Settings::default();"),
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
        for picks in [vec!["config"], vec!["config", "logging"], everything()] {
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

        // And the opposite end: with everything that can stand together
        // chosen, the brief names only the two modules that cannot, and
        // invents no other gap.
        let everything = written(&everything());
        let full = &everything
            .iter()
            .find(|(p, _)| p == "AGENTS.md")
            .expect("an agent brief")
            .1;
        assert!(
            full.contains("**Absent:** `logging`, `beacon`, `static` —"),
            "{full}"
        );
    }

    #[test]
    fn the_table_is_the_directory_and_names_nothing() {
        // There is no list of routes to go out of step, because there is no
        // list: `pages!` reads the directory. So what this checks is that
        // nothing crept back in.
        let a = answers(&everything());
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
        let built = contributions(&a).expect("the modules agree");
        assert!(!main_with_commands_rs(&a, &built, false).contains("mod views;"));
    }

    #[test]
    fn the_first_server_wrap_is_the_outermost_and_after_puts_one_inside() {
        // `rsx` comes first in this list and declares
        // `after("observability")`, so the observer is the outer wrap and
        // still logs and counts a static file.
        let a = answers(&["rsx", "rsx-htmx", "observability"]);
        let built = contributions(&a).expect("the modules agree");
        let text = plugins_mod_rs(&a, &built);
        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("no `{needle}` in:\n{text}"))
        };
        // `next` is wrapped innermost first, so the outer wrap comes last.
        assert!(
            at("<static_files::StaticFiles as ServerWrap>::wrap")
                < at("<observability::serving::Serving as ServerWrap>::wrap")
        );
        // The stop runs outermost first: `/readyz` fails before anything else.
        assert!(
            at("<observability::serving::Serving as ServerWrap>::before_stop")
                < at("<static_files::StaticFiles as ServerWrap>::before_stop")
        );
        // Without a wrap, no trait: the plain functions, as before the slot.
        let plain = plugins_mod_rs(&answers(&[]), &contributions(&answers(&[])).unwrap());
        assert!(!plain.contains("trait ServerWrap"), "{plain}");
    }

    #[test]
    fn a_renderer_with_static_files_leaves_src_app_rs_alone() {
        // `src/app.rs` is the developer's. A module that changed it made
        // `brocanth add` refuse once the developer had edited it.
        for picks in [
            &["rsx"][..],
            &["rsx", "rsx-tailwind", "rsx-htmx", "commands"][..],
        ] {
            let a = answers(picks);
            let built = contributions(&a).expect("the modules agree");
            assert_eq!(
                app_rs(&a, &built),
                app_rs(&answers(&[]), &built),
                "{picks:?}"
            );
        }
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
        let bad =
            orphans(&["rsx-tailwind"], Door::Create).expect_err("tailwind alone is not a project");
        let text = format!("{bad:?}");
        assert!(text.contains("part of"), "{text}");
        assert!(
            text.contains("--with rsx,rsx-tailwind"),
            "the fix is the message: {text}"
        );

        orphans(&["rsx", "rsx-tailwind", "rsx-htmx"], Door::Create)
            .expect("together they are fine");
        orphans(&DEFAULT, Door::Create).expect("the default set has no children in it");
    }

    #[test]
    fn every_child_alone_is_refused_at_every_door_and_the_generator() {
        // A child without its parent wrote a project that did not build when
        // a caller reached `scaffold` without the flag checks. Every door,
        // and the generator itself, now refuses it and names the parent.
        let children: Vec<_> = crate::modules::ALL
            .iter()
            .map(|m| m.manifest())
            .filter(|m| m.parent.is_some() && m.blocked.is_none())
            .collect();
        assert!(!children.is_empty());
        for child in &children {
            let parent = child.parent.unwrap();
            let alone = [child.key];
            for door in [Door::Create, Door::Add] {
                let bad = selection(&alone, door).expect_err(child.key);
                let text = format!("{bad:?}");
                assert!(
                    text.contains(parent),
                    "`{}` names its parent: {text}",
                    child.key
                );
            }
            let add = format!("{:?}", orphans(&alone, Door::Add).unwrap_err());
            assert!(
                add.contains(&format!("brocanth add {parent} {}", child.key)),
                "`add` says what to type at `add`: {add}"
            );
            scaffold(&answers(&alone)).expect_err("the generator refuses an orphan too");

            // With its parent and everything either needs, the selection
            // holds. The cheap half of "builds": the generator accepts it.
            // Close over needs and parents: a need can itself be a child.
            let mut with: Vec<&'static str> = vec![child.key];
            let mut i = 0;
            while i < with.len() {
                let m = about(with[i]);
                for key in m.needs.iter().map(|(need, _)| *need).chain(m.parent) {
                    if !with.contains(&key) {
                        with.push(key);
                    }
                }
                i += 1;
            }
            let with: Vec<&'static str> = all().into_iter().filter(|k| with.contains(k)).collect();
            selection(&with, Door::Create)
                .unwrap_or_else(|e| panic!("`{}` with {with:?}: {e:?}", child.key));
        }
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
        let bad = unmet(&["rsx", "rsx-tailwind"], Door::Create)
            .expect_err("tailwind needs a command line");
        let text = format!("{bad:?}");
        assert!(text.contains("rsx-tailwind"), "{text}");
        assert!(text.contains("commands"), "{text}");

        unmet(&["rsx", "rsx-tailwind", "commands"], Door::Create).expect("together they are fine");
        unmet(&DEFAULT, Door::Create).expect("the default shape stands on its own");
        for (name, picks) in shapes() {
            unmet(&picks, Door::Create)
                .unwrap_or_else(|_| panic!("the `{name}` shape is not a project"));
        }
    }

    #[test]
    fn the_serve_task_starts_the_server_with_or_without_a_command_line() {
        // With `commands` the binary answers no subcommand with its help, so
        // a task that ran a bare `cargo run` printed the help and served
        // nothing. The task has to name the command the scaffold wrote.
        for (name, picks) in shapes()
            .into_iter()
            .chain([("nothing", Vec::new()), ("commands", vec!["commands"])])
        {
            let a = answers(&picks);
            let built = contributions(&a).expect("a project");
            let mise = mise_toml(&built, a.has("commands"));
            let task: toml_edit::DocumentMut = mise.parse().expect("mise.toml parses");
            let run = task["tasks"]["serve"]["run"]
                .as_str()
                .expect("a serve task");
            if a.has("commands") {
                assert_eq!(run, "cargo run -- serve", "the `{name}` shape");
            } else {
                assert_eq!(run, "cargo run", "the `{name}` shape");
            }
        }
    }

    #[test]
    fn every_generated_toml_line_starts_in_the_first_column() {
        // A stray indent in an assembled string is valid TOML and still
        // wrong: the reader cannot tell a continuation from a mistake.
        for a in [answers(&["rsx", "rsx-tailwind", "commands"]), answers(&[])] {
            let built = contributions(&a).expect("a project");
            for line in mise_toml(&built, a.has("commands")).lines() {
                assert_eq!(line, line.trim_start(), "indented: {line:?}");
            }
        }
    }

    #[test]
    fn a_module_this_build_cannot_make_is_refused_before_anything_is_written() {
        // The flags refuse a blocked module by name, with its reason, so
        // nothing can write half a project. No module in this build is
        // blocked since `realtime-websocket` was unblocked; the loop holds
        // the next one to the rule, and the TUI tests hold the menu to it
        // with a fixture.
        for key in all() {
            let Some(why) = about(key).blocked else {
                continue;
            };
            let flagged = unavailable(&[key]).expect_err("a blocked module");
            assert!(
                format!("{flagged:?}").contains(why),
                "the refusal of {key} does not say why: {flagged:?}"
            );
        }
        assert!(unavailable(&["realtime", "realtime-websocket"]).is_ok());
    }

    #[test]
    fn nothing_the_menu_cannot_make_is_listed_as_merely_absent() {
        // The receipt means "you did not ask for this". A module nobody
        // could have asked for does not belong in it, in either file.
        let core = cargo_toml(&answers(&[]), &built(&[]));
        let brief = agents_md(&answers(&[]), &built(&[]));
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
            questions: false,
            json: false,
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

    /// The static export writes `/robots.txt` as a file, open, because the
    /// export is the published site.
    #[test]
    #[ignore = "runs cargo run over a generated application"]
    fn the_static_export_writes_robots_txt() {
        let brocanth = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-robots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = Answers {
            local: Some(brocanth.display().to_string()),
            ..answers(&["static", "config", "commands", "robots"])
        };
        for (path, contents) in scaffold(&a).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["run", "-q", "--", "export"])
            .current_dir(&root)
            .output()
            .expect("run cargo");
        assert!(
            output.status.success(),
            "the export failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let written = std::fs::read_to_string(root.join("dist/robots.txt"))
            .expect("the export wrote dist/robots.txt");
        assert_eq!(written, "User-agent: *\nDisallow:\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The static export writes `/sitemap.xml` as a file, and robots.txt
    /// names it.
    #[test]
    #[ignore = "runs cargo run over a generated application"]
    fn the_static_export_writes_sitemap_xml() {
        let brocanth = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-sitemap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = Answers {
            local: Some(brocanth.display().to_string()),
            ..answers(&["static", "config", "commands", "robots", "sitemap"])
        };
        for (path, contents) in scaffold(&a).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["run", "-q", "--", "export"])
            .current_dir(&root)
            .output()
            .expect("run cargo");
        assert!(
            output.status.success(),
            "the export failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let robots = std::fs::read_to_string(root.join("dist/robots.txt"))
            .expect("the export wrote dist/robots.txt");
        assert!(
            robots.ends_with("Sitemap: http://localhost:3000/sitemap.xml\n"),
            "{robots}"
        );
        let xml = std::fs::read_to_string(root.join("dist/sitemap.xml"))
            .expect("the export wrote dist/sitemap.xml");
        assert!(xml.contains("<loc>http://localhost:3000/</loc>"), "{xml}");
        assert!(
            xml.contains("<loc>http://localhost:3000/about</loc>"),
            "{xml}"
        );
        assert!(!xml.contains("users"), "{xml}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The beacon reaches the exported files once a collector is set: the
    /// export runs each page through the hooks, the beacon's included.
    #[test]
    #[ignore = "runs cargo build and the export over a generated application"]
    fn the_static_export_carries_the_beacon() {
        let brocanth = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let root = std::env::temp_dir().join(format!("brocanth-beacon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let a = Answers {
            local: Some(brocanth.display().to_string()),
            ..answers(&["static", "config", "commands", "beacon"])
        };
        for (path, contents) in scaffold(&a).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        let config = root.join("config/development.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        let text = text.replace(
            "# collector = \"https://app.example.com/_a/v\"",
            "collector = \"https://app.example.com/_a/v\"",
        );
        std::fs::write(&config, text).unwrap();
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .args(["run", "-q", "--", "export"])
            .current_dir(&root)
            .output()
            .expect("run cargo");
        assert!(
            output.status.success(),
            "the export failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let index = std::fs::read_to_string(root.join("dist/index.html"))
            .expect("the export wrote dist/index.html");
        assert!(
            index.contains("<script data-collector=\"https://app.example.com/_a/v\">"),
            "{index}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The flags a module declares, through a generated binary, both ways.
    ///
    /// Each run is a value the plugin refuses at startup, so the binary
    /// stops before it binds a socket, and the error proves the flag reached
    /// the plugin's settings. Without `config`, `main` reads the line itself.
    /// With it, a renamed flag reaches the file's loader as its top layer
    /// once `src/flags.rs` turns the flags on, and the canonical name is then
    /// refused by `clap`.
    ///
    /// `cargo test -p brocanth-cli -- --ignored`.
    #[test]
    #[ignore = "runs cargo build over a generated application"]
    fn declared_flags_reach_the_plugin_with_and_without_a_config_file() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let brocanth = crates
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let build = |name: &str, picks: Vec<&'static str>, flags_rs: Option<&str>| {
            let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let a = Answers {
                name: name.into(),
                title: "Flags Test".into(),
                picks,
                local: Some(brocanth.display().to_string()),
                git: false,
            };
            for (path, contents) in scaffold(&a).expect("the modules agree") {
                let full = root.join(&path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, contents).unwrap();
            }
            if let Some(text) = flags_rs {
                std::fs::write(root.join("src/flags.rs"), text).unwrap();
            }
            let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .args(["build", "--quiet"])
                .current_dir(&root)
                .output()
                .expect("run cargo");
            assert!(
                output.status.success(),
                "{name} did not compile:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            root
        };
        let run = |root: &Path, name: &str, args: &[&str], env: &[(&str, &str)]| {
            let output = Command::new(root.join("target/debug").join(name))
                .args(args)
                .envs(env.iter().copied())
                .current_dir(root)
                .output()
                .expect("run the binary");
            (
                output.status.success(),
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
            )
        };

        // No config file and no command line: `main` reads the flags, and
        // the plugins read the environment under them.
        let bare = build("flags-bare", vec!["sessions", "storage"], None);
        let (ok, out) = run(&bare, "flags-bare", &["--sessions.cookie="], &[]);
        assert!(!ok && out.contains("cookie must not be empty"), "{out}");
        let (ok, out) = run(&bare, "flags-bare", &["--storage.token_ttl", "abc"], &[]);
        assert!(
            !ok && out.contains("--storage.token_ttl is \"abc\""),
            "{out}"
        );
        let (ok, out) = run(&bare, "flags-bare", &["--storage.secret=x"], &[]);
        assert!(
            !ok && out.contains("unknown argument"),
            "a secret has no flag: {out}"
        );
        let (ok, out) = run(&bare, "flags-bare", &["--help"], &[]);
        assert!(ok && out.contains("--sessions.cookie <VALUE>"), "{out}");
        // The framework's own setting has a flag too, typed as its field.
        assert!(
            out.contains("--server.shutdown_grace_secs <VALUE>"),
            "{out}"
        );
        let line = ["--server.shutdown_grace_secs=soon"];
        let (ok, out) = run(&bare, "flags-bare", &line, &[]);
        assert!(
            !ok && out.contains("--server.shutdown_grace_secs is \"soon\""),
            "{out}"
        );
        // A variable reaches the plugin; a secret has one even with no flag.
        let env = [("FLAGS_BARE_SESSIONS__COOKIE", "")];
        let (ok, out) = run(&bare, "flags-bare", &[], &env);
        assert!(!ok && out.contains("cookie must not be empty"), "{out}");
        // Each layer on its own, then the merge. The flag wins over the
        // variable, and the secret is masked everywhere.
        let env = [
            ("FLAGS_BARE_SESSIONS__COOKIE", "from-env"),
            ("FLAGS_BARE_STORAGE__SECRET", "hush"),
        ];
        let line = ["--sessions.cookie=from-flag", "--settings"];
        let (ok, out) = run(&bare, "flags-bare", &line, &env);
        assert!(ok, "{out}");
        assert!(out.contains("   2  environment\n"), "{out}");
        assert!(
            out.contains("\"from-env\"  $FLAGS_BARE_SESSIONS__COOKIE"),
            "{out}"
        );
        assert!(out.contains("\"from-flag\"  3 flags"), "{out}");
        assert!(
            out.contains("storage.secret") && !out.contains("hush"),
            "{out}"
        );
        let _ = std::fs::remove_dir_all(&bare);

        // A config file and the flags left off: a flag is refused by name,
        // with the two ways to do what was meant.
        let off = build("flags-off", vec!["sessions", "config"], None);
        let (ok, out) = run(&off, "flags-off", &["--sessions.cookie=x"], &[]);
        assert!(
            !ok && out.contains("`--sessions.cookie` is off")
                && out.contains("help: set WITH_CONFIG = true in src/flags.rs"),
            "{out}"
        );
        let _ = std::fs::remove_dir_all(&off);

        // A config file, the flags turned on, and one renamed.
        let configured = build(
            "flags-configured",
            vec!["sessions", "config", "commands"],
            Some(
                "pub const WITH_CONFIG: bool = true;\n\
                 pub const RENAMES: &[(&str, &str)] = &[(\"sessions.cookie\", \"cookie\")];\n",
            ),
        );
        let name = "flags-configured";
        let (ok, out) = run(&configured, name, &["serve", "--cookie="], &[]);
        assert!(!ok && out.contains("cookie must not be empty"), "{out}");
        let (ok, out) = run(&configured, name, &["serve", "--sessions.cookie=x"], &[]);
        assert!(
            !ok && out.contains("--sessions.cookie"),
            "renamed away: {out}"
        );
        let (ok, out) = run(&configured, name, &["serve", "--help"], &[]);
        assert!(ok && out.contains("--cookie <VALUE>"), "{out}");
        let (ok, out) = run(&configured, name, &["settings", "--cookie=x"], &[]);
        assert!(
            ok && out.contains("--cookie") && out.contains("4 flags"),
            "{out}"
        );
        // `[server]` takes a flag on top of the file, like a module's section.
        let line = ["serve", "--server.shutdown_grace_secs=soon"];
        let (ok, out) = run(&configured, name, &line, &[]);
        assert!(
            !ok && out.contains("`[server] shutdown_grace_secs` is") && out.contains("help: "),
            "{out}"
        );
        let _ = std::fs::remove_dir_all(&configured);
    }

    /// The hooks from `brocanth-middleware` take their settings from the
    /// file, the flags and the environment, and a request shows it.
    ///
    /// Two generated servers. One reads `config/development.toml`, edited
    /// here the way a developer would: a renamed CSRF cookie, a report-only
    /// policy, a referrer policy, and a rate-limit rule of one GET. The
    /// other has no file, and gets the same values from flags and a
    /// variable. Each answers a real request over a socket.
    ///
    /// `cargo test -p brocanth-cli -- --ignored`.
    #[test]
    #[ignore = "runs cargo build over a generated application"]
    #[cfg(unix)]
    fn hook_settings_reach_a_request_from_the_file_and_from_the_flags() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf();
        let brocanth = crates
            .join("brocanth")
            .canonicalize()
            .expect("the framework is beside the cli");
        let build = |name: &str, picks: Vec<&'static str>| {
            let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let a = Answers {
                name: name.into(),
                title: "Hooks Test".into(),
                picks,
                local: Some(brocanth.display().to_string()),
                git: false,
            };
            for (path, contents) in scaffold(&a).expect("the modules agree") {
                let full = root.join(&path);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(&full, contents).unwrap();
            }
            root
        };
        let compile = |root: &Path| {
            let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .args(["build", "--quiet"])
                .current_dir(root)
                .output()
                .expect("run cargo");
            assert!(
                output.status.success(),
                "{} did not compile:\n{}",
                root.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        let hooks = vec![
            "middleware",
            "middleware-csrf",
            "middleware-security-headers",
            "middleware-rate-limit",
            "commands",
        ];

        // The file, edited.
        let mut picks = hooks.clone();
        picks.push("config");
        let filed = build("hooks-file", picks);
        let path = filed.join("config/development.toml");
        let toml = std::fs::read_to_string(&path).unwrap();
        let edited = toml
            .replace("cookie = \"csrf_token\"", "cookie = \"xsrf_token\"")
            .replace(
                "# referrer_policy = \"strict-origin-when-cross-origin\"",
                "referrer_policy = \"no-referrer\"",
            )
            .replace("report_only = false", "report_only = true")
            .replace("paths = [\"/login\"]", "paths = [\"/\"]")
            .replace("methods = [\"POST\"]", "methods = [\"GET\"]")
            .replace("limit = 5", "limit = 1");
        for changed in [
            "xsrf_token",
            "no-referrer",
            "report_only = true",
            "limit = 1",
        ] {
            assert!(
                edited.contains(changed),
                "the file has no {changed}:\n{toml}"
            );
        }
        std::fs::write(&path, edited).unwrap();
        compile(&filed);
        let answers = serve_and_get(&filed.join("target/debug/hooks-file"), &[], &[], 2);
        let first = answers[0].to_ascii_lowercase();
        assert!(first.starts_with("http/1.1 200"), "{first}");
        assert!(first.contains("set-cookie: xsrf_token="), "{first}");
        assert!(first.contains("referrer-policy: no-referrer"), "{first}");
        assert!(
            first.contains("content-security-policy-report-only:"),
            "{first}"
        );
        assert!(
            answers[1].starts_with("HTTP/1.1 429"),
            "the second GET is over the file's limit of one:\n{}",
            answers[1]
        );
        let _ = std::fs::remove_dir_all(&filed);

        // No file: the same values from the flags, and one variable.
        let flagged = build("hooks-flags", hooks);
        compile(&flagged);
        let answers = serve_and_get(
            &flagged.join("target/debug/hooks-flags"),
            &[
                "--csrf.cookie=xsrf_token",
                "--security_headers.csp.report_only=true",
                "--security_headers.referrer_policy=",
            ],
            &[("HOOKS_FLAGS_SECURITY_HEADERS__NOSNIFF", "false")],
            1,
        );
        let first = answers[0].to_ascii_lowercase();
        assert!(first.starts_with("http/1.1 200"), "{first}");
        assert!(first.contains("set-cookie: xsrf_token="), "{first}");
        assert!(
            first.contains("content-security-policy-report-only:"),
            "{first}"
        );
        assert!(
            !first.contains("referrer-policy:"),
            "an empty flag is none: {first}"
        );
        assert!(
            !first.contains("x-content-type-options:"),
            "the variable turned nosniff off: {first}"
        );
        let _ = std::fs::remove_dir_all(&flagged);
    }

    /// Starts `binary serve` on a free port with `args` and `env`, sends
    /// `count` GETs to `/`, stops the server and returns each response.
    #[cfg(unix)]
    fn serve_and_get(
        binary: &Path,
        args: &[&str],
        env: &[(&str, &str)],
        count: usize,
    ) -> Vec<String> {
        use std::io::{BufRead, BufReader, Read, Write};

        // The project root, where `config/` is: `<root>/target/debug/<name>`.
        let root = binary
            .ancestors()
            .nth(3)
            .expect("a binary under target/debug");
        let mut child = Command::new(binary)
            .current_dir(root)
            .args(["serve", "--addr", "127.0.0.1:0"])
            .args(args)
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the server");
        let mut stderr = BufReader::new(child.stderr.take().unwrap());
        let mut log = String::new();
        let addr = loop {
            let mut line = String::new();
            if stderr.read_line(&mut line).unwrap() == 0 {
                let _ = child.wait();
                panic!("the server stopped before it listened:\n{log}");
            }
            log.push_str(&line);
            if let Some(at) = line.trim().split("listening on http://").nth(1) {
                break at.to_string();
            }
        };
        let answers = (0..count)
            .map(|_| {
                let mut stream = std::net::TcpStream::connect(&addr).expect("connect");
                write!(
                    stream,
                    "GET / HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
                let mut response = Vec::new();
                let _ = stream.read_to_end(&mut response);
                String::from_utf8_lossy(&response).into_owned()
            })
            .collect();
        let _ = child.kill();
        let _ = child.wait();
        answers
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
            picks: vec![
                "observability",
                "observability-health",
                "config",
                "commands",
            ],
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

        let get = |addr: &str, path: &str| {
            let mut stream = std::net::TcpStream::connect(addr).expect("connect");
            write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut response = Vec::new();
            let _ = stream.read_to_end(&mut response);
            String::from_utf8_lossy(&response).into_owned()
        };
        // The `observe` probes, before anything stops.
        for probe in ["/up", "/readyz"] {
            let answer = get(&addr, probe);
            assert!(answer.starts_with("HTTP/1.1 200"), "{probe}: {answer}");
        }
        let client = std::thread::spawn(move || get(&addr, "/slow"));
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
/// publishes as the answer. `async`, so the table awaits it; `handle_put`
/// below is a plain `fn` in the same file.
pub async fn handle_post(_ctx: &mut Ctx, body: Signup) -> Result<Json<Created>, Error> {
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

    #[tokio::test]
    async fn a_body_that_breaks_a_rule_is_refused_before_the_handler_runs() {
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
        let Err(refused) = handler(&mut ctx).await else {
            panic!("the body broke two rules and was not refused");
        };
        assert_eq!(refused.status, 422);
        assert!(refused.message.contains("handle"), "{}", refused.message);
        assert!(refused.message.contains("age"), "both faults: {}", refused.message);
    }

    #[tokio::test]
    async fn a_body_that_passes_reaches_the_handler_and_comes_back_as_json() {
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

        let answered = handler(&mut ctx).await.expect("the body passed");
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
