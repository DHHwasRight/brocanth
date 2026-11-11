//! The `jobs` modules, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_jobs::scaffold::{Jobs, JobsMemory};
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Jobs, &JobsMemory];

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, MODULES)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, MODULES)
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// The queue with no config file: its settings come from flags and the
/// environment.
#[test]
fn the_queue_without_a_config_file() {
    shape("jobs-flags", &["jobs"]);
}

/// The queue on the `db` pool, with a command line and no admin: no `url`
/// in `[jobs]`, and no jobs tab.
#[test]
fn the_queue_on_the_pool_with_a_command_line() {
    shape("jobs-db-commands", &["config", "db", "jobs", "commands"]);
}
