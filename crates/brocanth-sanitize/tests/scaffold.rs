//! The `sanitize` module, written into a project by the CLI's generator.
//!
//! Bless with `BROCANTH_BLESS=1`; build the project too with
//! `BROCANTH_BUILD=1`. See `crates/brocanth-robots/tests/scaffold.rs` for
//! the pattern.

use brocanth_sanitize::scaffold::Sanitize;
use brocanth_scaffold::testing;

/// With rsx, the crate arrives with its `rsx` feature, so a component can
/// take a `SafeHtml` as a child.
#[test]
fn an_rsx_project_gets_the_sanitizer_with_its_template_impl() {
    let picks = &["rsx", "sanitize"];
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Sanitize])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/sanitize.txt"),
        &testing::snapshot(&files),
    );
    let manifest = files
        .iter()
        .find(|(path, _)| path == "Cargo.toml")
        .map(|(_, text)| text.as_str())
        .expect("a Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.starts_with("brocanth-sanitize"))
        .expect("the dependency");
    assert!(line.contains("\"rsx\""), "{line}");
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Sanitize])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// Without rsx there is no template engine to implement, so no feature.
#[test]
fn a_project_without_rsx_gets_the_sanitizer_alone() {
    let files = brocanth_cli::testing::generate("my-api", &["sanitize"], &[&Sanitize])
        .unwrap_or_else(|e| panic!("{e}"));
    let manifest = files
        .iter()
        .find(|(path, _)| path == "Cargo.toml")
        .map(|(_, text)| text.as_str())
        .expect("a Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.starts_with("brocanth-sanitize"))
        .expect("the dependency");
    assert!(!line.contains("rsx"), "{line}");
}
