//! The `i18n` modules, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_i18n::scaffold::{I18n, I18nRouting};
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&I18n, &I18nRouting];

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

/// One URL per locale beside the admin and the file routes, with a config
/// file and rsx: the `[i18n]` section, `.with_paths(true)`, and both routes
/// kept unprefixed, read from their module files.
#[test]
fn locale_paths_keep_other_modules_routes_unprefixed() {
    shape(
        "i18n-routing-modules",
        &[
            "config",
            "rsx",
            "admin",
            "storage",
            "storage-routes",
            "i18n",
            "i18n-routing",
        ],
    );
}

/// With jt, the consent banner and no config file: the banner's keys in
/// the catalogs, the jt section of AGENTS.md, and the settings from flags.
#[test]
fn jt_and_the_consent_banner_without_a_config_file() {
    shape("i18n-jt-gtag-flags", &["jt", "gtag", "i18n"]);
}
