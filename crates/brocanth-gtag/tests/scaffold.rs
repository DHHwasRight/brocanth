//! The `gtag` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_gtag::scaffold::Gtag;
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Gtag];

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

/// The banner in the request's locale, with the security headers and a
/// config file: the `[gtag]` section, the `i18n` feature, and the policy
/// that allows what the tag loads.
#[test]
fn the_banner_follows_the_locale_with_a_config_file() {
    shape(
        "gtag-i18n-config",
        &[
            "config",
            "middleware",
            "middleware-security-headers",
            "i18n",
            "gtag",
        ],
    );
}

/// The module alone: English words, and the settings from flags.
#[test]
fn the_module_alone() {
    shape("gtag-flags", &["gtag"]);
}
