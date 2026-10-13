//! A brocanth project. The binary is the CLI.
//!
//! `brocanth_task::main` builds the command line out of every task linked
//! into this binary, so adding a file under `src/tasks/` adds a subcommand.
//! Nothing here lists them.

mod tasks;

fn main() -> brocanth_task::anyhow::Result<()> {
    brocanth_task::main(brocanth_task::Context::boot()?)
}
