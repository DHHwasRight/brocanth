//! The `openapi` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_openapi::scaffold::OpenApi;
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&OpenApi];

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

/// With a config file and `input`: the `[openapi]` section, and a document
/// that lists the declared bodies.
#[test]
fn the_document_lists_the_declared_bodies() {
    shape("openapi-input", &["config", "input", "openapi"]);
}

/// The module alone: the settings from flags, and a document of paths
/// with no body schema.
#[test]
fn the_module_alone() {
    shape("openapi-flags", &["openapi"]);
}

/// With the admin: an `API` tab, and the admin's own routes hidden from
/// the public document, beside the document's two endpoints.
#[test]
fn with_the_admin_the_document_has_a_tab_and_hides_the_admin() {
    shape("openapi-admin", &["config", "admin", "openapi"]);
    let files =
        brocanth_cli::testing::generate("my-blog", &["config", "admin", "openapi"], MODULES)
            .unwrap_or_else(|e| panic!("{e}"));
    let file = |name: &str| {
        files
            .iter()
            .find(|(path, _)| path.as_str() == name)
            .map(|(_, text)| text.as_str())
            .unwrap_or_else(|| panic!("{name} is written"))
    };
    let plugin = file("src/plugins/openapi/mod.rs");
    assert!(plugin.contains(".hide(ROUTE, Verb::Get)"), "{plugin}");
    assert!(plugin.contains(".hide(YAML_ROUTE, Verb::Get)"), "{plugin}");
    assert!(
        plugin.contains(".hide_under(crate::modules::admin::MODULE.mount_at)"),
        "{plugin}"
    );
    assert!(plugin.contains("served.install();"), "{plugin}");
    assert!(file("src/plugins/mod.rs").contains("ApiPanel::installed()"));
}
