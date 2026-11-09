//! The `storage-routes` module, written into a project by the CLI's
//! generator.
//!
//! Each shape is checked by rustfmt and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project too with `BROCANTH_BUILD=1`.

use brocanth_scaffold::testing;
use brocanth_storage::scaffold::{FileStorage, StorageRoutes};

fn shape(name: &str, picks: &[&'static str]) {
    let modules: [&dyn brocanth_scaffold::Module; 2] = [&FileStorage, &StorageRoutes];
    let files = brocanth_cli::testing::generate("my-photos", picks, &modules)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-photos", picks, &modules)
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With a config file and the CSRF check: the settings are a section, and
/// the `lift` hook carries a plain form's token to the check.
#[test]
fn the_routes_sit_around_the_csrf_check() {
    shape(
        "storage-routes",
        &[
            "config",
            "middleware",
            "middleware-csrf",
            "storage",
            "storage-routes",
        ],
    );
}

/// Without a config file: the settings are literals and flags.
#[test]
fn the_routes_work_without_a_config_file() {
    shape("storage-routes-no-config", &["storage", "storage-routes"]);
}
