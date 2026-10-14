//! `brocanth` -- the command line over the framework.
//!
//! Six commands, one output layer. The layer is the interesting part and lives
//! in [`ui`]: whether a person is watching is decided once, in
//! [`ui::Face::decide`], and every byte any command writes goes through a
//! [`ui::Ui`] carrying that answer. No command calls `println!`, checks
//! `is_terminal`, or reads `NO_COLOR`.

mod commands;
mod tui;
mod ui;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};

use ui::{Face, Ui};

/// clap's own help, in the same five styles as everything else. A tool whose
/// `--help` looks like a different program is a tool with two designs.
fn help_styles() -> clap::builder::Styles {
    use anstyle::Style;
    clap::builder::Styles::styled()
        .header(ui::STRONG)
        .usage(ui::STRONG)
        .literal(ui::ACCENT)
        .placeholder(ui::DIM)
        .valid(ui::OK)
        .invalid(ui::WARN)
        .error(ui::BAD.bold())
        .context(Style::new())
}

/// The tail of `--help` outside a project, or inside one whose binary has not
/// been built yet.
const AFTER_HELP: &str =
    "Inside a project, any other subcommand is one this project defines: it is \
built and run by the project's own binary. Build it and this help lists them.";

#[derive(Parser)]
#[command(
    name = "brocanth",
    version,
    about = "A server framework for HTML-first applications. Nothing ships unless it is asked for.",
    after_help = AFTER_HELP,
    styles = help_styles(),
    max_term_width = 100,
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    #[command(flatten)]
    global: Global,
}

/// The flags that feed the one decision, plus where the application is.
#[derive(Args)]
struct Global {
    /// Package root [default: the nearest directory holding a Cargo.toml and src/pages]
    #[arg(long, short = 'C', global = true, value_name = "DIR")]
    dir: Option<PathBuf>,
    /// Never colour the output, even on a terminal
    #[arg(long, global = true)]
    no_color: bool,
    /// Print only the result and anything wrong with it
    #[arg(long, short, global = true)]
    quiet: bool,
    /// After the chooser, print the command line that would have given the
    /// same answers with nothing to ask
    #[arg(long, short = 'D', global = true)]
    debug: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Scaffold an application
    ///
    /// With no NAME and a terminal on both descriptors this opens a chooser.
    /// With a NAME it runs straight through, because every other answer has a
    /// default this command will tell you about.
    #[command(alias = "new")]
    Create(commands::create::CreateArgs),
    /// A command defined in this project, run by the project's own binary
    ///
    /// Everything this tool does not recognise is one. `brocanth` is
    /// installed once and scaffolds; a project's commands live in that
    /// project and are compiled into its binary, so they cannot be listed
    /// here -- but a built project has them in this help, above.
    ///
    /// The payload is never read -- dispatch works off the raw matches, so a
    /// listed command and an unlisted one take one path. The variant has to
    /// exist anyway: it is what makes clap accept an unknown subcommand
    /// rather than refusing it.
    #[command(external_subcommand)]
    Delegated(#[allow(dead_code)] Vec<OsString>),
}

/// `brocanth`'s own command, with the project's commands folded into the help.
///
/// Each one is a real subcommand, so it is listed under `Commands:` and
/// completes -- but it takes every remaining argument verbatim and answers no
/// `--help` of its own. The project's binary is the authority on its own
/// flags, and a `--help` synthesised here would be a worse copy of one that
/// already exists.
fn with_project_commands() -> clap::Command {
    let command = Cli::command();
    let Some(listed) = commands::delegate::listed(dir_from_argv().as_deref()) else {
        return command;
    };
    command
        .after_help(
            "Any other subcommand is also this project's: it is built and run by the \
project's own binary, whether or not it is listed above.",
        )
        .subcommands(listed.into_iter().map(|(name, about)| {
            clap::Command::new(name)
                .about(about)
                .disable_help_flag(true)
                .arg(
                    clap::Arg::new(DELEGATED)
                        .num_args(..)
                        .allow_hyphen_values(true)
                        .trailing_var_arg(true)
                        .value_parser(clap::value_parser!(OsString)),
                )
        }))
}

/// The argument every synthesised subcommand collects its own tail into.
const DELEGATED: &str = "delegated";

/// `-C`/`--dir` straight out of argv.
///
/// A pre-scan rather than a parse: clap cannot tell us where to look until it
/// has parsed, and it cannot finish parsing until it knows what subcommands
/// exist, which is what the directory decides.
fn dir_from_argv() -> Option<PathBuf> {
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let text = arg.to_string_lossy().into_owned();
        if text == "-C" || text == "--dir" {
            return args.next().map(PathBuf::from);
        }
        if let Some(rest) = text.strip_prefix("--dir=") {
            return Some(PathBuf::from(rest));
        }
        if let Some(rest) = text.strip_prefix("-C").filter(|r| !r.is_empty()) {
            return Some(PathBuf::from(rest));
        }
    }
    None
}

fn main() -> ExitCode {
    // `-C` has to be read before clap parses, because what this tool can do
    // depends on which directory it is pointed at: inside a project, the
    // project's own commands belong in `--help` beside `create`. A help that
    // omits what you can run is a help that lies by omission.
    let matches = with_project_commands().get_matches();
    let global = Global::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    // Asked once. Everything downstream reads the answer off the `Ui`.
    let face = Face::decide(global.no_color, global.quiet);
    let ui = Ui::new(face, global.quiet, global.debug);
    let dir = global.dir.as_deref();

    let result = match matches.subcommand() {
        Some(("create", m)) => commands::create::run(
            &ui,
            dir,
            commands::create::CreateArgs::from_arg_matches(m).unwrap_or_else(|e| e.exit()),
        ),
        // Everything else belongs to the project: the ones listed in the help
        // and the ones added since it was last built take the same path.
        Some((name, m)) => {
            let mut args: Vec<OsString> = vec![name.into()];
            // Two ids, because there are two ways to arrive here and only one
            // of them defines an argument. A *listed* command is synthesised
            // above and collects its tail into `DELEGATED`; an unlisted one
            // reaches clap's `external_subcommand`, which uses the empty id.
            // `try_get_many` rather than `get_many`: the latter panics on an
            // id the matched subcommand never declared, which is exactly the
            // case here.
            for id in [DELEGATED, ""] {
                if let Ok(Some(values)) = m.try_get_many::<OsString>(id) {
                    args.extend(values.cloned());
                }
            }
            commands::delegate::run(&ui, dir, args)
        }
        None => {
            let _ = with_project_commands().print_help();
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(code) => code,
        Err(diagnostics) => {
            for d in &diagnostics {
                ui.diagnostic(d);
            }
            ExitCode::FAILURE
        }
    }
}
