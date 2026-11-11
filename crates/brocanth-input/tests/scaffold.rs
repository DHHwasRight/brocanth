//! The `input` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_input::scaffold::Entry;
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Entry];

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

/// With rsx: a refused form answers the form again, as HTML.
#[test]
fn a_refused_form_answers_html_with_rsx() {
    shape("input-rsx", &["rsx", "input"]);
}

/// With no renderer: a refused body answers its faults as JSON.
#[test]
fn a_refused_body_answers_json_without_a_renderer() {
    shape("input", &["input"]);
}
