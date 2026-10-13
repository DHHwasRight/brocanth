//! `#[task]` takes `name` and nothing else.
use brocanth_task::{clap, task};

#[task(nmae = "typo")]
#[derive(clap::Args)]
struct Oops {
    #[arg(long)]
    flag: bool,
}

fn main() {}
