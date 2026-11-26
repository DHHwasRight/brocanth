//! The `backoffice` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_backoffice::scaffold::BackofficeService;
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&BackofficeService];

fn generate(picks: &[&'static str]) -> Vec<(String, String)> {
    brocanth_cli::testing::generate("my-blog", picks, MODULES).unwrap_or_else(|e| panic!("{e}"))
}

fn file<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p == path)
        .map(|(_, text)| text.as_str())
        .unwrap_or_else(|| panic!("{path} is not generated"))
}

fn shape(name: &str, picks: &[&'static str]) {
    let files = generate(picks);
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

/// The admin alone and its second binary, with a config file.
#[test]
fn the_admin_and_its_second_binary() {
    shape("backoffice", &["config", "admin", "backoffice"]);
}

/// With a command line and the model layer: the second binary reads the
/// flags itself, and reaches `migrations/` from its own directory.
#[test]
fn the_second_binary_beside_a_command_line() {
    shape(
        "backoffice-commands",
        &["config", "commands", "db", "model", "admin", "backoffice"],
    );
}

/// Staff, rows under the policy and row-level security: the claim carries
/// the member of staff's grants, and the data side runs as their database
/// identity.
#[test]
fn the_second_binary_with_staff_and_row_level_security() {
    shape(
        "backoffice-staff",
        &[
            "config",
            "commands",
            "db",
            "model",
            "auth",
            "admin",
            "admin-model",
            "superuser",
            "policy",
            "rls",
            "backoffice",
        ],
    );
}

/// Tabs with a data contract: the application's wire serves each one,
/// and the backoffice binary reads it over the wire when `data_url` is
/// set. The generated test runs both halves in one process.
#[test]
fn the_tabs_cross_the_wire() {
    shape(
        "backoffice-panels",
        &[
            "config",
            "commands",
            "db",
            "admin",
            "flags",
            "jobs",
            "cache",
            "analytics",
            "backoffice",
        ],
    );
}

/// The second binary compiles the application's own modules, each from
/// its own directory, and the manifest runs its tests once.
#[test]
fn the_second_binary_shares_the_applications_modules() {
    let files = generate(&["config", "commands", "db", "model", "admin", "backoffice"]);
    let bin = file(&files, "src/bin/backoffice.rs");
    for line in [
        "#[path = \"../app.rs\"]\nmod app;",
        "#[path = \"../commands/mod.rs\"]\nmod commands;",
        "#[path = \"../../migrations/mod.rs\"]\nmod migrations;",
        "#[path = \"../plugins/mod.rs\"]\nmod plugins;",
    ] {
        assert!(bin.contains(line), "{line} is missing:\n{bin}");
    }
    assert!(!bin.contains(brocanth_scaffold::APP_MODS), "{bin}");
    let manifest = file(&files, "Cargo.toml");
    assert!(
        manifest.contains(
            "[[bin]]\nname = \"backoffice\"\npath = \"src/bin/backoffice.rs\"\ntest = false"
        ),
        "{manifest}"
    );
    // The command line keeps `clap`, and `read_line` is there for the
    // second binary.
    let plugins = file(&files, "src/plugins/mod.rs");
    assert!(plugins.contains("pub fn flag_args("), "{plugins}");
    assert!(plugins.contains("pub fn read_line("), "{plugins}");
}

/// The admin goes on the contract through the generated
/// `plugins::admin_panels`, not through the admin's own file.
#[test]
fn the_admin_goes_on_the_contract_through_a_generated_file() {
    let files = generate(&["config", "admin", "backoffice"]);
    let plugins = file(&files, "src/plugins/mod.rs");
    assert!(
        plugins.contains("config = config.data_through(crate::plugins::backoffice::contract);"),
        "{plugins}"
    );
    assert!(!file(&files, "src/plugins/admin/mod.rs").contains("data_through"));
}

/// Which binary an application serves as is a value the binary builds it
/// with, not process-wide state: the plugin reads it from the services,
/// and neither generated file keeps a global.
#[test]
fn the_role_is_a_value_the_binary_builds_with() {
    let files = generate(&["config", "admin", "backoffice"]);
    let plugin = file(&files, "src/plugins/backoffice/mod.rs");
    let bin = file(&files, "src/bin/backoffice.rs");
    for text in [plugin, bin] {
        assert!(
            !text
                .lines()
                .any(|line| line.trim_start().starts_with("static ")
                    || line.contains("thread_local!")),
            "{text}"
        );
    }
    assert!(
        plugin.contains("crate::app::serve_with(&addr.to_string(), given())"),
        "{plugin}"
    );
    assert!(plugin.contains("lc.services().get::<Role>()"), "{plugin}");
    assert!(!plugin.contains("brocanth_backoffice::role()"), "{plugin}");
}

/// Without the admin there is nothing to serve.
#[test]
fn the_module_refuses_a_project_without_the_admin() {
    let refused = brocanth_cli::testing::generate("my-blog", &["config", "backoffice"], MODULES);
    assert!(refused.is_err(), "a backoffice with no admin was generated");
}
