//! The `filters` module, written into a project by the CLI's generator,
//! checked with rustfmt and compared with a golden file. Bless with
//! `BROCANTH_BLESS=1`; build each project too with `BROCANTH_BUILD=1`.

use brocanth_filters::scaffold::Filters;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Filters])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Filters])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// Filters beside the policy and the paginator: `ctx.filters` and the rules.
#[test]
fn a_project_reads_its_filters_through_a_declaration() {
    shape("filters", &["rsx", "policy", "pagination", "filters"]);
}

/// Without `policy` the module is refused, with the reason.
#[test]
fn filters_without_a_policy_are_refused() {
    let wrong = brocanth_cli::testing::generate("my-blog", &["filters"], &[&Filters])
        .expect_err("filters need policy");
    assert!(wrong.contains("policy"), "{wrong}");
}
