//! The `db` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_db::scaffold::{Database, DbEmbedded};
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Database, &DbEmbedded];

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

/// The pool alone, with no config file and no command line: `[db]` comes
/// from flags and the environment.
#[test]
fn the_pool_without_a_config_file() {
    shape("db-flags", &["db"]);
}

/// The pool under the model layer, with a command line: the `migrate`
/// command runs the migrations.
#[test]
fn the_pool_under_the_models_with_a_command_line() {
    shape("db-model-commands", &["config", "db", "model", "commands"]);
}

/// The pool on a development server from the binary, with no config file:
/// `[db_embedded]` comes from flags and the environment.
#[test]
fn the_pool_on_an_embedded_server_without_a_config_file() {
    shape("db-embedded-flags", &["db", "db-embedded"]);
}

/// The same with a config file and a command line: `[db_embedded]` in the
/// file, and `migrate` and `dbshell` still on `[db] url`.
#[test]
fn the_pool_on_an_embedded_server_with_a_config_file() {
    shape(
        "db-embedded-config",
        &["config", "db", "db-embedded", "commands"],
    );
}
