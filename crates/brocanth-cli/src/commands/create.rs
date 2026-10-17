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
fn offered() -> clap::builder::PossibleValuesParser {
    clap::builder::PossibleValuesParser::new(
        crate::modules::ALL
            .iter()
            .map(|module| {
                let m = module.manifest();
                clap::builder::PossibleValue::new(m.key).help(m.gist)
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
fn all() -> Vec<&'static str> {
    crate::modules::keys()
}

/// The manifest of a module this CLI has. Panics for a key it does not,
/// which is a programming error here rather than anything a user can type:
/// every key reaching this came from [`all`] or from a validated flag.
fn about(key: &str) -> brocanth_scaffold::Manifest {
    crate::modules::find(key)
        .unwrap_or_else(|| unreachable!("`{key}` is not a module this CLI has"))
        .manifest()
}

/// The answers, however they arrived.
struct Answers {
    /// The cargo package name, and the directory.
    name: String,
    /// The human name.
    title: String,
    picks: Vec<&'static str>,
    /// A local checkout to depend on, instead of the repository.
    local: Option<String>,
    git: bool,
}

impl Answers {
    fn has(&self, key: &str) -> bool {
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
    for pick in all().iter().filter(|p| !answers.has(p)) {
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
    orphans(&picked(args))?;

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

/// A choice that is part of another choice, named without it.
///
/// The chooser cannot produce this -- it hides a child whose parent is off --
/// so it is only reachable from the flags, where it is nearly always a
/// forgotten word rather than an intention.
fn orphans(picks: &[&str]) -> Result<(), Vec<Diagnostic>> {
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

    let mut wrong: Vec<Diagnostic> = built
        .problems()
        .iter()
        .map(|problem| Diagnostic::error("module", problem.clone()))
        .collect();
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

fn scaffold(a: &Answers) -> Result<Vec<(String, String)>, Vec<Diagnostic>> {
    let built = contributions(a)?;
    let mut files = vec![
        ("Cargo.toml".to_string(), cargo_toml(a, &built)),
        (
            ".gitignore".to_string(),
            if a.has("rsx-tailwind") {
                // `assets/css/site.css` is **not** ignored. It is embedded
                // at compile time, so a checkout without it does not build,
                // and "run npm first" is a worse answer than 3 kB in git.
                "/target\n/node_modules\n".to_string()
            } else {
                "/target\n".to_string()
            },
        ),
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
            mise_toml(a),
        ),
        (
            "src/main.rs".to_string(),
            if a.has("commands") {
                main_with_commands_rs(a, &built)
            } else {
                main_serve_only_rs(a, &built)
            },
        ),
        // Always: it is the one answer to what this application is, and both
        // shapes of `main` read it.
        ("src/app.rs".to_string(), app_rs(a, &built)),
        ("src/pages/mod.rs".to_string(), pages_mod_rs(a)),
        ("src/pages/index.rs".to_string(), index_rs(a)),
        ("src/pages/about.rs".to_string(), about_rs(a)),
        ("src/pages/users/[id].rs".to_string(), user_rs(a)),
    ];
    if a.has("rsx") {
        // Beside the page each one renders, and named after it. The Crystal
        // page_router does the same with `layout.cr`, and the reason carries
        // over: a route is a file, so the markup for a route is the file
        // next to it rather than a mirror tree to keep in step.
        files.push(("src/pages/layout.rsx".to_string(), layout_rsx(a)));
        files.push(("src/pages/index.rsx".to_string(), index_rsx(a)));
        files.push(("src/pages/about.rsx".to_string(), ABOUT_RSX.to_string()));
        files.push((
            "src/pages/users/layout.rsx".to_string(),
            users_layout_rsx(a),
        ));
        files.push(("src/pages/users/[id].rsx".to_string(), USER_RSX.to_string()));
    }
    if a.has("rsx-tailwind") || a.has("rsx-htmx") {
        files.push(("src/assets.rs".to_string(), ASSETS_RS.to_string()));
    }
    if a.has("rsx-tailwind") {
        files.push((
            "assets/css/_tailwind.css".to_string(),
            TAILWIND_CSS.to_string(),
        ));
        // So the first `cargo build` works before anyone has run node.
        files.push((
            "assets/css/site.css".to_string(),
            SITE_CSS_PLACEHOLDER.to_string(),
        ));
        files.push(("scripts/build-css.sh".to_string(), BUILD_CSS_SH.to_string()));
        files.push(("package.json".to_string(), package_json(a)));
    }
    if a.has("rsx-htmx") {
        // Vendored into the brocanth binary, written out here. A project
        // that has to fetch a script to finish is a project that fails on a
        // train.
        files.push(("public/dpswap.min.js".to_string(), DPSWAP_JS.to_string()));
        files.push(("src/pages/link.rsx".to_string(), LINK_RSX.to_string()));
    }
    if !built.slot_is_empty(Slot::PluginsMod) {
        files.push(("src/plugins/mod.rs".to_string(), plugins_mod_rs(&built)));
    }
    if a.has("config") {
        files.push((
            format!("config/{}.toml", brocanth_config::DEFAULT_ENV),
            config_toml(a, &built),
        ));
    }
    if a.has("commands") {
        // The marker the global `brocanth` binary walks up to find. Its
        // presence is what makes a directory a project.
        files.push((
            "brocanth.toml".to_string(),
            format!("[project]\nname = {:?}\n", a.name),
        ));
        // No cargo alias. A project's command line is a framework concern and
        // cargo is the build tool; hooking one into the other conflates them.
        // The ways in are `brocanth <command>` from anywhere inside the
        // project, and the built binary itself.
        files.push((
            "src/commands/mod.rs".to_string(),
            COMMANDS_MOD_RS.to_string(),
        ));
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
    files.push(("AGENTS.md".to_string(), agents_md(a)));
    Ok(files)
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
    let absent: Vec<String> = all()
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
        "\n## Tasks\n\nA command is a `clap::Args` struct with `#[cmd]` on it, in its own file under\n\
         `src/commands/`, plus a `mod` line. It becomes a subcommand automatically —\n\
         registration happens at link time and there is no list to add it to.\n\n\
         ```\nbrocanth --help      # lists this project's commands too\nbrocanth serve\n```\n"
    } else {
        ""
    };

    format!(
        "# Working in {name}\n\
         \n\
         A brief for a coding agent. Written by `brocanth create`, so the lists\n\
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
const COMMANDS_MOD_RS: &str = r#"//! Every command in this project.
//!
//! The `mod` lines are the whole of it. A command registers itself through a
//! link-time slice, so adding one is a file and a `mod`, and there is no list
//! to keep in step.
//!
//! `serve` arrived with the scaffold and is ordinary code here: rename it,
//! give it flags, or delete it. The framework does not own this command line.

pub mod serve;
"#;

/// Where a brocanth crate comes from: a local checkout when one was named, the
/// repository otherwise.
fn source_of(a: &Answers, dep: &Dep) -> String {
    let crate_name = dep.name;
    let note = dep.note.map_or_else(String::new, |n| format!("  # {n}"));
    match &dep.source {
        // Not a brocanth crate, so `--brocanth-path` has nothing to say
        // about where it comes from.
        Source::Git { url, branch } => {
            return format!("{crate_name} = {{ git = {url:?}, branch = {branch:?} }}{note}")
        }
        Source::Version(req) => return format!("{crate_name} = {{ version = {req:?} }}{note}"),
        Source::Brocanth => {}
    }
    match &a.local {
        // `--brocanth-path` points at `crates/brocanth`; its siblings are the
        // other crates of the same checkout.
        Some(path) => {
            let dir = if crate_name == "brocanth" {
                Path::new(path).to_path_buf()
            } else {
                Path::new(path)
                    .parent()
                    .map(|p| p.join(crate_name))
                    .unwrap_or_else(|| Path::new(path).to_path_buf())
            };
            format!(
                "{crate_name} = {{ path = {:?} }}{note}",
                dir.display().to_string()
            )
        }
        None => format!("{crate_name} = {{ git = {REPOSITORY:?} }}{note}"),
    }
}

/// One of brocanth's own crates, which every project gets whatever it chose.
fn core(name: &'static str) -> Dep {
    Dep::brocanth(name)
}

/// The manifest, and the comment that makes it a statement rather than a list.
fn cargo_toml(a: &Answers, built: &Built) -> String {
    let mut deps = vec![source_of(a, &core("brocanth"))];
    for pick in &a.picks {
        for dep in about(pick).crates {
            deps.push(source_of(a, &dep));
        }
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
            "tokio = { version = \"1\", features = [\"rt-multi-thread\"] }  # the reactor `serve` runs on"
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
    let mut absent: Vec<String> = Vec::new();
    for key in all().into_iter().filter(|key| !a.has(key)) {
        if let Some((what, why)) = about(key).absent {
            absent.push(format!("#   {what:<17}{why}"));
        }
    }
    if !needs_serde(a, built) {
        absent.push("#   serde            nothing here is deserialised".to_string());
    }
    let absent = format!(
        "#\n# Not here, because nothing asked for it:\n{}\n",
        absent.join("\n")
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

/// `main`, which is the modularity claim written out as a numbered list.
fn app_rs(a: &Answers, built: &Built) -> String {
    let upper = a.ident().to_uppercase();
    let any_plugin = !built.slot_is_empty(Slot::PluginsMod);
    // A layer is a thing some module offered, so the import that names
    // `Next` follows from the slot rather than from a list of module names
    // this file would have to keep in step with the registry.
    let layered = !built.slot_is_empty(Slot::AppSetup);
    let mut out = String::new();

    out.push_str(&format!(
        "//! {title} -- everything this binary contains, in the order it happens.\n\
         //!\n\
         //! A plugin that is not named in [`build`] is not registered, and one that is\n\
         //! not in `Cargo.toml` is not compiled at all. \"The framework decided for you\n\
         //! and you cannot see where\" is the thing this layout exists to make\n\
         //! impossible.\n\
         //!\n\
         //! `serve` and every other command read this, so there is one answer to what\n\
         //! this application is.\n\
         \n\
         use std::sync::Arc;\n\
         \n",
        title = a.title
    ));
    // Whatever the modules need in scope. Sorted, because rustfmt sorts
    // `use` lines and a project must survive its own first `cargo fmt`.
    let mut uses: Vec<&str> = built.slot(Slot::AppUses).collect();
    uses.sort_unstable();
    for line in uses {
        out.push_str(line);
    }
    out.push_str("use brocanth::plugin_app::AppBuilder;\n");
    out.push_str(if layered {
        "use brocanth::routing::{Dispatch, MiddlewareStack, Next, Router};\n"
    } else {
        "use brocanth::routing::{Dispatch, MiddlewareStack, Router};\n"
    });
    if a.has("config") {
        out.push_str("use brocanth_config::{AppBuilderConfigExt, Loader};\n");
    }
    out.push_str("\nuse crate::pages::{self, Ctx, Handler, IntoResponse, Response};\n");
    if any_plugin {
        out.push_str("use crate::plugins;\n");
    }
    out.push('\n');

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
         }}\n\
         \n\
         /// Builds it. The only place that decides what this binary is.\n\
         pub fn build() -> Result<App, String> {{\n",
        name = a.name
    ));

    // 1 and 2: the application and every plugin in it.
    if a.has("config") {
        out.push_str(&format!(
            "    // 1. Configuration. The core takes already-built values; finding the\n\
            \x20   //    files, layering them and typing each section is `brocanth-config`'s\n\
            \x20   //    job, and it is the only reason a TOML parser is in the tree.\n\
            \x20   //    `${upper}_ENV` picks the environment, `${upper}_CONFIG` overrides\n\
            \x20   //    the file outright.\n\
            \x20   let config = Loader::new(APP).load().map_err(|e| e.to_string())?;\n\
             \n\
            \x20   // 2. The application, and every plugin it contains. Each line is a line\n\
            \x20   //    a reader can delete; nothing registers itself. A section no\n\
            \x20   //    registered instance claims stops the build rather than being\n\
            \x20   //    silently ignored.\n\
            \x20   let app = AppBuilder::new()\n\
            \x20       .with_config(config)\n",
        ));
        for line in built.slot(Slot::AppPlugins) {
            out.push_str(line);
        }
    } else {
        out.push_str(
            "    // 1. Configuration. No file lookup, no merge order, no TOML parser: the\n\
            \x20   //    core takes values that are already built, so each plugin below is\n\
            \x20   //    handed a literal the compiler checks. Add the config plugin when a\n\
            \x20   //    file is wanted; nothing else changes.\n\
             \n\
            \x20   // 2. The application, and every plugin it contains. Each line is a line\n\
            \x20   //    a reader can delete; nothing registers itself.\n\
            \x20   let app = AppBuilder::new()\n",
        );
        for line in built.slot(Slot::AppPlugins) {
            out.push_str(line);
        }
    }

    out.push_str(
        "        .setup(|setup| {\n\
        \x20           // A plugin *offers* middleware and cannot mount it: a\n\
        \x20           // `MiddlewareCatalog` has no `mount`. Mounting takes a pattern, the\n\
        \x20           // pattern is a decision, and the decision is the application's.\n",
    );
    for line in built.slot(Slot::AppSetup) {
        out.push_str(line);
    }
    if !layered {
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
        \x20   // 3. The route table: the shape of src/pages and nothing else. A file is\n\
        \x20   //    a route, and `pages::router` is the only place that says so.\n\
        \x20   let router = pages::router().map_err(|e| e.to_string())?;\n\
         \n",
    );

    // 4: the chain.
    if layered {
        out.push_str(
            "    // 4. The chain, from exactly what step 2 mounted. The match is\n\
            \x20   //    exhaustive on purpose: a name this binary has no layer for stops\n\
            \x20   //    the application rather than silently not running.\n\
            \x20   let mut stack: MiddlewareStack<Ctx, Response> = MiddlewareStack::new();\n\
            \x20   for mount in app.router_plan().mounts() {\n\
            \x20       let layer: fn(&mut Ctx, Next<'_, Ctx, Response>) -> Response =\n\
            \x20           match mount.middleware.as_str() {\n",
        );
        if a.has("logging") {
            out.push_str("                \"request_log\" => plugins::logging::layer,\n");
        }
        if a.has("sessions") {
            out.push_str("                \"session\" => plugins::sessions::layer,\n");
        }
        // Wrapped the way rustfmt wants it. A scaffolded project should be
        // fmt-clean on its first `cargo fmt`, or the generator is handing the
        // developer a diff they did not write.
        out.push_str(
            "                other => {\n\
            \x20                   return Err(format!(\n\
            \x20                       \"`{other}` is mounted, but this binary has no layer for it\"\n\
            \x20                   ))\n\
            \x20               }\n\
            \x20           };\n\
            \x20       stack\n\
            \x20           .use_at(&mount.pattern, layer)\n\
            \x20           .map_err(|e| e.to_string())?;\n\
            \x20   }\n\
             \n",
        );
    } else {
        out.push_str(
            "    // 4. The chain. Nothing offered middleware, so the stack is empty --\n\
            \x20   //    and an empty stack costs nothing: `run` allocates only when a\n\
            \x20   //    layer matches the path.\n\
            \x20   let stack: MiddlewareStack<Ctx, Response> = MiddlewareStack::new();\n\
             \n",
        );
    }

    // 5: the context, where a plugin provided one.
    if a.has("sessions") || a.has("model") {
        out.push_str(
            "    // The per-request context. These lines compile only because the\n\
            \x20   // plugins that provide the values were registered in step 2: the\n\
            \x20   // installed provider set lives in the application's *type*. Delete a\n\
            \x20   // registration above and this stops compiling, which is the whole\n\
            \x20   // mechanism -- modularity enforced rather than documented.\n\
            \x20   let cx = app.context();\n",
        );
        if a.has("sessions") {
            out.push_str(
                "    let session: &plugins::sessions::Session = cx.extract();\n\
                \x20   eprintln!(\"session cookie: {}\", session.cookie);\n",
            );
        }
        if a.has("model") {
            out.push_str(
                "    let db: &plugins::model::Db = cx.extract();\n\
                \x20   eprintln!(\"database: {}\", db.url);\n",
            );
        }
    }

    out.push_str(
        "    Ok(App {\n\
        \x20       router,\n\
        \x20       stack,\n\
        \x20       plugins: app.instance_names().map(str::to_string).collect(),\n\
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

    out.push_str(
        "/// Serves until the process is stopped.\n\
         ///\n\
         /// Synchronous on the outside because the command that calls it is, and\n\
         /// the runtime is built here rather than wrapped around `main`: a command\n\
         /// that never serves should not start a reactor to run.\n\
         pub fn serve(addr: &str) -> Result<(), String> {\n\
        \x20   let app = Arc::new(build()?);\n\
        \x20   eprintln!(\"{APP}: plugins: [{}]\", app.plugins.join(\", \"));\n\
         \n\
        \x20   let runtime = tokio::runtime::Builder::new_multi_thread()\n\
        \x20       .enable_all()\n\
        \x20       .build()\n\
        \x20       .map_err(|e| e.to_string())?;\n\
         \n\
        \x20   runtime.block_on(async move {\n\
        \x20       let server = brocanth_server::Server::bind(addr)\n\
        \x20           .await\n\
        \x20           .map_err(|e| e.to_string())?;\n\
        \x20       let bound = server.local_addr().map_err(|e| e.to_string())?;\n\
        \x20       eprintln!(\"{APP}: listening on http://{bound}\");\n\
         \n\
        \x20       server\n\
        \x20           .run(move |request| {\n\
        \x20               // Dispatch is synchronous and runs here, outside the\n\
        \x20               // future, so what crosses the await is a finished\n\
        \x20               // response and nothing borrowed.\n\
        \x20               let out = ",
    );
    out.push_str(if a.has("rsx-tailwind") || a.has("rsx-htmx") {
        // Static files never reach a page. They are bytes with a content
        // type, and the page contract carries a `String` body on purpose.
        "serve_asset(&request).unwrap_or_else(|| sent(dispatch(&app, &request)));\n"
    } else {
        "sent(dispatch(&app, &request));\n"
    });
    out.push_str(
        "\x20               async move { out }\n\
        \x20           })\n\
        \x20           .await\n\
        \x20           .map_err(|e| e.to_string())\n\
        \x20   })\n\
         }\n\
         \n\
         /// One request: the table, then the chain, then the handler.\n\
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
    out.replace(
        "@CTX@",
        if a.has("rsx-htmx") {
            "Ctx::new(verb, path, matched.params()).holding(holding(request))"
        } else {
            "Ctx::new(verb, path, matched.params())"
        },
    )
}

/// `mise.toml`. The sections are `[tasks.*]`, which is mise's spelling and not
/// brocanth's -- a rename of brocanth's own vocabulary once turned these into
/// `[commands.*]`, and mise answered `unknown field`.
/// `src/main.rs` when the project has no command line: it serves, and that is
/// all it does.
fn main_serve_only_rs(a: &Answers, built: &Built) -> String {
    format!(
        r##"//! {title} -- a brocanth application.
//!
//! No command line was asked for, so this binary does one thing. Everything it
//! contains is in `src/app.rs`; `brocanth create --with commands` turns this
//! into a command line with `serve` as one of its commands.

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
        plugins = if built.slot_is_empty(Slot::PluginsMod) {
            ""
        } else {
            "mod plugins;\n"
        },
    )
}

/// `src/main.rs` when the project has a command line: it is the command line.
fn main_with_commands_rs(a: &Answers, built: &Built) -> String {
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
        plugins = if built.slot_is_empty(Slot::PluginsMod) {
            ""
        } else {
            "mod plugins;\n"
        },
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

fn mise_toml(a: &Answers) -> String {
    let mut out = String::from(MISE_TOML);
    if a.has("rsx-tailwind") {
        out.push_str(
            "\n[tasks.css]\n\
             description = \"build assets/css/site.css from _tailwind.css\"\n\
             run = \"bash scripts/build-css.sh\"\n\
             \n\
             [tasks.\"css:watch\"]\n\
             description = \"the same, again on each change\"\n\
             run = \"bash scripts/build-css.sh --watch\"\n",
        );
    }
    out
}

const MISE_TOML: &str = r#"[tools]
# Must match the toolchain brocanth's proc-macro artifacts were built with:
# they are not compatible across patch releases.
rust = "1.98"

[tasks.run]
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

brocanth_pages::pages!("src/pages");

use brocanth::routing::{BuildError, Params, Router, Verb};

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
}

impl Error {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// The default for a failure nobody classified: it is a bug here, not a
    /// mistake by the caller.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(500, message)
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
        let body = if self.status < 500 {
            self.message
        } else {
            String::from("Internal Server Error")
        };
        Response::new(self.status, body)
    }
}

/// What a handler is given. Middleware sees it too, and may change it before
/// the handler runs or after it returns.
pub struct Ctx {
    pub verb: Verb,
    pub path: String,
    params: Vec<(String, String)>,@DEPTH_FIELD@
}

impl Ctx {
    pub fn new(verb: Verb, path: &str, params: Params<'_>) -> Self {
        Self {
            verb,
            path: path.to_string(),
            params: params
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),@DEPTH_INIT@
        }
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

/// dp-swap, the client half of partial navigation.
///
/// Embedded rather than fetched: `brocanth create` has to finish on a
/// machine with no network, and a scaffold that sometimes leaves a file out
/// is worse than a binary a few kilobytes larger. See `vendor/README.md`.
const DPSWAP_JS: &str = include_str!("../../vendor/dpswap.min.js");

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
/// `html` rather than a guess: every page returns markup, and a handler that
/// wants another type says so with a `content-type` header, which is applied
/// after and therefore wins.
fn sent(answered: Response) -> brocanth_server::Response {
    let mut out = brocanth_server::Response::html(answered.status, answered.body);
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

/// `src/assets.rs`: two directories, embedded.
const ASSETS_RS: &str = r#"//! Every static file this binary serves.
//!
//! Two directories, two rules. `assets/` is **fingerprinted** -- `site.css`
//! is published at `/css/site.<hash>.css`, so it can be cached forever and a
//! new build invalidates it by having a different name. `public/` is served
//! **verbatim**, because `robots.txt`, `favicon.ico` and anything linked to
//! from outside must keep the name they were given.
//!
//! Write a reference with `assets::url`, never by hand:
//!
//! ```ignore
//! <link rel="stylesheet" href={assets::url("css/site.css")}>
//! ```
//!
//! A name nothing publishes panics there rather than 404ing in a browser.
//!
//! Embedded, like the templates and the route table, so `serve` works from
//! any directory and nothing can go missing after the build passed. A file
//! added where there was none needs `touch src/assets.rs`, because nothing
//! referenced it yet.

brocanth_assets::assets!("assets", "public");
"#;

/// The templates and the bridge to them, written only when the project asked
/// for them.
const PAGES_RSX_RS: &str = r##"/// Every template in this application.
///
/// The `.rsx` files sit next to the pages they render -- `index.rsx` beside
/// `index.rs` -- and `load_dir!` compiles the tree while this crate is being
/// compiled. Nothing is read from disk when a request arrives, and a template
/// that does not parse is a compile error on its own line.
///
/// A module of their own, because `load_dir!` emits a `pub mod index` for
/// `index.rsx` and `pages!` emits one for `index.rs`: unnested they collide.
/// The path is relative to the package root and not to this file, because a
/// proc macro cannot learn where it was written on stable Rust.
///
/// A file whose name is not an identifier gets the nearest one: `[id].rsx` is
/// `views::users::_id_`.
pub mod views {
    rsx::load_dir!("src/pages");
}

/// Renders a page inside the layouts that cover it.
///
/// `layout.rsx` wraps every page in its directory and in every directory
/// below it, and they stack from the root down. A page never names one,
/// because which layout covers a page is a property of where the page is --
/// the rule Crystal's page_router uses for `layout.cr`.
///
/// The macro reads that off the directory tree at compile time: it knows
/// which file it was written in, walks up to `src/`, and wraps the page in
/// every `layout.rsx` it passes. Add one to a directory and every page under
/// it is wrapped, with nothing to register and no chain to keep in step.
///
/// Re-exported here so a page imports it from the same place as everything
/// else it uses, and so the layout rule lives beside the route table rather
/// than in a crate name.
pub use brocanth_pages::html;

/// The URL prefix a layout covers, for a layout under a parameter.
///
/// `html!` passes a layout the prefix it wraps, which is the marker dp-swap
/// reads to work out how much of a page two URLs share. A directory named
/// after a parameter has no prefix of its own until a request arrives, so
/// the macro hands this function the template and the request fills it:
/// `/{section}` under `/alpha/card-2` is `/alpha`.
///
/// Called only from the expansion, and only for a layout that sits under a
/// parameter. A marker reading `/[section]` would be the same string on
/// every section, and the client would conclude that two pages share a
/// layer they do not.
pub fn prefix(ctx: &Ctx, template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        // A parameter the router did not match cannot happen: this template
        // is built from the directory the route was read out of, so the
        // name is one the pattern declares.
        out.push_str(ctx.param(&rest[open + 1..open + close]).unwrap_or_default());
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// A partial, and the replacements for the chrome it left behind.
///
/// A layout the client keeps is a layout this answer does not contain, which
/// is the point of asking for a partial and the reason its chrome goes
/// stale: the sidebar still marks the section you came from. A layout that
/// has something to correct defines `Oob` beside its `Layout`, and `html!`
/// renders one per skipped layer and passes them here.
///
/// They lead, because the static version of this writes them that way. The
/// client reads the whole body before it swaps anything, so the order is a
/// convention rather than a requirement, and one convention is better than
/// two.
pub fn render_with(
    out_of_band: Vec<String>,
    content: impl rsx::Template,
) -> Result<Response, Error> {
    let mut body = out_of_band.concat();
    body.push_str(&content.render_to_string()?);
    Ok(Response::ok(body))
}

/// A page with no layout at all: a fragment, a partial, an htmx swap.
///
/// What `html!` expands into, once it has wrapped the page, so this is also
/// the escape hatch -- name a layout by hand and pass it here.
///
/// It returns the handler's own `Result` rather than a value, so a template
/// that bails propagates with `?` instead of turning into a blank 200. For
/// another status or a header, compose with the forms above:
/// `Ok((html!(ctx, tpl)?, 404))`, or `Ok(html!(ctx, tpl)?.with_header("x-kind", "page"))`.
///
/// Deliberately a function and not `impl<T: rsx::Template> IntoResponse for T`.
/// `()`, `Option<T>` and `Vec<T>` are templates, so a blanket impl overlaps
/// the ones above and does not compile -- and an `into_response` that cannot
/// fail would have to swallow a render error. A blanket impl could not apply
/// a layout either, since it would not know which page it was rendering.
pub fn render(content: impl rsx::Template) -> Result<Response, Error> {
    // `render_to_string` builds into its own String, so a page that fails
    // half way through its layout leaves nothing behind. There is no partial
    // body to take back, which is the property the rest of this file is
    // built on.
    Ok(Response::ok(content.render_to_string()?))
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
        assert_eq!(patterns, ["/", "/about", "/users/{id}"]);
    }

    #[test]
    fn a_path_that_exists_with_a_verb_that_does_not_is_a_405() {
        let router = router().expect("the pages produce a table");
        let allow = router.allow_for("/about").expect("/about is a route");
        assert_eq!(allow.to_string(), "GET, HEAD");
    }
}
"##;

/// `src/pages/mod.rs`, in three parts so the rsx bridge can land before the
/// test module rather than after it, which clippy refuses.
fn pages_mod_rs(a: &Answers) -> String {
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
    if a.has("rsx") {
        // The root layout writes the name, so there is one place it is
        // spelled rather than one per page.
        out.push_str(&format!(
            "/// The application's name, as the root layout writes it.\n\
             pub const TITLE: &str = {:?};\n\n",
            a.title
        ));
        out.push_str(PAGES_RSX_RS);
        out.push('\n');
    }
    out.push_str(PAGES_MOD_RS_TESTS);
    out.replace("@DEPTH_FIELD@", field)
        .replace("@DEPTH_INIT@", init)
        .replace("@DEPTH_IMPL@", &format!("{setter}{methods}"))
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
fn layout_rsx(a: &Answers) -> String {
    let mut head = String::new();
    if a.has("rsx-tailwind") {
        // `assets::url`, never the path by hand: the published name carries
        // a content hash, so writing it would be writing a name that changes
        // every time the stylesheet does.
        head.push_str(
            "            <link rel=\"stylesheet\" href={crate::assets::url(\"css/site.css\")}>\n",
        );
    }
    if a.has("rsx-htmx") {
        head.push_str(
            "            <script src={crate::assets::url(\"dpswap.min.js\")} defer></script>\n",
        );
    }
    let body_class = if a.has("rsx-tailwind") {
        " class=\"min-h-screen bg-white text-slate-900\""
    } else {
        ""
    };
    let main_open = if a.has("rsx-htmx") {
        // The marker dp-swap walks. It wraps exactly this layout's children
        // and nothing else, which is what lets the client find the deepest
        // layer both documents share.
        "<main dp-layout={layout} id=\"outlet\">"
    } else {
        "<main>"
    };
    LAYOUT_RSX
        .replace("@HEAD@\n", &head)
        .replace("@BODY_CLASS@", body_class)
        .replace("@MAIN@", main_open)
        .replace(
            "@PARAMS@",
            if a.has("rsx-htmx") {
                "layout: &str, "
            } else {
                ""
            },
        )
}

const LAYOUT_RSX: &str = r#"// The document every page in this directory, and every directory below it,
// is rendered inside. A page never names it: which layout wraps a page is a
// property of where the page is, which is the rule the Crystal page_router
// uses for layout.cr.
//
// A layout is an ordinary component with a parameter of type `Element`, and
// `children` is the only parameter `html!` fills -- it cannot know what a
// second one would mean. A value every page shares comes from a const, as
// the title does here; a value belonging to one request belongs on the page
// component, which the handler builds and which already has the context.

use crate::pages::TITLE;

pub fn Layout(@PARAMS@children: Element) -> Element {
    <!DOCTYPE html>
    <html lang="en">
        <head>
            <meta charset="utf-8">
            <meta name="viewport" content="width=device-width, initial-scale=1">
            <title>{TITLE}</title>
@HEAD@
        </head>
        <body@BODY_CLASS@>
            <header><a href="/">{TITLE}</a></header>
            @MAIN@{children}</main>
        </body>
    </html>
}
"#;

/// `<Link>`: an anchor that swaps instead of reloading.
///
/// An ordinary `<a href>` first, enhanced second. With JavaScript off, with
/// a crawler, or on the first load of any page, it is a link and the browser
/// does what it always did. dp-swap turns it into a partial navigation only
/// where it can.
///
/// `to` takes what the route helpers return, so a link to a route that does
/// not exist does not compile -- `routes::users::id::path(7)`, never a
/// string. That is Crystal page_router's `UserPage.path(123)`.
const LINK_RSX: &str = r#"// An anchor that swaps instead of reloading the document.
//
// Written as a real <a href> and enhanced afterwards. With JavaScript off,
// to a crawler, and on the first load of any page it is a link and the
// browser does what it always did -- which is why `to` is required and why
// there is no version of this that renders a <span> with a click handler.
//
// `to` takes what the route helpers return:
//
//     <Link to={routes::users::id::path(7)}>User 7</Link>
//
// never a string. A route that does not exist is then a compile error
// rather than a 404 somebody finds later.

pub fn Link(to: &str, children: Element) -> Element {
    <a href={to} dp-boost>{children}</a>
}
"#;

/// The layout one directory down, to show that they stack.
fn users_layout_rsx(a: &Answers) -> String {
    if a.has("rsx-htmx") {
        return USERS_LAYOUT_RSX
            .replace("@PARAMS@", "layout: &str, ")
            .replace("@MARK@", " dp-layout={layout}");
    }
    USERS_LAYOUT_RSX
        .replace("@PARAMS@", "")
        .replace("@MARK@", "")
}

const USERS_LAYOUT_RSX: &str = r#"// Wraps every page under src/pages/users/, inside the root layout rather
// than instead of it. Nothing declares the stacking: `html!` reads it off
// the directory tree.
//
// `layout` is the URL prefix this layout covers, filled in by `html!`, and
// `dp-layout` is the marker dp-swap walks to find the deepest layer two
// pages share. It goes on the element wrapping exactly the children and
// nothing else: anything inside it is replaced along with them, so a
// sidebar in here would lose its scroll position on every navigation.

pub fn Layout(@PARAMS@children: Element) -> Element {
    <section class="users">
        <nav><a href="/">home</a></nav>
        <div@MARK@>{children}</div>
    </section>
}
"#;

fn index_rsx(a: &Answers) -> String {
    if a.has("rsx-htmx") {
        return INDEX_RSX
            .replace("@USE@", "use crate::pages::views::link::Link;\n\n")
            .replace(
                "<a href=\"/about\">/about</a>",
                "<Link to=\"/about\">/about</Link>",
            )
            .replace(
                "<a href=\"/users/7\">/users/7</a>",
                "<Link to={&crate::pages::routes::users::id::path(7)}>User 7</Link>",
            );
    }
    INDEX_RSX.replace("@USE@", "")
}

const INDEX_RSX: &str = r#"@USE@// The page, and only the page. The document around it is layout.rsx, and
// this file does not know it exists.

pub fn Index(title: &str) -> Element {
    <h1>{title}</h1>
    <p>
        This file is <code>src/pages/index.rsx</code>, beside the handler
        in <code>index.rs</code>, and it is compiled into the binary. Edit it
        and cargo rebuilds; break it and the build stops on the line you broke.
    </p>
    <p>
        An interpolated value is escaped on the way out, always. Writing
        markup verbatim needs a different and greppable hole, so an injection
        is something you can search for rather than something you have to
        remember.
    </p>
    <ul>
        <li><a href="/about">/about</a></li>
        <li><a href="/users/7">/users/7</a></li>
    </ul>
}
"#;

const ABOUT_RSX: &str = r#"// The parameters are this page's contract with its handler. Rename one and
// src/pages/about.rs stops compiling, which is the point.

pub fn About(title: &str, verb: &str, path: &str) -> Element {
    <h1>About</h1>
    <p>
        You asked for <code>{verb} {path}</code>. This page answers GET
        and nothing else, so a DELETE to it is a 405 carrying
        an <code>Allow</code> header rather than a 404.
    </p>
    <p>The shell around this is <code>src/pages/layout.rsx</code>, which {title} shares with every page.</p>
}
"#;

const USER_RSX: &str = r#"// Two layouts wrap this one: the root, and users/layout.rsx beside it.
// Neither is named here or anywhere else -- `html!` reads the directory
// tree. Delete users/layout.rsx and this page is wrapped in one.

pub fn User(id: u64) -> Element {
    <h1>User {id}</h1>
    <p>
        The <code>id</code> came out of the path. A component takes typed
        parameters, so this one is a <code>u64</code> the whole way
        from <code>src/pages/users/[id].rs</code>, never a string that
        might not be a number.
    </p>
}
"#;

// -- tailwind ----------------------------------------------------------------

/// The input of the CSS build. Never published: the leading underscore is
/// the mark `assets!` reads for a file that is bundled into another.
const TAILWIND_CSS: &str = r#"/* The input of the CSS build. Never published.
 *
 * The leading underscore marks an input: a file that is bundled into another
 * and that `assets!` must not publish on its own. The build reads this file
 * and writes assets/css/site.css, which is the one that ships:
 *
 *     mise run css          # once, minified
 *     mise run css:watch    # again on each change
 */

@import "tailwindcss";

/* Where the class names are.
 *
 * Tailwind resolves each path against this stylesheet and keeps its automatic
 * detection on as well. These are explicit for two reasons: `.rsx` is not an
 * extension Tailwind knows, and an explicit glob states the contract.
 */

/* The templates. A .rsx file is Rust with an HTML-shaped body. */
@source "../../src/pages/**/*.rsx";

/* Plain Rust holds class names too -- a helper that builds a class string,
 * or a handler choosing one. */
@source "../../src/**/*.rs";
"#;

/// Written so the first `cargo build` works before anyone has run node.
///
/// Without it `assets::url("css/site.css")` names a file nothing publishes,
/// and a brand-new project would fail on its first request with a message
/// about an asset rather than about node. depoth's build script writes
/// placeholders for the same reason.
const SITE_CSS_PLACEHOLDER: &str = "/* Written by `brocanth create` so the \
                                    first build works.\n   `mise run css` \
                                    replaces it with the real one. */\n";

const BUILD_CSS_SH: &str = r##"#!/usr/bin/env bash
#
# Build assets/css/site.css from assets/css/_tailwind.css.
#
#   scripts/build-css.sh            once, minified
#   scripts/build-css.sh --watch    again on each change
#
# Runs the Tailwind CLI out of node_modules and does not fall back to `npx`,
# because npx downloads a package when the local one is absent and that hides
# the real fault: nobody ran `npm install`.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

INPUT="assets/css/_tailwind.css"
OUTPUT="assets/css/site.css"
CLI="node_modules/.bin/tailwindcss"

if [ ! -x "$CLI" ]; then
  echo "error: the Tailwind CLI is absent at $ROOT/$CLI" >&2
  echo "       run 'npm install' in $ROOT first." >&2
  exit 1
fi

if [ "${1:-}" = "--watch" ]; then
  exec "$CLI" --input "$INPUT" --output "$OUTPUT" --watch
fi

"$CLI" --input "$INPUT" --output "$OUTPUT" --minify

if [ ! -s "$OUTPUT" ]; then
  echo "error: the build wrote no CSS to $ROOT/$OUTPUT" >&2
  exit 1
fi

# The stylesheet is embedded at compile time, so a new one is a new binary.
# Touching the module that embeds it is what makes cargo notice.
touch src/assets.rs

echo "built $OUTPUT ($(wc -c < "$OUTPUT" | tr -d ' ') bytes)"
"##;

fn package_json(a: &Answers) -> String {
    format!(
        r#"{{
  "name": {name:?},
  "version": "0.0.0",
  "private": true,
  "description": "The CSS build. Nothing in the Rust binary needs node; this does.",
  "scripts": {{
    "css": "bash scripts/build-css.sh",
    "css:watch": "bash scripts/build-css.sh --watch"
  }},
  "devDependencies": {{
    "@tailwindcss/cli": "^4.1.14"
  }}
}}
"#,
        name = a.name
    )
}

fn plugins_mod_rs(built: &Built) -> String {
    let mut out = String::from(
        "//! The plugins this application contains.\n\
         //!\n\
         //! Each one is also named in `main`: being here makes it compile, being named\n\
         //! there makes it run. The core depends on none of them, and the dependency\n\
         //! arrow points this way and never back.\n\
         \n",
    );
    // Sorted, not in the order the modules ran. rustfmt orders `mod`
    // declarations alphabetically, and a scaffold that hands a developer a
    // diff on their first `cargo fmt` is a scaffold that got it wrong.
    let mut lines: Vec<&str> = built.slot(Slot::PluginsMod).collect();
    lines.sort_unstable();
    for line in lines {
        out.push_str(line);
    }
    out
}

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
         name = {title:?}\n",
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
            ("everything", all()),
            ("rsx", vec!["rsx"]),
            ("rsx-tailwind", vec!["rsx", "rsx-tailwind"]),
            ("rsx-htmx", vec!["rsx", "rsx-htmx"]),
            ("rsx-tailwind-htmx", vec!["rsx", "rsx-tailwind", "rsx-htmx"]),
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
                let mut child = Command::new("rustfmt")
                    .args(["--edition", "2021", "--check"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .expect("rustfmt runs");
                child
                    .stdin
                    .take()
                    .expect("a pipe")
                    .write_all(source.as_bytes())
                    .expect("rustfmt reads");
                // Reading a file from stdin, rustfmt reports by printing a
                // diff and still exits 0, so the empty stdout is the signal.
                let done = child.wait_with_output().expect("rustfmt finishes");
                if !done.stdout.is_empty() {
                    unformatted.push(format!("{name}: {path}"));
                }
            }
        }
        assert!(
            unformatted.is_empty(),
            "rustfmt would change these:\n  {}",
            unformatted.join("\n  ")
        );
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
        assert!(
            !paths.iter().any(|p| p.starts_with("src/plugins")),
            "{paths:?}"
        );
        assert!(!paths.iter().any(|p| p.starts_with("config/")), "{paths:?}");
        // The route table is not optional: it is the core.
        assert!(paths.contains(&"src/pages/mod.rs".to_string()));
    }

    #[test]
    fn a_plugin_writes_its_file_and_nobody_elses() {
        let paths = paths(&["logging"]);
        assert!(paths.contains(&"src/plugins/logging.rs".to_string()));
        assert!(!paths.contains(&"src/plugins/sessions.rs".to_string()));
        assert!(!paths.contains(&"src/plugins/model.rs".to_string()));
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
        let everything = cargo_toml(&answers(&all()), &built(&all()));
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
        let main = app_rs(
            &answers(&["logging", "sessions", "config"]),
            &built(&["logging", "sessions", "config"]),
        );
        assert!(
            main.contains(".plugin_from_config::<plugins::logging::Logging>(\"logging\")"),
            "{main}"
        );
        assert!(
            main.contains(".plugin_from_config::<plugins::sessions::Sessions>(\"sessions\")"),
            "{main}"
        );
        // The mount is in the application's setup block, never in the plugin.
        assert!(
            main.contains("setup.router().mount(\"/{*rest}\", \"request_log\")"),
            "{main}"
        );
        let plugin = crate::modules::logging::plugin_rs(&project(&["logging"]));
        assert!(
            !plugin.contains(".mount("),
            "a plugin must not mount: {plugin}"
        );
        assert!(plugin.contains("catalog.offer("), "{plugin}");
    }

    #[test]
    fn a_core_only_app_reads_no_file_and_registers_nothing() {
        let main = app_rs(&answers(&[]), &built(&[]));
        assert!(!main.contains("Loader::new"), "{main}");
        // `.plugin(` and not `.plugin`: `App` has a `plugins` field now, and
        // an assertion that matches it is an assertion about nothing.
        assert!(!main.contains(".plugin("), "{main}");
        assert!(!main.contains(".plugin_from_config"), "{main}");
        assert!(!main.contains("use crate::plugins;"), "{main}");
        assert!(!main.contains("brocanth_config"), "{main}");
    }

    #[test]
    fn without_the_config_plugin_a_plugin_is_built_from_a_literal() {
        let main = app_rs(&answers(&["logging"]), &built(&["logging"]));
        // Already wrapped, because it is longer than rustfmt's chain_width
        // and a project must survive its own first `cargo fmt`.
        assert!(
            main.contains(".plugin(plugins::logging::Logging::new(\n")
                && main.contains("plugins::logging::Settings::default(),"),
            "{main}"
        );
        assert!(!main.contains("plugin_from_config"), "{main}");
        // `Plugin::new` is a trait method, so this branch -- and only this
        // branch -- needs the trait in scope.
        assert!(main.contains("use brocanth::plugin_api::Plugin;"), "{main}");
    }

    #[test]
    fn the_generated_config_names_only_sections_a_plugin_claims() {
        // `validate_sections` rejects a section nothing claims, so a scaffold
        // that wrote `[sessions]` without the sessions plugin would fail on its
        // own first run. The real crate is asked, not a reimplementation.
        for picks in [vec!["config"], vec!["config", "logging"], all().to_vec()] {
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
        let everything = written(&all());
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
        let a = answers(&all());
        let table = pages_mod_rs(&a);
        assert!(
            table.contains(r#"brocanth_pages::pages!("src/pages");"#),
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
        assert!(!pages_mod_rs(&a).contains("pub fn html("));
        assert!(!main_with_commands_rs(&a, &built(picks)).contains("mod views;"));
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
        let pages = pages_mod_rs(&answers(&["rsx"]));
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
        let a = answers(&["rsx"]);
        for template in [INDEX_RSX, ABOUT_RSX, USER_RSX] {
            assert!(
                !template.contains("<Layout"),
                "a page names its own shell:\n{template}"
            );
        }
        for page in [index_rs(&a), about_rs(&a), user_rs(&a)] {
            assert!(
                !page.contains("Layout"),
                "a handler names a layout:\n{page}"
            );
        }
        let pages = pages_mod_rs(&a);
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
    fn every_generated_toml_line_starts_in_the_first_column() {
        // A stray indent in an assembled string is valid TOML and still
        // wrong: the reader cannot tell a continuation from a mistake.
        for a in [answers(&["rsx", "rsx-tailwind"]), answers(&[])] {
            for line in mise_toml(&a).lines() {
                assert_eq!(line, line.trim_start(), "indented: {line:?}");
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
        let pages = pages_mod_rs(&answers(&["rsx"]));
        assert!(
            pages.contains("pub fn render(content: impl rsx::Template)"),
            "{pages}"
        );
    }

    #[test]
    fn a_handler_reaches_a_template_through_one_function_and_no_other() {
        let a = answers(&["rsx"]);
        let pages = pages_mod_rs(&a);
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
            pages_mod_rs(&a).contains(&format!("pub const TITLE: &str = {:?};", a.title)),
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
        let pages = pages_mod_rs(&answers(&["rsx"]));
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
            picks: vec!["logging", "config", "sessions", "rsx"],
            local: Some(brocanth.display().to_string()),
            git: false,
        };
        for (path, contents) in scaffold(&a).expect("the modules agree") {
            let full = root.join(&path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(&full, contents).unwrap();
        }
        let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .arg("build")
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
}
