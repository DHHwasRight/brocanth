//! The `oauth`, `oauth-client` and `oauth-client-memory` modules, written
//! into a project by the CLI's generator.
//!
//! Each shape is checked by rustfmt and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project and run its tests too with `BROCANTH_BUILD=1`.

use brocanth_oauth_client::scaffold::{Oauth, OauthClient, OauthClientMemory};
use brocanth_scaffold::testing;

const MODULES: [&dyn brocanth_scaffold::Module; 3] = [&Oauth, &OauthClient, &OauthClientMemory];

fn shape(name: &str, picks: &[&'static str]) -> Vec<(String, String)> {
    let files = brocanth_cli::testing::generate("my-app", picks, &MODULES)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        // Named after the shape: two builds in one process with one name
        // would share a directory.
        let built = brocanth_cli::testing::generate_for_build(name, picks, &MODULES)
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
        .unwrap_or_else(|| panic!("no {path}"))
}

/// The production shape: tokens in Postgres, the CSRF token on the
/// forms, and the connections tab in the admin.
#[test]
fn the_client_keeps_tokens_in_postgres_and_adds_an_admin_tab() {
    let files = shape(
        "oauth-client",
        &[
            "config",
            "db",
            "auth",
            "middleware",
            "middleware-csrf",
            "admin",
            "oauth",
            "oauth-client",
        ],
    );
    let plugin = file(&files, "src/plugins/oauth_client/mod.rs");
    assert!(plugin.contains("PgTokenStore::new(db)"));
    assert!(plugin.contains("stamped(ctx, page.html())"));
    let mods = file(&files, "src/plugins/mod.rs");
    assert!(
        mods.contains("ConnectionsPanel::installed()"),
        "the admin tab"
    );
}

/// Tests and development: the tokens in memory, no CSRF module and no
/// config file, so the settings are read from the environment.
#[test]
fn the_memory_store_needs_no_migration_and_no_config_file() {
    let files = shape(
        "oauth-client-memory",
        &["db", "auth", "oauth", "oauth-client", "oauth-client-memory"],
    );
    let plugin = file(&files, "src/plugins/oauth_client/mod.rs");
    assert!(plugin.contains("MemoryTokenStore::new()"));
    assert!(!plugin.contains("migrate"), "{plugin}");
    assert!(!plugin.contains("stamped"));
}

/// `oauth-client` without `auth` is refused, with the reason.
#[test]
fn the_client_without_auth_is_refused_with_the_reason() {
    let wrong =
        brocanth_cli::testing::generate("my-app", &["db", "oauth", "oauth-client"], &MODULES)
            .expect_err("auth is needed");
    assert!(
        wrong.contains("auth") && wrong.contains("signed-in"),
        "{wrong}"
    );
}

/// The sealing key and each client secret print as `<redacted>`, in both
/// shapes, and the generated file carries the test that proves it.
#[test]
fn a_printed_settings_never_shows_a_secret() {
    for picks in [
        &["config", "db", "auth", "oauth", "oauth-client"][..],
        &["db", "auth", "oauth", "oauth-client", "oauth-client-memory"][..],
    ] {
        let files = brocanth_cli::testing::generate("my-app", picks, &MODULES)
            .unwrap_or_else(|e| panic!("{e}"));
        let plugin = file(&files, "src/plugins/oauth_client/mod.rs");
        assert!(!plugin.contains("derive(Debug"), "{plugin}");
        assert!(
            plugin.contains("impl std::fmt::Debug for Settings"),
            "{plugin}"
        );
        for secret in [
            "key",
            "github_client_secret",
            "google_client_secret",
            "microsoft_client_secret",
            "gitlab_client_secret",
        ] {
            assert!(
                plugin.contains(&format!("&self.{secret}.as_ref().map(|_| \"<redacted>\")")),
                "{secret} is printed: {plugin}"
            );
        }
        assert!(plugin.contains("fn a_printed_settings_never_shows_a_secret()"));
    }
}
