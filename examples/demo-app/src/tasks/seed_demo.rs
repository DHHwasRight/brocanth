//! `seed_demo` -- the name derived from the struct, in snake_case.

use brocanth_task::{anyhow, clap, task, Context, Task};

/// Fill the demo database with sample rows.
#[task]
#[derive(clap::Args)]
pub struct SeedDemo {
    /// How many rows to write.
    #[arg(long, default_value_t = 100)]
    pub count: u32,
}

impl Task for SeedDemo {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        println!(
            "would seed {} rows into the {} database",
            self.count,
            ctx.env()
        );
        Ok(())
    }
}
