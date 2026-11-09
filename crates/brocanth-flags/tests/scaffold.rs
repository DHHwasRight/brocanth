//! The `flags` module, written into a project by the CLI's generator.
//!
//! Generates a project with the module, checks that rustfmt leaves every
//! Rust file alone, and compares the project with a golden file. Bless with
//! `BROCANTH_BLESS=1`; build each project too with `BROCANTH_BUILD=1`.

use brocanth_flags::scaffold::FeatureFlags;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&FeatureFlags])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&FeatureFlags])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With a config file and the admin: `[flags]` in the file, and a tab at
/// `/admin/flags` that lists the declared flags.
#[test]
fn a_project_with_the_admin_gets_flags_and_their_tab() {
    shape("flags-admin", &["config", "db", "admin", "flags"]);
}

/// Without a config file: the settings are flags and variables.
#[test]
fn a_project_without_config_reads_the_settings_from_flags() {
    shape("flags-no-config", &["db", "flags"]);
}

/// The module needs `db`, and says why.
#[test]
fn flags_without_a_database_is_refused_with_the_reason() {
    let wrong = brocanth_cli::testing::generate("my-blog", &["flags"], &[&FeatureFlags])
        .expect_err("flags needs db");
    assert!(wrong.contains("db"), "{wrong}");
}
