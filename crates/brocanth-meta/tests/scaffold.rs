//! The `meta` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked with rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project too with `BROCANTH_BUILD=1`.

use brocanth_i18n::scaffold::I18nRouting;
use brocanth_meta::scaffold::PageMeta;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&PageMeta, &I18nRouting])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files =
            brocanth_cli::testing::generate_for_build("my-blog", picks, &[&PageMeta, &I18nRouting])
                .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With templates and a config file: the root layout writes the head, and
/// `[meta]` is in the file.
#[test]
fn the_root_layout_writes_each_pages_head() {
    shape("meta", &["config", "rsx", "meta"]);
}

/// With one URL per locale: `lang`, the canonical URL with its segment,
/// and an `hreflang` alternate for each locale.
#[test]
fn a_page_lists_its_other_locales() {
    shape(
        "meta-i18n-routing",
        &["rsx", "i18n", "i18n-routing", "meta"],
    );
}
