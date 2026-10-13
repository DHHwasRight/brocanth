//! A brocanth project. The binary is the CLI.
//!
//! `brocanth_command::main` builds the command line out of every command linked
//! into this binary, so adding a file under `src/commands/` adds a subcommand.
//! Nothing here lists them.

mod commands;

fn main() -> brocanth_command::anyhow::Result<()> {
    brocanth_command::main(brocanth_command::Context::boot()?)
}
