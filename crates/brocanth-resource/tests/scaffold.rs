//! The `resource` module, written into a project by the CLI's generator.
//!
//! The shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! the project too with `BROCANTH_BUILD=1`.

use brocanth_resource::scaffold::Records;
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Records];

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

/// The module adapts to nothing: the page triple's impls, and an empty
/// list of resources mounted at `/api`.
#[test]
fn an_empty_list_of_resources_under_api() {
    shape("resource", &["resource"]);
}
