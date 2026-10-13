//! `cache:clear` -- a name no Rust identifier can spell, which is what
//! `name = "..."` is for.

use brocanth_task::{anyhow, clap, task, Context, Task};

/// Empty every cache this project writes.
#[task(name = "cache:clear")]
#[derive(clap::Args)]
pub struct CacheClear {
    /// Clear without asking first.
    #[arg(long)]
    pub force: bool,
}

impl Task for CacheClear {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        if !self.force {
            println!("this would clear every cache; pass --force to mean it");
            return Ok(());
        }
        println!("cleared every cache");
        Ok(())
    }
}
