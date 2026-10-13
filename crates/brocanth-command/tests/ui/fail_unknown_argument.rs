//! `#[cmd]` takes `name` and nothing else.
use brocanth_command::{clap, cmd};

#[cmd(nmae = "typo")]
#[derive(clap::Args)]
struct Oops {
    #[arg(long)]
    flag: bool,
}

fn main() {}
