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

use clap::{Args, ValueEnum};

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
    #[arg(
        long = "with",
        value_enum,
        value_name = "PLUGIN",
        value_delimiter = ',',
        conflicts_with = "core_only"
    )]
    pub with: Vec<Pick>,
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

/// The plugin set on offer.
///
/// Each one is a capability the core deliberately does not have. The core is a
/// router handing a request and a context to a handler, with three
/// dependencies; everything below is something an application opts into by
/// naming it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Pick {
    /// A request logger. The core assumes no log layer at all
    Logging,
    /// TOML configuration: four sources, merged, typed per section
    Config,
    /// Cookie sessions, and a `Session` on the context
    Sessions,
    /// The model layer: SeaORM, validations and migrations
    Model,
    /// Project commands: a `manage` binary whose subcommands are your own structs
    Commands,
}

impl Pick {
    const ALL: [Pick; 5] = [
        Pick::Logging,
        Pick::Config,
        Pick::Sessions,
        Pick::Model,
        Pick::Commands,
    ];
    /// What a non-interactive run gets when no flag chose. Stated rather than
    /// guessed at: the command prints the flag that overrides it.
    const DEFAULT: [Pick; 3] = [Pick::Logging, Pick::Config, Pick::Commands];

    fn key(self) -> &'static str {
        match self {
            Pick::Logging => "logging",
            Pick::Config => "config",
            Pick::Sessions => "sessions",
            Pick::Model => "model",
            Pick::Commands => "commands",
        }
    }

    /// The one line a reader needs to decide. The right-hand half is what the
    /// binary pays, because that is the question §8 says to keep asking.
    fn gist(self) -> &'static str {
        match self {
            Pick::Logging => "a line per request; the core assumes no log layer",
            Pick::Config => "config/<env>.toml, merged; without it nothing links a TOML parser",
            Pick::Sessions => "a cookie, and a `Session` the context hands a handler",
            Pick::Model => "SeaORM, validations, migrations; without it no SQL driver at all",
            Pick::Commands => "this project's own command line, `serve` plus whatever you add",
        }
    }

    /// The crate a choice puts in the tree, if any.
    fn crate_name(self) -> Option<&'static str> {
        match self {
            Pick::Config => Some("brocanth-config"),
            Pick::Model => Some("brocanth-model"),
            Pick::Commands => Some("brocanth-command"),
            Pick::Logging | Pick::Sessions => None,
        }
    }
}

impl std::fmt::Display for Pick {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:<9}-- {}", self.key(), self.gist())
    }
}

/// The answers, however they arrived.
struct Answers {
    /// The cargo package name, and the directory.
    name: String,
    /// The human name.
    title: String,
    picks: Vec<Pick>,
    /// A local checkout to depend on, instead of the repository.
    local: Option<String>,
    git: bool,
}

impl Answers {
    fn has(&self, pick: Pick) -> bool {
        self.picks.contains(&pick)
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
            let names: Vec<&str> = self.picks.iter().map(|p| p.key()).collect();
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

    /// Plugins that write a file under `src/plugins/`.
    fn plugins(&self) -> Vec<Pick> {
        self.picks
            .iter()
            .copied()
            // Config is not a plugin instance, and Tasks is a second binary
            // rather than anything the application registers: neither belongs
            // in the list `main` mounts.
            .filter(|p| *p != Pick::Config && *p != Pick::Commands)
            .collect()
    }

    /// Whether anything deserialises.
    ///
    /// Only the config plugin makes that true: `Plugin::Config` carries no
    /// bounds the core imposes, so a plugin built from a literal in `main`
    /// needs no deserialiser and the scaffold derives none.
    fn needs_serde(&self) -> bool {
        self.has(Pick::Config) && !self.plugins().is_empty()
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

    let files = scaffold(&answers);
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
            Cell::styled(pick.key(), ACCENT),
            Cell::styled(pick.gist(), DIM),
        ]);
    }
    for pick in Pick::ALL.iter().filter(|p| !answers.has(**p)) {
        contents.row(vec![
            Cell::styled(pick.key(), DIM),
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
    if answers.has(Pick::Commands) {
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
fn picked(args: &CreateArgs) -> Vec<Pick> {
    if args.core_only {
        Vec::new()
    } else if !args.with.is_empty() {
        let mut picks = args.with.clone();
        picks.sort();
        picks.dedup();
        picks
    } else {
        Pick::DEFAULT.to_vec()
    }
}

/// The full-screen route. Returns the same [`Answers`] the flags produce.
fn chooser(ui: &Ui, args: &CreateArgs, local: Option<String>) -> Result<Answers, Vec<Diagnostic>> {
    let preset = picked(args);
    let draft = Draft {
        name: String::new(),
        title: args.title.clone().unwrap_or_default(),
        choices: Pick::ALL
            .iter()
            .map(|pick| Choice {
                label: pick.key().to_string(),
                blurb: pick.gist().to_string(),
                on: preset.contains(pick),
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

    let picks = Pick::ALL
        .iter()
        .zip(&draft.choices)
        .filter(|(_, choice)| choice.on)
        .map(|(pick, _)| *pick)
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

fn scaffold(a: &Answers) -> Vec<(String, String)> {
    let mut files = vec![
        ("Cargo.toml".to_string(), cargo_toml(a)),
        (".gitignore".to_string(), "/target\n".to_string()),
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
            MISE_TOML.to_string(),
        ),
        (
            "src/main.rs".to_string(),
            if a.has(Pick::Commands) {
                main_with_commands_rs(a)
            } else {
                main_serve_only_rs(a)
            },
        ),
        // Always: it is the one answer to what this application is, and both
        // shapes of `main` read it.
        ("src/app.rs".to_string(), app_rs(a)),
        ("src/pages/mod.rs".to_string(), PAGES_MOD_RS.to_string()),
        ("src/pages/index.rs".to_string(), index_rs(a)),
        ("src/pages/about.rs".to_string(), about_rs(a)),
        ("src/pages/users/[id].rs".to_string(), USER_RS.to_string()),
    ];
    if !a.plugins().is_empty() {
        files.push(("src/plugins/mod.rs".to_string(), plugins_mod_rs(a)));
    }
    if a.has(Pick::Logging) {
        files.push(("src/plugins/logging.rs".to_string(), logging_rs(a)));
    }
    if a.has(Pick::Sessions) {
        files.push(("src/plugins/sessions.rs".to_string(), sessions_rs(a)));
    }
    if a.has(Pick::Model) {
        files.push(("src/plugins/model.rs".to_string(), model_rs(a)));
    }
    if a.has(Pick::Config) {
        files.push((
            format!("config/{}.toml", brocanth_config::DEFAULT_ENV),
            config_toml(a),
        ));
    }
    if a.has(Pick::Commands) {
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
    files.push(("AGENTS.md".to_string(), agents_md(a)));
    files
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
        chosen.push(format!("`{}`  {}", pick.key(), pick.gist()));
    }
    let absent: Vec<String> = Pick::ALL
        .iter()
        .filter(|p| !a.has(**p))
        .map(|p| format!("`{}`", p.key()))
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

    let commands = if a.has(Pick::Commands) {
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
         brocanth check     # wiring first: mounts and middleware offers, then cargo\n\
         cargo test\n\
         cargo clippy --all-targets -- -D warnings\n\
         cargo fmt --check\n\
         ```\n\
         \n\
         `brocanth routes` prints the route table and, below it, every mounted\n\
         middleware with the file and line where it was mounted. Use it instead\n\
         of guessing what covers a request.\n\
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
         `src/pages/users/[id].rs` is `/users/{{id}}`. Two edits:\n\
         \n\
         1. Write `src/pages/<name>.rs` returning `Result<impl IntoResponse, Error>`.\n\
         2. In `src/pages/mod.rs`, add `pub mod <name>;` and a `routes.file(..)`\n\
         \x20  row. It names the **file**, not the URL, so the table and the\n\
         \x20  filesystem cannot drift apart.\n\
         \n\
         A handler returns a value and never writes a response itself. That is\n\
         what makes a half-written body on an error impossible rather than\n\
         unlikely.\n\
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
        commands = commands,
    )
}

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
fn source_of(a: &Answers, crate_name: &str) -> String {
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
                "{crate_name} = {{ path = {:?} }}",
                dir.display().to_string()
            )
        }
        None => format!("{crate_name} = {{ git = {REPOSITORY:?} }}"),
    }
}

/// The manifest, and the comment that makes it a statement rather than a list.
fn cargo_toml(a: &Answers) -> String {
    let mut deps = vec![source_of(a, "brocanth")];
    for pick in &a.picks {
        if let Some(name) = pick.crate_name() {
            deps.push(source_of(a, name));
        }
    }
    {
        // Every project serves, so every project gets a socket and a reactor.
        deps.push(source_of(a, "brocanth-server"));
        deps.push(
            "tokio = { version = \"1\", features = [\"rt-multi-thread\"] }  # the reactor `serve` runs on"
                .to_string(),
        );
    }
    if a.needs_serde() {
        deps.push(
            "serde = { version = \"1\", features = [\"derive\"] }  # the config plugin reads typed sections"
                .to_string(),
        );
    }

    let mut absent: Vec<String> = Vec::new();
    if !a.has(Pick::Model) {
        absent.push(
            "#   brocanth-model   no ORM, no SQL driver, no sea-orm anywhere in the lock file"
                .to_string(),
        );
    }
    if !a.has(Pick::Config) {
        absent.push(
            "#   brocanth-config  no TOML parser; main builds each plugin from a literal"
                .to_string(),
        );
    }
    if !a.needs_serde() {
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
    let default_run = if a.has(Pick::Commands) {
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
         {deps}\n",
        name = a.name,
        default_run = default_run,
        deps = deps.join("\n"),
        absent = if absent.contains("#   ") {
            absent
        } else {
            String::new()
        },
    )
}

/// `main`, which is the modularity claim written out as a numbered list.
fn app_rs(a: &Answers) -> String {
    let upper = a.ident().to_uppercase();
    let plugins = a.plugins();
    let layered = a.has(Pick::Logging) || a.has(Pick::Sessions);
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
         \n\
         use brocanth::plugin_app::AppBuilder;\n",
        title = a.title
    ));
    out.push_str(if layered {
        "use brocanth::routing::{Dispatch, MiddlewareStack, Next, Router, Verb};\n"
    } else {
        "use brocanth::routing::{Dispatch, MiddlewareStack, Router, Verb};\n"
    });
    if a.has(Pick::Config) {
        out.push_str("use brocanth_config::{AppBuilderConfigExt, Loader};\n");
    }
    out.push_str("\nuse crate::pages::{self, Ctx, Handler, IntoResponse, Response};\n");
    if !plugins.is_empty() {
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
    if a.has(Pick::Config) {
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
        for pick in &plugins {
            out.push_str(&format!(
                "        .plugin_from_config::<plugins::{key}::{ty}>({key:?})\n\
                \x20       .map_err(|e| e.to_string())?\n",
                key = pick.key(),
                ty = type_of(*pick),
            ));
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
        for pick in &plugins {
            out.push_str(&format!(
                "        .plugin(plugins::{key}::{ty}::new(plugins::{key}::Settings::default()))\n",
                key = pick.key(),
                ty = type_of(*pick),
            ));
        }
    }

    out.push_str(
        "        .setup(|setup| {\n\
        \x20           // A plugin *offers* middleware and cannot mount it: a\n\
        \x20           // `MiddlewareCatalog` has no `mount`. Mounting takes a pattern, the\n\
        \x20           // pattern is a decision, and the decision is the application's.\n",
    );
    if a.has(Pick::Logging) {
        out.push_str("            setup.router().mount(\"/{*rest}\", \"request_log\");\n");
    }
    if a.has(Pick::Sessions) {
        out.push_str("            setup.router().mount(\"/users/{*rest}\", \"session\");\n");
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
        if a.has(Pick::Logging) {
            out.push_str("                \"request_log\" => plugins::logging::layer,\n");
        }
        if a.has(Pick::Sessions) {
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
    if a.has(Pick::Sessions) || a.has(Pick::Model) {
        out.push_str(
            "\n    // The per-request context. These lines compile only because the\n\
            \x20   // plugins that provide the values were registered in step 2: the\n\
            \x20   // installed provider set lives in the application's *type*. Delete a\n\
            \x20   // registration above and this stops compiling, which is the whole\n\
            \x20   // mechanism -- modularity enforced rather than documented.\n\
            \x20   let cx = app.context();\n",
        );
        if a.has(Pick::Sessions) {
            out.push_str(
                "    let session: &plugins::sessions::Session = cx.extract();\n\
                \x20   eprintln!(\"session cookie: {}\", session.cookie);\n",
            );
        }
        if a.has(Pick::Model) {
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
         \n\
         /// Serves until the process is stopped.\n\
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
        \x20               let answered = dispatch(&app, request.verb, &request.path);\n\
        \x20               async move {\n\
        \x20                   let mut out = brocanth_server::Response::html(answered.status, answered.body);\n\
        \x20                   for (name, value) in &answered.headers {\n\
        \x20                       out = out.with_header(name, value);\n\
        \x20                   }\n\
        \x20                   out\n\
        \x20               }\n\
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
         pub fn dispatch(app: &App, verb: Verb, path: &str) -> Response {\n\
        \x20   match app.router.dispatch(verb, path) {\n\
        \x20       Dispatch::Found { handler, matched } => {\n\
        \x20           let mut ctx = Ctx::new(verb, path, matched.params());\n\
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
    out
}

fn type_of(pick: Pick) -> &'static str {
    match pick {
        Pick::Logging => "Logging",
        Pick::Sessions => "Sessions",
        Pick::Model => "Model",
        Pick::Config => "ConfigPlugin",
        // Never reached: `plugins()` drops it before this is called, because
        // commands are a binary beside the application, not a layer inside it.
        Pick::Commands => unreachable!("commands is not a plugin"),
    }
}

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
//! contains is in `src/app.rs`; `brocanth create --with commands` turns this
//! into a command line with `serve` as one of its commands.

mod app;
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
        plugins = if a.plugins().is_empty() {
            ""
        } else {
            "mod plugins;\n"
        },
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

mod app;
mod commands;
mod pages;
{plugins}
fn main() -> brocanth_command::anyhow::Result<()> {{
    brocanth_command::main(brocanth_command::Context::boot()?)
}}
"##,
        title = a.title,
        plugins = if a.plugins().is_empty() {
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

const MISE_TOML: &str = r#"[tools]
# Must match the toolchain brocanth's proc-macro artifacts were built with:
# they are not compatible across patch releases.
rust = "1.98"

[tasks.run]
run = "cargo run"

[tasks.test]
run = "cargo test"
"#;

const PAGES_MOD_RS: &str = r##"//! The route table.
//!
//! One file is one route, and the path on disk is the URL: `index.rs` is `/`,
//! `about.rs` is `/about`, `users/[id].rs` is `/users/{id}`. The framework does
//! the conversion -- see [`router`] -- so the table and the filesystem cannot
//! drift apart, and renaming a file renames a route.

pub mod about;
pub mod index;

pub mod users {
    // `[id].rs` is not a Rust identifier, so the module says where to look.
    // The typed-route-table macro emits this; until it lands it is written
    // once, here.
    #[path = "[id].rs"]
    pub mod id;
}

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
    params: Vec<(String, String)>,
}

impl Ctx {
    pub fn new(verb: Verb, path: &str, params: Params<'_>) -> Self {
        Self {
            verb,
            path: path.to_string(),
            params: params
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
        }
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

/// The table.
///
/// Each line names the *file* and the framework derives the route from it, so
/// there is no second list to keep in step. Registering the same file and verb
/// twice is an error rather than a silent overwrite.
pub fn router() -> Result<Router<Handler>, BuildError> {
    let mut routes = Router::builder();
    routes.file("index.rs", "", Verb::Get, page(index::handle))?;
    routes.file("about.rs", "", Verb::Get, page(about::handle))?;
    routes.file("users/[id].rs", "", Verb::Get, page(users::id::handle))?;
    routes.file(
        "users/[id].rs",
        "",
        Verb::Post,
        page(users::id::handle_post),
    )?;
    routes.build()
}

#[cfg(test)]
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

fn index_rs(a: &Answers) -> String {
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

const USER_RS: &str = r##"//! `/users/{id}` -- the brackets in the file name are the parameter.
//!
//! There is no ceiling on how many a route may have. The Crystal prototype had
//! `MAX_PARAMS = 4` and dropped the fifth without saying so, because its
//! collector could not prove a captured parameter does not outlive the request.
//! Here the parameters borrow the path, so the question does not arise.

use crate::pages::{Ctx, Error, IntoResponse, Response};

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
pub fn handle_post(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let id: u64 = ctx.require("id")?.parse()?;

    Ok(Response::empty(303).with_header("location", format!("/users/{id}")))
}
"##;

fn plugins_mod_rs(a: &Answers) -> String {
    let mut out = String::from(
        "//! The plugins this application contains.\n\
         //!\n\
         //! Each one is also named in `main`: being here makes it compile, being named\n\
         //! there makes it run. The core depends on none of them, and the dependency\n\
         //! arrow points this way and never back.\n\
         \n",
    );
    for pick in [Pick::Logging, Pick::Model, Pick::Sessions] {
        if a.has(pick) {
            out.push_str(&format!("pub mod {};\n", pick.key()));
        }
    }
    out
}

/// The derive a settings struct carries, and the sentence that explains it.
///
/// This is the §8 claim at its sharpest: with the config plugin the struct is
/// deserialised and derives for it; without, nothing deserialises anything and
/// the derive is not there to be compiled.
fn settings_derive(a: &Answers, section: &str) -> (String, String) {
    if a.has(Pick::Config) {
        (
            "#[derive(Debug, serde::Deserialize)]\n#[serde(default, deny_unknown_fields)]"
                .to_string(),
            format!(
                "It derives `Deserialize` because this application reads it from\n\
                 /// `[{section}]` in a TOML file. Drop the config plugin and the derive goes\n\
                 /// with it: `Plugin::Config` carries no bounds the core imposes."
            ),
        )
    } else {
        (
            "#[derive(Debug)]".to_string(),
            "Nothing deserialises it: `main` builds it from a literal, and the\n\
             /// compiler checks that literal. `Plugin::Config` carries no bounds the\n\
             /// core imposes, so there is no deserialiser here to link."
                .to_string(),
        )
    }
}

fn logging_rs(a: &Answers) -> String {
    let (derive, note) = settings_derive(a, "logging");
    format!(
        r##"//! Request logging.
//!
//! The core assumes no log layer at all -- not `log`, not `tracing`, not a
//! trait with a no-op default. This file is the whole of logging in this
//! binary: delete the registration in `main` and nothing logs, delete this file
//! and nothing about logging is compiled.

use brocanth::plugin_api::{{Lifecycle, MiddlewareCatalog, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::routing::Next;

use crate::pages::{{Ctx, Response}};

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// `error`, `warn`, `info` or `debug`.
    pub level: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            level: "info".to_string(),
        }}
    }}
}}

pub struct Logging {{
    level: String,
}}

impl Plugin for Logging {{
    type Config = Settings;
    /// Nothing on the context: a logger writes, it does not hand values out.
    type Provides = NoContext;
    const NAME: &'static str = "logging";

    fn new(config: Settings) -> Self {{
        Self {{
            level: config.level,
        }}
    }}

    fn configure_router(&mut self, catalog: &mut MiddlewareCatalog<'_>) -> Result<(), PluginError> {{
        // Offered, not mounted. Where it runs is the application's call, made
        // in `main`, at a pattern this file never sees.
        catalog.offer("request_log", "one line per request: verb, path, status");
        Ok(())
    }}

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        eprintln!("[{{}}] level {{}}", lc.instance(), self.level);
        Ok(())
    }}
}}

/// The layer `request_log` mounts.
///
/// A plain function is a layer: `Layer` is implemented for anything callable
/// with a context and the rest of the chain. Not calling `next.run` is how a
/// layer short-circuits, and dropping it is all that takes.
pub fn layer(ctx: &mut Ctx, next: Next<'_, Ctx, Response>) -> Response {{
    let verb = ctx.verb;
    let path = ctx.path.clone();
    let response = next.run(ctx);
    eprintln!("{{verb}} {{path}} -> {{}}", response.status);
    response
}}
"##
    )
}

fn sessions_rs(a: &Answers) -> String {
    let (derive, note) = settings_derive(a, "sessions");
    format!(
        r##"//! Cookie sessions, and the one value they put on the context.

use brocanth::plugin_api::{{Lifecycle, MiddlewareCatalog, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth::routing::Next;

use crate::pages::{{Ctx, Response}};

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    pub cookie: String,
    /// `Secure`, so the cookie never leaves over plain HTTP. False in
    /// development and nowhere else.
    pub secure: bool,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            cookie: "_{ident}_session".to_string(),
            secure: true,
        }}
    }}
}}

/// What this plugin puts on the context.
///
/// One provider owns one type, so there is no string name to misspell and no
/// `responds_to?` guard to forget. A handler that asks for a `Session` in an
/// application that never registered this plugin does not compile.
#[derive(Debug, Default)]
pub struct Session {{
    pub cookie: String,
}}

impl ContextItem for Session {{}}

pub struct Sessions {{
    cookie: String,
    secure: bool,
}}

impl Plugin for Sessions {{
    type Config = Settings;
    type Provides = ProvidesContext<Session>;
    const NAME: &'static str = "sessions";

    fn new(config: Settings) -> Self {{
        Self {{
            cookie: config.cookie,
            secure: config.secure,
        }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.cookie.is_empty() {{
            // A missing or ill-typed key fails at startup, not at first use,
            // and the plugin never quietly chooses a name the operator did not.
            return Err(PluginError::new("[sessions] cookie must not be empty"));
        }}
        Ok(())
    }}

    fn configure_router(&mut self, catalog: &mut MiddlewareCatalog<'_>) -> Result<(), PluginError> {{
        catalog.offer(
            "session",
            "reads the cookie and puts a Session on the context",
        );
        Ok(())
    }}

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if !self.secure {{
            eprintln!("[sessions] Secure is off: development only");
        }}
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        cx.insert(Session {{
            cookie: self.cookie.clone(),
        }});
    }}
}}

/// The layer `session` mounts. Reading the cookie off the request and writing
/// `Set-Cookie` on the way out both need the request type that arrives with the
/// HTTP server; the seam is here and it is already the right shape.
pub fn layer(ctx: &mut Ctx, next: Next<'_, Ctx, Response>) -> Response {{
    next.run(ctx)
}}
"##,
        ident = a.ident()
    )
}

fn model_rs(a: &Answers) -> String {
    let (derive, note) = settings_derive(a, "model");
    format!(
        r##"//! The model layer.
//!
//! The ORM is recommended and absent. SeaORM is the recommendation, it is not a
//! dependency of the core and never will be, and it arrives through
//! `brocanth-model` -- which is in `Cargo.toml` because this file is here and
//! through no other route. An application that stores nothing pays nothing for
//! it: not a crate in the lock file, not a section in the config, not a byte.
//!
//! The entity modules go under `src/models/`, from `brocanth generate model`.

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth_model::ModelRegistry;

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// `sqlite://db/development.sqlite?mode=rwc`, or a Postgres URL.
    pub url: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            url: "sqlite://db/development.sqlite?mode=rwc".to_string(),
        }}
    }}
}}

/// The handle a handler asks the context for.
pub struct Db {{
    pub url: String,
}}

impl ContextItem for Db {{}}

pub struct Model {{
    url: String,
}}

impl Plugin for Model {{
    type Config = Settings;
    type Provides = ProvidesContext<Db>;
    const NAME: &'static str = "model";

    fn new(config: Settings) -> Self {{
        Self {{ url: config.url }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.url.is_empty() {{
            return Err(PluginError::new(
                "[model] url is required; there is no default database",
            ));
        }}
        Ok(())
    }}

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // The pool opens here, at the last hook before the server is handed the
        // application, and drains on the matching shutdown hook.
        //
        // The registry below is what makes the generators and an admin UI
        // tractable: every entity compiled into this binary can be enumerated,
        // with its columns, nullability and keys. It is empty until the first
        // `brocanth generate model`, and the line is here so it is not a
        // surprise when it fills.
        let registry = ModelRegistry::new();
        eprintln!("[model] {{}} entities registered", registry.names().len());
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        cx.insert(Db {{
            url: self.url.clone(),
        }});
    }}
}}
"##
    )
}

fn config_toml(a: &Answers) -> String {
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
    if a.has(Pick::Logging) {
        out.push_str("\n[logging]\nlevel = \"debug\"\n");
    }
    if a.has(Pick::Sessions) {
        out.push_str(&format!(
            "\n[sessions]\ncookie = \"_{ident}_session\"\n\
             # Development only. Anywhere a browser reaches over HTTPS, this is true.\n\
             secure = false\n",
            ident = a.ident()
        ));
    }
    if a.has(Pick::Model) {
        out.push_str("\n[model]\nurl = \"sqlite://db/development.sqlite?mode=rwc\"\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answers(picks: &[Pick]) -> Answers {
        Answers {
            name: "my-blog".into(),
            title: "My Blog".into(),
            picks: picks.to_vec(),
            local: None,
            git: false,
        }
    }

    fn paths(picks: &[Pick]) -> Vec<String> {
        scaffold(&answers(picks))
            .into_iter()
            .map(|(p, _)| p)
            .collect()
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
        let paths = paths(&[Pick::Logging]);
        assert!(paths.contains(&"src/plugins/logging.rs".to_string()));
        assert!(!paths.contains(&"src/plugins/sessions.rs".to_string()));
        assert!(!paths.contains(&"src/plugins/model.rs".to_string()));
    }

    #[test]
    fn the_manifest_holds_exactly_what_was_chosen() {
        // The §8 claim, as an assertion rather than a sentence in a README.
        // Dependency *lines*, not mentions: the comment above `[dependencies]`
        // names every crate that is absent, which is the point of it.
        let core = cargo_toml(&answers(&[]));
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
        let logging = cargo_toml(&answers(&[Pick::Logging]));
        assert!(!declared(&logging, "serde"), "{logging}");
        assert!(!declared(&logging, "brocanth-config"), "{logging}");

        let configured = cargo_toml(&answers(&[Pick::Logging, Pick::Config]));
        assert!(declared(&configured, "brocanth-config"), "{configured}");
        assert!(declared(&configured, "serde"), "{configured}");

        let with_model = cargo_toml(&answers(&[Pick::Model]));
        assert!(declared(&with_model, "brocanth-model"), "{with_model}");
    }

    #[test]
    fn the_manifest_says_what_is_absent_and_why() {
        let core = cargo_toml(&answers(&[]));
        assert!(
            core.contains("Not here, because nothing asked for it"),
            "{core}"
        );
        assert!(
            core.contains("no sea-orm anywhere in the lock file"),
            "{core}"
        );
        let everything = cargo_toml(&answers(&Pick::ALL));
        assert!(
            !everything.contains("Not here, because"),
            "nothing is absent, so nothing is listed: {everything}"
        );
    }

    #[test]
    fn the_config_plugin_is_what_puts_a_deserialiser_in_the_source() {
        // "A feature that is off must mean the code is absent, not that a
        // branch returns early." Two scaffolds, and the difference is a derive.
        let configured = logging_rs(&answers(&[Pick::Logging, Pick::Config]));
        assert!(configured.contains("serde::Deserialize"), "{configured}");
        assert!(configured.contains("deny_unknown_fields"), "{configured}");

        let literal = logging_rs(&answers(&[Pick::Logging]));
        assert!(!literal.contains("serde"), "{literal}");
    }

    #[test]
    fn app_names_every_plugin_and_mounts_what_no_plugin_may() {
        let main = app_rs(&answers(&[Pick::Logging, Pick::Sessions, Pick::Config]));
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
        let plugin = logging_rs(&answers(&[Pick::Logging]));
        assert!(
            !plugin.contains(".mount("),
            "a plugin must not mount: {plugin}"
        );
        assert!(plugin.contains("catalog.offer("), "{plugin}");
    }

    #[test]
    fn a_core_only_app_reads_no_file_and_registers_nothing() {
        let main = app_rs(&answers(&[]));
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
        let main = app_rs(&answers(&[Pick::Logging]));
        assert!(
            main.contains(
                ".plugin(plugins::logging::Logging::new(plugins::logging::Settings::default()))"
            ),
            "{main}"
        );
        assert!(!main.contains("plugin_from_config"), "{main}");
    }

    #[test]
    fn the_generated_config_names_only_sections_a_plugin_claims() {
        // `validate_sections` rejects a section nothing claims, so a scaffold
        // that wrote `[sessions]` without the sessions plugin would fail on its
        // own first run. The real crate is asked, not a reimplementation.
        for picks in [
            vec![Pick::Config],
            vec![Pick::Config, Pick::Logging],
            Pick::ALL.to_vec(),
        ] {
            let a = answers(&picks);
            let toml = config_toml(&a);
            let config = brocanth_config::Config::from_toml(a.name.as_str(), "development", &toml)
                .unwrap_or_else(|e| panic!("{picks:?}: {e}\n{toml}"));
            // The instances the generated `main` registers: one per plugin,
            // plus `config` itself, which `with_config` adds.
            let mut claimed: Vec<String> =
                a.plugins().iter().map(|p| p.key().to_string()).collect();
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
        let mut answers = answers(&[Pick::Logging, Pick::Commands]);
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
        let only_logging = scaffold(&answers(&[Pick::Logging]));
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
        let everything = scaffold(&answers(&Pick::ALL));
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
    fn every_page_the_table_names_is_a_file_the_scaffold_writes() {
        // The generated `router()` names three files. If one were not written
        // the application would not compile, which is a worse way to find out
        // than this test.
        let files = scaffold(&answers(&Pick::ALL));
        for page in ["index.rs", "about.rs", "users/[id].rs"] {
            assert!(PAGES_MOD_RS.contains(&format!("\"{page}\"")), "{page}");
            assert!(
                files.iter().any(|(p, _)| p == &format!("src/pages/{page}")),
                "the table names {page} but nothing writes it"
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
            picks: vec![Pick::Logging, Pick::Config, Pick::Sessions],
            local: Some(brocanth.display().to_string()),
            git: false,
        };
        for (path, contents) in scaffold(&a) {
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
