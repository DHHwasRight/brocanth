//! Rails-style commands for a brocanth project.
//!
//! A command is a `clap::Args` struct with a `run` method. Write one anywhere in
//! your project, put `#[brocanth_command::cmd]` on it, and it appears in your
//! binary's `--help` with its arguments parsed. There is no list to add it to
//! and nothing is loaded at run time: registration happens at link time,
//! through a [`linkme`] distributed slice.
//!
//! # Defining a command
//!
//! ```
//! use brocanth_command::{anyhow, clap, cmd, Context, Command};
//!
//! /// Seed the demo database.
//! #[cmd]
//! #[derive(clap::Args)]
//! pub struct SeedDemo {
//!     /// How many rows to write.
//!     #[arg(long, default_value_t = 100)]
//!     pub count: u32,
//! }
//!
//! impl Command for SeedDemo {
//!     fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
//!         println!("seeding {} rows", self.count);
//!         Ok(())
//!     }
//! }
//! ```
//!
//! Your `main` is the whole CLI:
//!
//! ```no_run
//! fn main() -> brocanth_command::anyhow::Result<()> {
//!     brocanth_command::main(brocanth_command::Context::boot()?)
//! }
//! ```
//!
//! # Two traits, because an impl block has one author
//!
//! [`Task::run`] is yours. [`Registered::NAME`] is the macro's, derived from
//! the struct name in `snake_case`. They are separate traits because a single
//! one could not hold both halves: you would have to write the name the macro
//! exists to work out for you.
//!
//! You never write `impl Registered` by hand. [`entry`] is bounded on it, so
//! forgetting `#[cmd]` is a type error at the registration site rather than a
//! command that silently never appears.
//!
//! # The linking gotcha
//!
//! **A crate that is never referenced is dropped by the linker, and its commands
//! go with it.** The slice is populated by the linker from the object files it
//! actually keeps. Your own crate is fine -- `main` is in it. A *third-party*
//! crate whose only purpose is to contribute commands is not: nothing in your
//! code mentions it, so nothing keeps it.
//!
//! Reference it explicitly:
//!
//! ```ignore
//! // In main.rs. The `as _` imports nothing; it only tells the linker the
//! // crate is wanted, which keeps the commands it registers.
//! use some_task_pack as _;
//! ```
//!
//! Without that line the crate compiles, the tests pass, and the commands are
//! simply absent from `--help`. It is the one failure here that is silent.

// The macro emits absolute `::brocanth_command::...` paths, which is what makes
// it work in other people's crates. For the built-in below to use the same
// macro rather than a privileged copy, the crate needs that name for itself.
extern crate self as brocanth_command;

use std::collections::BTreeMap;

use clap::{ArgMatches, Command as ClapCommand};

pub use anyhow;
pub use clap;
pub use linkme;

/// Registers a `clap::Args` struct as a command. See the [crate docs](crate).
pub use brocanth_command_macros::cmd;

// ---------------------------------------------------------------------------
// Context
// ---------------------------------------------------------------------------

/// What every command is handed.
///
/// Deliberately thin for now: the environment name and the project root. A
/// command that needs a database handle will get it from a field added here, not
/// by reaching for a global.
#[derive(Debug, Clone)]
pub struct Context {
    env: String,
    root: std::path::PathBuf,
}

impl Context {
    /// Reads the environment and finds the project root.
    ///
    /// `$BROCANTH_ENV` names the environment and defaults to `development`.
    /// The root is the nearest ancestor holding `brocanth.toml`, falling back
    /// to the current directory so a command still runs outside a project.
    pub fn boot() -> anyhow::Result<Self> {
        let env = std::env::var("BROCANTH_ENV").unwrap_or_else(|_| "development".to_string());
        let here = std::env::current_dir()?;
        let root = here
            .ancestors()
            .find(|dir| dir.join("brocanth.toml").is_file())
            .unwrap_or(&here)
            .to_path_buf();
        Ok(Self { env, root })
    }

    /// The environment name: `development` unless `$BROCANTH_ENV` says else.
    pub fn env(&self) -> &str {
        &self.env
    }

    /// The directory holding `brocanth.toml`.
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}

// ---------------------------------------------------------------------------
// The traits
// ---------------------------------------------------------------------------

/// What a command does. Implemented by hand.
pub trait Command: clap::Args + 'static {
    /// Runs the command. `self` holds the parsed arguments.
    fn run(self, ctx: &mut Context) -> anyhow::Result<()>;
}

/// What a command is called. Implemented by `#[cmd]`, never by hand.
///
/// Separate from [`Task`] because the two halves have different authors and
/// an `impl` block has only one.
pub trait Registered: Command {
    /// The subcommand name.
    const NAME: &'static str;
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// One registered command: its name, how to describe it to clap, how to run it.
///
/// The two function pointers are generic `fn` items rather than closures, so
/// the whole value is a constant and can live in a distributed slice.
#[derive(Debug)]
pub struct CommandEntry {
    /// The subcommand name.
    pub name: &'static str,
    /// Adds this command's arguments to a `Command` already named for it.
    pub augment: fn(ClapCommand) -> ClapCommand,
    /// Parses the matches back into the struct and runs it.
    pub run: fn(&ArgMatches, &mut Context) -> anyhow::Result<()>,
}

/// Every command linked into this binary.
///
/// Populated at link time. Order is whatever the linker chose, so anything
/// that cares sorts first.
#[linkme::distributed_slice]
pub static COMMANDS: [CommandEntry];

fn augment_of<T: Registered>(command: ClapCommand) -> ClapCommand {
    T::augment_args(command)
}

fn run_of<T: Registered>(matches: &ArgMatches, ctx: &mut Context) -> anyhow::Result<()> {
    T::from_arg_matches(matches)?.run(ctx)
}

/// The slice element for a command. Called by `#[cmd]`.
pub const fn entry<T: Registered>() -> CommandEntry {
    CommandEntry {
        name: T::NAME,
        augment: augment_of::<T>,
        run: run_of::<T>,
    }
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

/// Builds the CLI from [`COMMANDS`] and runs whichever command was asked for.
///
/// Two names colliding is reported before any `Command` is built, because
/// clap's own behaviour for a duplicate subcommand is to keep one silently --
/// and a command that vanished is far worse to debug than one that refused to
/// start.
pub fn main(ctx: Context) -> anyhow::Result<()> {
    match run_with(&COMMANDS, ctx, std::env::args_os()) {
        // clap reports `--help` and `--version` as errors so the caller can
        // decide what they mean. The decision here is clap's own: print to
        // the right stream and exit with the right code. Treating them as
        // failures would make `--help` exit non-zero and print the help once
        // as help and again as an error message.
        Err(error) => match error.downcast::<clap::Error>() {
            Ok(from_clap) => from_clap.exit(),
            Err(other) => Err(other),
        },
        ok => ok,
    }
}

/// [`main`], with the slice and the arguments passed in, so it is testable.
#[doc(hidden)]
pub fn run_with<I, T>(
    commands: &'static [CommandEntry],
    mut ctx: Context,
    args: I,
) -> anyhow::Result<()>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString> + Clone,
{
    let sorted = sorted_unique(commands)?;

    let mut command = ClapCommand::new("manage")
        .about("Project commands. Each one is a struct in this project with `#[cmd]` on it.")
        .subcommand_required(true)
        .arg_required_else_help(true);
    for entry in &sorted {
        command = command.subcommand((entry.augment)(ClapCommand::new(entry.name)));
    }

    let matches = command.try_get_matches_from(args)?;
    let (name, sub) = matches
        .subcommand()
        .expect("subcommand_required means clap already refused an empty invocation");
    let entry = sorted
        .iter()
        .find(|entry| entry.name == name)
        .expect("clap matched a subcommand this loop just registered");
    (entry.run)(sub, &mut ctx)
}

/// The entries by name, or an error naming every collision.
///
/// A `BTreeMap` does the sorting and the detection in one pass, and reports
/// *all* duplicates rather than only the first: two separate failing builds to
/// fix two collisions is one build too many.
fn sorted_unique(commands: &'static [CommandEntry]) -> anyhow::Result<Vec<&'static CommandEntry>> {
    let mut by_name: BTreeMap<&'static str, Vec<&'static CommandEntry>> = BTreeMap::new();
    for entry in commands {
        by_name.entry(entry.name).or_default().push(entry);
    }

    let clashes: Vec<&str> = by_name
        .iter()
        .filter(|(_, entries)| entries.len() > 1)
        .map(|(name, _)| *name)
        .collect();
    if !clashes.is_empty() {
        anyhow::bail!(
            "two or more commands share a name: {}.\n\
             Every command name must be unique across this binary and every crate \
             it links. Rename one with `#[cmd(name = \"...\")]`.",
            clashes.join(", ")
        );
    }

    Ok(by_name.into_values().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context {
            env: "test".to_string(),
            root: std::path::PathBuf::from("."),
        }
    }

    /// Two entries under one name, built by hand rather than by the macro --
    /// the macro cannot produce this, which is exactly why the runner has to
    /// catch it: the second crate is somebody else's.
    static DUPLICATES: [CommandEntry; 3] = [
        CommandEntry {
            name: "migrate",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
        CommandEntry {
            name: "seed",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
        CommandEntry {
            name: "migrate",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
    ];

    #[test]
    fn two_tasks_with_one_name_is_an_error_that_names_them() {
        let error = sorted_unique(&DUPLICATES).expect_err("a duplicate must not be tolerated");
        let message = error.to_string();
        assert!(message.contains("migrate"), "{message}");
        assert!(
            !message.contains("seed"),
            "the command that is fine should not be named: {message}"
        );
        assert!(
            message.contains("name = "),
            "it should say how to fix it: {message}"
        );
    }

    static TWO_CLASHES: [CommandEntry; 4] = [
        CommandEntry {
            name: "b",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
        CommandEntry {
            name: "a",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
        CommandEntry {
            name: "b",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
        CommandEntry {
            name: "a",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
    ];

    #[test]
    fn every_collision_is_reported_not_only_the_first() {
        let message = sorted_unique(&TWO_CLASHES)
            .expect_err("two collisions")
            .to_string();
        assert!(message.contains("a, b"), "both, in order: {message}");
    }

    static ORDERED: [CommandEntry; 3] = [
        CommandEntry {
            name: "zebra",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
        CommandEntry {
            name: "apple",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
        CommandEntry {
            name: "mango",
            augment: |c| c,
            run: |_, _| Ok(()),
        },
    ];

    #[test]
    fn the_help_order_is_alphabetical_not_the_linkers_order() {
        // The linker's order is unspecified and varies by platform, so a CLI
        // that printed it would have a `--help` that differed between a Mac
        // and the CI box.
        let names: Vec<&str> = sorted_unique(&ORDERED)
            .expect("no duplicates")
            .iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["apple", "mango", "zebra"]);
    }

    /// A command that exists only here, so the registration claim has
    /// something of its own to be about. This crate ships no commands: a
    /// project's command line is the project's.
    #[cmd]
    #[derive(clap::Args)]
    struct ProvesRegistration {
        #[arg(long)]
        flag: bool,
    }

    impl Command for ProvesRegistration {
        fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_attribute_really_writes_into_the_slice() {
        // If `#[cmd]` stopped emitting the slice element, this is the test
        // that fails first -- and nothing else would, because a command that
        // never registers is simply absent rather than wrong.
        assert!(
            COMMANDS.iter().any(|e| e.name == "proves_registration"),
            "a `#[cmd]` struct is missing from the registry"
        );
    }

    #[test]
    fn the_derived_name_is_the_struct_name_in_snake_case() {
        assert_eq!(
            <ProvesRegistration as Registered>::NAME,
            "proves_registration"
        );
    }

    #[test]
    fn a_command_runs_through_the_runner() {
        static ONE: [CommandEntry; 1] = [CommandEntry {
            name: "proves_registration",
            augment: augment_of::<ProvesRegistration>,
            run: run_of::<ProvesRegistration>,
        }];
        run_with(&ONE, ctx(), ["app", "proves_registration", "--flag"]).expect("it ran");
    }

    #[test]
    fn an_unknown_subcommand_is_an_error_rather_than_a_silent_success() {
        static ONE: [CommandEntry; 1] = [CommandEntry {
            name: "proves_registration",
            augment: augment_of::<ProvesRegistration>,
            run: run_of::<ProvesRegistration>,
        }];
        assert!(run_with(&ONE, ctx(), ["app", "nope"]).is_err());
    }

    #[test]
    fn boot_defaults_the_environment_rather_than_failing_without_one() {
        let ctx = Context::boot().expect("boot works outside a project");
        assert!(!ctx.env().is_empty());
    }
}
