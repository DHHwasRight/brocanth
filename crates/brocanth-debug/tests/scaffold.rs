//! The `debug` and `debug-toolbar` modules, written into a project by the
//! CLI's generator.
//!
//! Bless with `BROCANTH_BLESS=1`; build each project too with
//! `BROCANTH_BUILD=1`. See `crates/brocanth-robots/tests/scaffold.rs` for
//! the pattern.

use brocanth_debug::scaffold::{Debug, DebugToolbar};
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) -> Vec<(String, String)> {
    let modules: [&dyn brocanth_scaffold::Module; 2] = [&Debug, &DebugToolbar];
    let files = brocanth_cli::testing::generate("my-blog", picks, &modules)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let built = brocanth_cli::testing::generate_for_build("my-blog", picks, &modules)
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

/// With a config file and no ORM: `[debug_toolbar]`, the hooks, and the
/// toolbar told the chains at start.
#[test]
fn the_toolbar_is_registered_and_learns_the_chains_at_start() {
    let files = shape("debug-toolbar", &["config", "debug", "debug-toolbar"]);
    let plugins = file(&files, "src/plugins/mod.rs");
    assert!(
        plugins.contains("toolbar.observe($app.pipeline(), crate::plugins::settings_report());")
    );
    let manifest = file(&files, "Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.starts_with("brocanth-debug"))
        .expect("the dependency");
    assert!(
        line.contains("\"config\"") && !line.contains("\"model\""),
        "{line}"
    );
    // The deploy check knows the module is for development.
    let receipt = file(&files, "brocanth.toml");
    assert!(receipt.contains("debug-toolbar"), "{receipt}");
}

/// With the ORM: the crate's `model` feature, and the model plugin hands
/// its connection to the toolbar's probe.
#[test]
fn with_the_orm_the_toolbar_counts_its_statements() {
    let files = shape(
        "debug-toolbar-model",
        &["config", "db", "model", "debug", "debug-toolbar"],
    );
    let model = file(&files, "src/plugins/model/mod.rs");
    assert!(model.contains("orm.probed(probe)"), "{model}");
    let manifest = file(&files, "Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.starts_with("brocanth-debug"))
        .expect("the dependency");
    assert!(line.contains("\"model\""), "{line}");
}
