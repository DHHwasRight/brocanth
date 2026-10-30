//! `cache:clear` -- a name no Rust identifier can spell, which is what
//! `name = "..."` is for.

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Empty every cache this project writes.
#[cmd(name = "cache:clear")]
#[derive(clap::Args)]
pub struct CacheClear {
    /// Clear without asking first.
    #[arg(long)]
    pub force: bool,
}

impl Command for CacheClear {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        if !self.force {
            println!("this would clear every cache; pass --force to mean it");
            return Ok(());
        }
        println!("cleared every cache");
        Ok(())
    }
}
