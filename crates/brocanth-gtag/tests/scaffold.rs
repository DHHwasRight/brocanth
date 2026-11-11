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

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| panic!("{path} is generated"))
}

/// The admin never gets the tag, wherever src/modules/admin.rs mounts it.
/// The registration reads `mount_at` from the module file, with a config
/// file and without one, and the `exclude` setting does not copy it
/// (docs/module-config.md §9). With `BROCANTH_BUILD=1`, each project with
/// the admin moved builds and passes its tests with no other edit.
#[test]
fn a_moved_admin_never_gets_the_tag_with_no_other_edit() {
    for picks in [&["config", "admin", "gtag"][..], &["admin", "gtag"][..]] {
        let files = brocanth_cli::testing::generate("my-blog", picks, &[&Gtag])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::assert_formatted(&files);
        let plugins = file(&files, "src/plugins/mod.rs");
        assert!(
            plugins.contains(".exclude_path(crate::modules::admin::MODULE.mount_at)"),
            "{picks:?}: the registration reads the admin's path from its module file:\n{plugins}"
        );
        if picks.contains(&"config") {
            let exclude = file(&files, "config/development.toml")
                .lines()
                .find(|line| line.starts_with("exclude = "))
                .expect("[gtag] exclude");
            assert_eq!(exclude, "exclude = []", "the admin's path is not copied");
        }
        if testing::building() {
            let mut files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Gtag])
                .unwrap_or_else(|e| panic!("{e}"));
            brocanth_cli::testing::move_admin(&mut files, "/backoffice")
                .unwrap_or_else(|e| panic!("{e}"));
            testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
        }
    }

    // Without the admin, nothing names it.
    let files = brocanth_cli::testing::generate("my-blog", &["config", "gtag"], &[&Gtag])
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!file(&files, "src/plugins/mod.rs").contains("exclude_path"));
}
