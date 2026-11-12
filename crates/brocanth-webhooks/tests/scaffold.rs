//! The `webhooks`, `webhooks-in` and `webhooks-out` modules, written into a
//! project by the CLI's generator.
//!
//! Each shape is checked by rustfmt and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project too with `BROCANTH_BUILD=1`.

use brocanth_scaffold::testing;
use brocanth_webhooks::scaffold::{Webhooks, WebhooksIn, WebhooksOut};

fn shape(name: &str, picks: &[&'static str]) {
    let modules: [&dyn brocanth_scaffold::Module; 3] = [&Webhooks, &WebhooksIn, &WebhooksOut];
    let files = brocanth_cli::testing::generate("my-shop", picks, &modules)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-shop", picks, &modules)
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// Without a database the keys are in memory, and the generated test signs
/// a delivery with the development secret and sends it twice.
#[test]
fn incoming_webhooks_keep_their_keys_in_memory_without_a_database() {
    shape("webhooks-in", &["config", "webhooks", "webhooks-in"]);
}

/// With a database the keys are rows, created on the first readiness check.
#[test]
fn incoming_webhooks_keep_their_keys_in_the_database_with_one() {
    shape(
        "webhooks-in-db",
        &["config", "db", "webhooks", "webhooks-in"],
    );
}

/// Outgoing webhooks with the admin: the deliveries tab is an integration,
/// behind `integrate.admin` in `src/modules/webhooks_out.rs`.
#[test]
fn outgoing_webhooks_add_a_deliveries_tab_to_the_admin() {
    shape(
        "webhooks-out",
        &["config", "db", "jobs", "admin", "webhooks", "webhooks-out"],
    );
}

/// `webhooks-out` without `db` is refused, with the reason. It needs no
/// `jobs`: its queue is its own, on the `db` module's pool.
#[test]
fn outgoing_webhooks_without_a_database_are_refused_with_the_reason() {
    let modules: [&dyn brocanth_scaffold::Module; 3] = [&Webhooks, &WebhooksIn, &WebhooksOut];
    let wrong = brocanth_cli::testing::generate("my-shop", &["webhooks", "webhooks-out"], &modules)
        .expect_err("db is needed");
    assert!(wrong.contains("db"), "{wrong}");
    brocanth_cli::testing::generate("my-shop", &["db", "webhooks", "webhooks-out"], &modules)
        .unwrap_or_else(|e| panic!("jobs is not needed: {e}"));
}

/// Outside development, the `example` source refuses the public
/// development secret, and a config file with no secret starts with none.
#[test]
fn incoming_webhooks_refuse_the_development_secret_outside_development() {
    let modules: [&dyn brocanth_scaffold::Module; 3] = [&Webhooks, &WebhooksIn, &WebhooksOut];
    let files = brocanth_cli::testing::generate(
        "my-shop",
        &["config", "webhooks", "webhooks-in"],
        &modules,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let plugin = files
        .iter()
        .find(|(path, _)| path.as_str() == "src/plugins/webhooks_in/mod.rs")
        .map(|(_, text)| text.as_str())
        .expect("the plugin file");
    assert!(plugin.contains("secret: String::new(),"), "{plugin}");
    assert!(plugin.contains("secret == DEVELOPMENT_SECRET && environment != \"development\""));
    let development = files
        .iter()
        .find(|(path, _)| path.as_str() == "config/development.toml")
        .map(|(_, text)| text.as_str())
        .expect("the development file");
    assert!(development.contains("whsec_"), "{development}");
}
