//! `brocanth` -- the command line over the framework.
//!
//! Six commands, one output layer. The layer is the interesting part and lives
//! in [`ui`]: whether a person is watching is decided once, in
//! [`ui::Face::decide`], and every byte any command writes goes through a
//! [`ui::Ui`] carrying that answer. No command calls `println!`, checks
//! `is_terminal`, or reads `NO_COLOR`.

mod commands;
mod project;
mod tui;
mod ui;

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

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

#[derive(Parser)]
#[command(
    name = "brocanth",
    version,
    about = "A server framework for HTML-first applications. Nothing ships unless it is asked for.",
    after_help = "Inside a project, any other subcommand is a command: it is built and run by \
the project's own binary. `brocanth commands` lists the ones this project defines.",
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
    /// Print the resolved route table
    Routes(commands::routes::RoutesArgs),
    /// Compile the application
    Build(commands::build::BuildArgs),
    /// Check the route table, the mounts and the plugins, then compile
    Check(commands::check::CheckArgs),
    /// Generate a page, a plugin or a model
    Generate(commands::generate::GenerateArgs),
    /// Serve the application and rebuild as it changes
    Dev(commands::dev::DevArgs),
    /// A command defined in this project, run by the project's own binary
    ///
    /// Anything this tool does not recognise is a command. `brocanth` is
    /// installed once; the commands it can run live in a project and are
    /// compiled into that project's binary, so they cannot be listed here.
    /// Inside a project, `brocanth commands` lists them.
    #[command(external_subcommand)]
    Task(Vec<OsString>),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    // Asked once. Everything downstream reads the answer off the `Ui`.
    let face = Face::decide(cli.global.no_color, cli.global.quiet);
    let ui = Ui::new(face, cli.global.quiet);
    let dir = cli.global.dir.as_deref();

    let result = match cli.command {
        Command::Create(args) => commands::create::run(&ui, dir, args),
        Command::Routes(args) => commands::routes::run(&ui, dir, args),
        Command::Build(args) => commands::build::run(&ui, dir, args),
        Command::Check(args) => commands::check::run(&ui, dir, args),
        Command::Generate(args) => commands::generate::run(&ui, dir, args),
        Command::Dev(args) => commands::dev::run(&ui, dir, args),
        Command::Task(args) => commands::delegate::run(&ui, dir, args),
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
