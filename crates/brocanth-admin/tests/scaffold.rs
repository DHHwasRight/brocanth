//! The `admin` modules, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_admin::scaffold::{Backoffice, BackofficeActions, BackofficeModels};
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Backoffice, &BackofficeActions, &BackofficeModels];

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

/// The admin over the model layer, with its bulk actions and a config
/// file: one line per entity, and "Delete selected" on each list.
#[test]
fn the_admin_over_the_models_with_actions() {
    shape(
        "admin-model",
        &[
            "config",
            "db",
            "model",
            "admin",
            "admin-model",
            "admin-actions",
        ],
    );
}

/// The admin alone, with a config file and nothing to list: the locked
/// door and its settings.
#[test]
fn the_admin_alone_with_a_config_file() {
    shape("admin-config", &["config", "admin"]);
}
