//! The `humanize` module, written into a project by the CLI's generator.
//!
//! Bless with `BROCANTH_BLESS=1`; build the project too with
//! `BROCANTH_BUILD=1`. See `crates/brocanth-robots/tests/scaffold.rs` for
//! the pattern.

use brocanth_humanize::scaffold::Humanize;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) -> Vec<(String, String)> {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Humanize])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let built = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Humanize])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&built).unwrap_or_else(|e| panic!("{e}"));
    }
    files
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| panic!("{path} is generated"))
}

/// Without `i18n`, one language from the module file, and no locale data.
#[test]
fn without_i18n_the_language_is_the_module_option() {
    let files = shape("humanize", &["config", "humanize"]);
    let helper = file(&files, "src/plugins/humanize/mod.rs");
    assert!(
        helper.contains("crate::modules::humanize::MODULE.locale"),
        "{helper}"
    );
    let manifest = file(&files, "Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.starts_with("brocanth-humanize"))
        .expect("the dependency");
    assert!(!line.contains("i18n"), "{line}");
}

/// With `i18n`, the request's locale, and the crate's `i18n` feature.
#[test]
fn with_i18n_the_language_is_the_request_locale() {
    let files = shape("humanize-i18n", &["config", "i18n", "humanize"]);
    let helper = file(&files, "src/plugins/humanize/mod.rs");
    assert!(
        helper.contains("crate::plugins::i18n::locale(ctx).formats()"),
        "{helper}"
    );
    let manifest = file(&files, "Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.starts_with("brocanth-humanize"))
        .expect("the dependency");
    assert!(line.contains("\"i18n\""), "{line}");
}
