//! The `markdown` module, written into a project by the CLI's generator.
//!
//! Bless with `BROCANTH_BLESS=1`; build the project too with
//! `BROCANTH_BUILD=1`. See `crates/brocanth-robots/tests/scaffold.rs` for
//! the pattern.

use brocanth_markdown::scaffold::MarkdownModule;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) -> Vec<(String, String)> {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&MarkdownModule])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let built = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&MarkdownModule])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&built).unwrap_or_else(|e| panic!("{e}"));
    }
    files
}

fn dependency(files: &[(String, String)]) -> String {
    files
        .iter()
        .find(|(path, _)| path == "Cargo.toml")
        .and_then(|(_, text)| text.lines().find(|l| l.starts_with("brocanth-markdown")))
        .expect("the dependency")
        .to_string()
}

/// With rsx, the crate arrives with its `rsx` feature, so a component
/// writes the rendered HTML with `{! body }`.
#[test]
fn an_rsx_project_gets_markdown_with_its_template_impl() {
    let files = shape("markdown", &["rsx", "markdown"]);
    assert!(dependency(&files).contains("\"rsx\""), "{files:?}");
}

/// Without rsx there is no template engine to implement, so no feature.
#[test]
fn a_project_without_templates_gets_the_plain_crate() {
    let files = shape("markdown-plain", &["markdown"]);
    assert!(!dependency(&files).contains("rsx"));
}
