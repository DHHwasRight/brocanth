//! The `errors` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked with rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project too with `BROCANTH_BUILD=1`.

use brocanth_errors::scaffold::ErrorPages;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&ErrorPages])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&ErrorPages])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With templates: `not-found.rsx` and `error.rsx` inside the layouts, and
/// the setting in the config file.
#[test]
fn a_project_with_templates_answers_its_own_error_pages() {
    shape("errors", &["config", "rsx", "errors"]);
}

/// Without templates or a config file: the plain page, and the setting as
/// a flag.
#[test]
fn a_project_without_templates_answers_the_plain_page() {
    shape("errors-plain", &["errors"]);
}
