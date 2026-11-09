//! The modules this crate writes into a project beside `auth`, through the
//! CLI's own generator.
//!
//! Each shape is generated, checked with rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project and run its tests too with `BROCANTH_BUILD=1`.

#![cfg(feature = "scaffold")]

use brocanth_auth::scaffold::{Auth, AuthLockout, AuthScreens};
use brocanth_scaffold::testing;
use brocanth_scaffold::Module;

const MODULES: &[&'static dyn Module] = &[&Auth, &AuthScreens, &AuthLockout];

/// What `auth-screens` needs, and the config file.
const SCREENS: &[&str] = &[
    "config",
    "auth",
    "auth-screens",
    "rsx",
    "input",
    "middleware",
    "middleware-csrf",
    "mail",
    "flash",
];

fn shape(name: &str, picks: &[&'static str]) -> Vec<(String, String)> {
    let files = brocanth_cli::testing::generate("my-blog", picks, MODULES)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        // Named after the shape: two builds in one process with one name
        // would share a directory.
        let built = brocanth_cli::testing::generate_for_build(name, picks, MODULES)
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

/// Accounts in memory: the pages, and a test that walks sign-up and
/// sign-in through the application.
#[test]
fn the_screens_are_pages_with_a_csrf_field_in_every_form() {
    let files = shape("auth-screens", SCREENS);
    let markup = file(&files, "src/pages/account/screens.rsx");
    let forms = markup.matches("<form method=\"post\"").count();
    assert_eq!(forms, 8, "every screen posts");
    assert_eq!(markup.matches("{! csrf}").count(), forms, "{markup}");
    assert!(file(&files, "src/plugins/auth_screens/mod.rs").contains("mod tests"));
}

/// Accounts in Postgres: the same pages, and no test that needs a database.
#[test]
fn with_postgres_the_generated_test_is_left_out() {
    let mut picks = SCREENS.to_vec();
    picks.push("jobs");
    let files = brocanth_cli::testing::generate("my-blog", &picks, MODULES)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(!file(&files, "src/plugins/auth_screens/mod.rs").contains("mod tests"));
}

/// A screen without the CSRF check is refused, not written.
#[test]
fn the_screens_refuse_a_project_without_the_csrf_check() {
    let picks: Vec<&str> = SCREENS
        .iter()
        .copied()
        .filter(|k| *k != "middleware-csrf")
        .collect();
    let refused = brocanth_cli::testing::generate("my-blog", &picks, MODULES);
    assert!(refused.is_err(), "auth-screens was written without CSRF");
}

/// The screens with the lockout: sign-in counts wrong passwords, and a
/// reset lifts the lock.
#[test]
fn with_the_lockout_sign_in_goes_through_it() {
    let mut picks = SCREENS.to_vec();
    picks.push("auth-lockout");
    let files = shape("auth-lockout", &picks);
    let signin = file(&files, "src/pages/account/signin.rs");
    assert!(signin.contains("auth_lockout::lockout(&auth)"), "{signin}");
    assert!(!signin.contains("auth.accounts().login("), "{signin}");
    assert!(file(&files, "src/pages/account/reset/[token].rs").contains(".clear(&account)"));
    assert!(file(&files, "src/plugins/auth_lockout.rs")
        .contains("fn the_policy_in_the_module_file_is_valid"));
}

/// The lockout alone: the function, and no page.
#[test]
fn the_lockout_alone_writes_no_page() {
    let files = brocanth_cli::testing::generate("my-blog", &["auth", "auth-lockout"], MODULES)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    assert!(files
        .iter()
        .all(|(path, _)| !path.starts_with("src/pages/account/")));
    file(&files, "src/plugins/auth_lockout.rs");
}
