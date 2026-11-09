//! The `pagination` module, written into a project by the CLI's generator,
//! checked with rustfmt and compared with a golden file. Bless with
//! `BROCANTH_BLESS=1`; build each project too with `BROCANTH_BUILD=1`.

use brocanth_pagination::scaffold::Pagination;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Pagination])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Pagination])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With rsx: the two pager components, and the context helpers.
#[test]
fn an_rsx_project_gets_the_pager_components() {
    shape("pagination-rsx", &["rsx", "policy", "pagination"]);
}

/// With jt: the page links as JSON, and the HTML pager as a function.
#[test]
fn a_jt_project_gets_the_links_as_json() {
    shape("pagination-jt", &["jt", "pagination"]);
}
