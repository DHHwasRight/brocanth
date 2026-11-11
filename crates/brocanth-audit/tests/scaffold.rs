//! The `audit` module, written into a project by the CLI's generator.
//!
//! Generates a project with the module, checks that rustfmt leaves every
//! Rust file alone, and compares the project with a golden file. Bless with
//! `BROCANTH_BLESS=1`; build each project too with `BROCANTH_BUILD=1`.

use brocanth_audit::scaffold::Audit;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Audit])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Audit])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With a config file and the admin: `[audit]` in the file, and the
/// History tab, link and journal behind `integrate.admin`.
#[test]
fn a_project_with_the_admin_gets_the_history_tab() {
    shape("audit-admin", &["config", "db", "model", "admin", "audit"]);
}

/// Without the admin: the plugin and the sink, no tab.
#[test]
fn a_project_without_the_admin_gets_the_plugin_and_the_sink() {
    shape("audit-no-admin", &["config", "db", "model", "audit"]);
}

/// The module needs `model`, and says why.
#[test]
fn audit_without_the_model_layer_is_refused_with_the_reason() {
    let wrong = brocanth_cli::testing::generate("my-blog", &["db", "audit"], &[&Audit])
        .expect_err("audit needs model");
    assert!(wrong.contains("model"), "{wrong}");
}
