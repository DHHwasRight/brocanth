//! The settings tab, rendered through the real admin.
//!
//! These assert on HTML and not on a context struct, because a screen that
//! compiles and prints the wrong thing is the failure worth catching — and
//! here the wrong thing is a password. The admin is real, the policy is real,
//! and the configuration is real files on a real disk, layered by the real
//! loader.

#![cfg(all(feature = "admin", feature = "toml"))]

use std::path::Path;
use std::sync::Arc;

use brocanth_admin::policy::{policy_fn, Action, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_config::admin::SettingsPanel;
use brocanth_config::{Config, Loader, Redaction};
use brocanth_test::Html;

struct Operator;

// ---------------------------------------------------------------------------
// Scaffolding
// ---------------------------------------------------------------------------

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn lookup(
    pairs: &[(&str, &str)],
) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static + use<> {
    let map: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move |k: &str| {
        map.iter()
            .find(|(name, _)| name == k)
            .map(|(_, value)| value.clone())
    }
}

fn vars(pairs: &[(&str, &str)]) -> impl Fn() -> Vec<(String, String)> + Send + Sync + 'static {
    let owned: Vec<(String, String)> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    move || owned.clone()
}

fn admin(panel: SettingsPanel) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(panel)
            // Reading is allowed and acting is not, because this panel has no
            // buttons and must not grow one by accident.
            .with_policy(policy_fn(|_: &Principal<'_>, r: Request<'_>| {
                r.action != Action::ActOnPanel
            })),
    )
}

fn get(admin: &Admin, path: &str) -> Html {
    let who = Operator;
    let response =
        block_on(admin.dispatch(Principal::new(&who), "GET", path, "", &Submission::new()))
            .expect("dispatch");
    assert_eq!(response.status(), 200, "{path}");
    Html::new(response.html().expect("a page").to_string())
}

/// A configuration layered over four real files plus the environment, so the
/// screen under test has something to show the layering of.
fn layered(dir: &Path) -> Config {
    // 4. The committed defaults.
    write(
        &dir.join("config/development.toml"),
        "[server]\nhost = \"10.0.0.1\"\nport = 3100\n[cache]\nkey_prefix = \"shop\"\n",
    );
    // 3. The checkout.
    write(
        &dir.join("shop_development.toml"),
        "[server]\nport = 3200\n[database]\nurl = \"postgres://app:hunter2@db.internal:5432/shop\"\n",
    );
    // 2. The machine.
    write(
        &dir.join("etc/shop/development.toml"),
        "[smtp]\nhost = \"mail.internal\"\npassword = \"correct-horse\"\n",
    );
    // 1. The deployment, pointed at by hand.
    let explicit = dir.join("deploy.toml");
    write(&explicit, "[server]\nport = 8080\n");

    Loader::new("shop")
        .env("development")
        .root(dir)
        .cwd(dir)
        .env_lookup(lookup(&[("SHOP_CONFIG", explicit.to_str().unwrap())]))
        .env_vars(vars(&[("SHOP_SERVER__WORKERS", "16")]))
        .load()
        .expect("load")
}

// ---------------------------------------------------------------------------
// Redaction. Not optional.
// ---------------------------------------------------------------------------

#[test]
fn a_password_never_reaches_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let page = get(
        &admin(SettingsPanel::new(layered(dir.path()))),
        "/admin/settings",
    );

    // The two shapes a secret arrives in, and neither of them is printed.
    page.assert_no_text("hunter2");
    page.assert_no_text("correct-horse");

    // The key named like a secret is hidden whole, and the screen says which
    // rule hid it, so a false positive is a key to rename rather than a
    // mystery.
    page.assert_text("smtp.password");
    page.assert_text("[redacted]");
    page.assert_text("hidden: the key is named like a secret");

    // The URL keeps everything that answers "which database is this process
    // pointed at", which is the whole reason the string is not masked whole.
    page.assert_text("postgres://app:[redacted]@db.internal:5432/shop");
    page.assert_text("hidden: the URL carried a password");
    page.assert_text("2 values are hidden.");
}

#[test]
fn a_password_that_was_overridden_is_still_a_password() {
    // The `was ...` lines are the layering, which is the point of the screen.
    // They are also a second copy of every value, and a redaction that forgot
    // them would print the secret one line lower down.
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("config/development.toml"),
        "[smtp]\npassword = \"the-old-one\"\n",
    );
    write(
        &root.join("shop_development.toml"),
        "[smtp]\npassword = \"the-new-one\"\n",
    );
    let config = Loader::new("shop")
        .env("development")
        .root(root)
        .cwd(root)
        .env_lookup(lookup(&[]))
        .load()
        .expect("load");

    let page = get(&admin(SettingsPanel::new(config)), "/admin/settings");
    page.assert_no_text("the-new-one");
    page.assert_no_text("the-old-one");
    // The layer is still shown, and still attributed: what changed is that
    // the value in it is not readable.
    page.assert_text("was [redacted]");
    page.assert_text("development.toml");
}

#[test]
fn a_key_that_merely_mentions_a_key_is_still_readable() {
    // Over-redaction is the right bias and it is not free. The one
    // refinement — `key` counts only as the last segment — is what keeps the
    // tab worth opening.
    let dir = tempfile::tempdir().unwrap();
    let page = get(
        &admin(SettingsPanel::new(layered(dir.path()))),
        "/admin/settings",
    );
    page.assert_text("cache.key_prefix");
    page.assert_text("\"shop\"");
}

#[test]
fn an_application_can_widen_the_rule_with_its_own_vocabulary() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("config/development.toml"),
        "[payments]\nsas = \"sv=2021&sig=abcdef\"\n",
    );
    let config = Loader::new("shop")
        .env("development")
        .root(root)
        .cwd(root)
        .env_lookup(lookup(&[]))
        .load()
        .expect("load");

    let plain = get(
        &admin(SettingsPanel::new(config.clone())),
        "/admin/settings",
    );
    plain.assert_text("sv=2021");

    let widened = get(
        &admin(SettingsPanel::new(config).redacting(Redaction::standard().also("sas"))),
        "/admin/settings",
    );
    widened.assert_no_text("sv=2021");
    widened.assert_text("[redacted]");
}

// ---------------------------------------------------------------------------
// The screen itself
// ---------------------------------------------------------------------------

#[test]
fn the_screen_says_which_configuration_this_process_is_running() {
    let dir = tempfile::tempdir().unwrap();
    let page = get(
        &admin(SettingsPanel::new(layered(dir.path()))),
        "/admin/settings",
    );

    page.assert_text("Settings");
    page.assert_text("shop");
    page.assert_text("development");

    // Every section, and every key in it.
    page.assert_text("[server]");
    page.assert_text("[database]");
    page.assert_text("[smtp]");
    page.assert_text("[cache]");
    page.assert_text("server.host");
    page.assert_text("server.port");
    page.assert_text("server.workers");
    page.assert_text("smtp.host");
}

#[test]
fn every_value_says_which_file_and_line_decided_it() {
    // The layering is the point. A value, the file that set it, and every
    // layer it beat.
    let dir = tempfile::tempdir().unwrap();
    let page = get(
        &admin(SettingsPanel::new(layered(dir.path()))),
        "/admin/settings",
    );

    // The four slots are listed in precedence order, by path.
    page.assert_text("deploy.toml");
    page.assert_text("etc/shop/development.toml");
    page.assert_text("shop_development.toml");
    page.assert_text("config/development.toml");
    page.assert_text("env var");
    page.assert_text("working dir");
    page.assert_text("config dir");

    // The environment sits above every file, named in full.
    page.assert_text("SHOP_*");
    page.assert_text("SHOP_SERVER__WORKERS");
    page.assert_text("$SHOP_SERVER__WORKERS");

    // `server.port` is set in three files. The winner cites the deployment
    // file -- `[2]`, because the environment is source 1 and sits above every
    // file -- and the two layers it beat are under it with their own origins.
    page.assert_text("[2] deploy.toml:2");
    page.assert_text("was 3200");
    page.assert_text("was 3100");
}

#[test]
fn a_configuration_that_found_nothing_says_so_rather_than_drawing_an_empty_table() {
    let dir = tempfile::tempdir().unwrap();
    let config = Loader::new("shop")
        .env("development")
        .root(dir.path())
        .cwd(dir.path())
        .env_lookup(lookup(&[]))
        .load()
        .expect("load");

    let page = get(&admin(SettingsPanel::new(config)), "/admin/settings");
    page.assert_text("No configuration file was found");
    page.assert_text("This configuration sets nothing.");
}

#[test]
fn the_screen_admits_that_plugin_defaults_are_not_on_it() {
    // The panel is built before `AppBuilder::build()`, so the defaults layer
    // does not exist yet. Saying so is the difference between a screen that
    // is incomplete and a screen that is wrong.
    let dir = tempfile::tempdir().unwrap();
    let page = get(
        &admin(SettingsPanel::new(layered(dir.path()))),
        "/admin/settings",
    );
    page.assert_text("These are the values as found");
    page.assert_text("effective_of(app.services()).unwrap().dump()");
}

#[test]
fn the_panel_serves_one_path_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let a = admin(SettingsPanel::new(layered(dir.path())));
    let who = Operator;
    for (method, path) in [("GET", "/admin/settings/nope"), ("POST", "/admin/settings")] {
        let response =
            block_on(a.dispatch(Principal::new(&who), method, path, "", &Submission::new()))
                .expect("dispatch");
        assert_ne!(response.status(), 200, "{method} {path}");
    }
}

/// Prints the screen, for a human who wants to see it.
#[test]
#[ignore = "prints a page, asserts nothing"]
fn show() {
    let dir = tempfile::tempdir().unwrap();
    let page = get(
        &admin(SettingsPanel::new(layered(dir.path()))),
        "/admin/settings",
    );
    println!("{}", page.source());
}
