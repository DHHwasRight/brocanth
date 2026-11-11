//! The `client` module, written into a project by the CLI's generator.
//!
//! The shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! the project too with `BROCANTH_BUILD=1`.

use brocanth_client::scaffold::Client;
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Client];

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

/// The module adapts to nothing, and needs `input` and `commands`: an
/// `api` subcommand built from the table `input` writes.
#[test]
fn an_api_subcommand_from_the_routes() {
    shape("client", &["commands", "input", "client"]);
}

/// Without `commands` the client has nowhere to go: the generator refuses
/// the project, and says what to add.
#[test]
fn a_client_without_commands_is_refused() {
    let wrong = brocanth_cli::testing::generate("my-blog", &["input", "client"], MODULES)
        .expect_err("client needs commands");
    assert!(wrong.contains("commands"), "{wrong}");
    assert!(wrong.contains("help:"), "{wrong}");
}
