//! The `analytics` and `beacon` modules, written into a project by the
//! CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.
//!
//! The shapes with a config file stay in the CLI (`analytics`, with the
//! admin, and `beacon`, on a static site). A copy here would be the same
//! text twice, so these shapes are the branches the CLI does not write.

use brocanth_analytics::scaffold::{PageViews, StaticBeacon};
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&PageViews, &StaticBeacon];

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

/// Page views with no admin and no config file: no analytics tab, and the
/// settings come from flags and the environment.
#[test]
fn page_views_without_a_config_file() {
    shape("analytics-flags", &["db", "analytics"]);
}

/// The beacon on a server with no config file: the collector is a flag.
#[test]
fn the_beacon_on_a_server_without_a_config_file() {
    shape("beacon-flags", &["beacon"]);
}
