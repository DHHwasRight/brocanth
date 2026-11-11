//! The `policy` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_policy::scaffold::Authorisation;
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Authorisation];

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

/// With a config file, the model and the broker: the `[policy]` section,
/// the dialect note that names the model's database, and where a rule
/// joins a broadcast key.
#[test]
fn with_the_model_and_the_broker() {
    shape(
        "policy-model-realtime",
        &["config", "db", "model", "realtime", "policy"],
    );
}

/// The module alone: the settings from flags, and no dialect note.
#[test]
fn the_module_alone() {
    shape("policy-flags", &["policy"]);
}
