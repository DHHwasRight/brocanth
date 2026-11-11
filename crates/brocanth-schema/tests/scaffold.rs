//! The `validation` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_schema::scaffold::Validation;
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Validation];

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

/// With a config file: the `[validation]` section.
#[test]
fn with_a_config_file() {
    shape("validation-config", &["config", "validation"]);
}

/// With no config file: the settings come from flags and the environment.
#[test]
fn without_a_config_file() {
    shape("validation-flags", &["validation"]);
}
