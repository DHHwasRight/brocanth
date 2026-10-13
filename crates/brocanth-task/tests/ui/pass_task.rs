//! The shape a task is meant to have, both spellings of the name.
use brocanth_task::{anyhow, clap, task, Context, Registered, Task};

/// Seed the demo database.
#[task]
#[derive(clap::Args)]
struct SeedDemo {
    #[arg(long, default_value_t = 100)]
    count: u32,
}

impl Task for SeedDemo {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Clear the cache.
#[task(name = "cache:clear")]
#[derive(clap::Args)]
struct CacheClear {
    #[arg(long)]
    force: bool,
}

impl Task for CacheClear {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        Ok(())
    }
}

fn main() {
    // The macro derived one name and took the other verbatim.
    assert_eq!(<SeedDemo as Registered>::NAME, "seed_demo");
    assert_eq!(<CacheClear as Registered>::NAME, "cache:clear");
}
