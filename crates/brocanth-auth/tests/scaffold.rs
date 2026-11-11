//! The modules this crate writes into a project beside `auth`, through the
//! CLI's own generator.
//!
//! Each shape is generated, checked with rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project and run its tests too with `BROCANTH_BUILD=1`.

use brocanth_auth::scaffold::{Auth, AuthLockout, AuthOauth, AuthScreens, Roles, Superuser};
use brocanth_scaffold::testing;
use brocanth_scaffold::Module;

const MODULES: &[&'static dyn Module] = &[
    &Auth,
    &AuthOauth,
    &Superuser,
    &Roles,
    &AuthScreens,
    &AuthLockout,
];

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

/// Generates `picks`, checks that rustfmt leaves it alone, and compares it
/// with `tests/golden/<name>.txt`.
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
    let plugin = file(&files, "src/plugins/auth_screens/mod.rs");
    assert!(plugin.contains("mod tests"));
    assert!(plugin.contains("fn every_form_without_the_csrf_token_is_refused"));
    assert!(
        !plugin.contains("a_locked_account"),
        "no lockout test without the lockout"
    );
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
    assert!(file(&files, "src/plugins/auth_screens/mod.rs")
        .contains("fn a_locked_account_answers_like_a_wrong_password_until_a_reset"));
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

/// Provider sign-in with a config file and the CSRF hook: the settings are
/// a section, and the button forms carry the token.
#[test]
fn provider_sign_in_mounts_the_flow_and_reads_its_section() {
    let files = shape(
        "auth-oauth",
        &[
            "config",
            "middleware",
            "middleware-csrf",
            "auth",
            "auth-oauth",
        ],
    );
    // A printed `Settings` shows no client secret.
    let plugin = file(&files, "src/plugins/auth_oauth/mod.rs");
    assert!(!plugin.contains("derive(Debug"), "{plugin}");
    for provider in ["github", "google", "microsoft", "gitlab"] {
        assert!(
            plugin.contains(&format!(
                "&self.{provider}_client_secret.as_ref().map(|_| \"<redacted>\")"
            )),
            "{provider}: {plugin}"
        );
    }
    assert!(plugin.contains("fn a_printed_settings_never_shows_a_secret()"));
}

/// Superusers and roles with the admin, accounts in Postgres and a command
/// line: the admin's policy is `StaffPolicy`, its principal the member of
/// staff, the roles tab is in, and the binary has `createsuperuser`.
#[test]
fn staff_open_the_admin_and_get_a_command() {
    shape(
        "staff",
        &[
            "config",
            "commands",
            "db",
            "cache",
            "admin",
            "auth",
            "superuser",
            "roles",
        ],
    );
}

/// A superuser in memory, with no config file and no admin: no command,
/// because an account a command makes in memory is gone when it exits.
#[test]
fn a_superuser_in_memory_is_made_at_start_only() {
    shape("superuser-memory", &["auth", "superuser"]);
}

/// `roles` without `superuser` is refused, with the reason.
#[test]
fn roles_need_a_superuser() {
    let refused = brocanth_cli::testing::generate("my-blog", &["auth", "roles"], MODULES)
        .expect_err("roles without superuser is refused");
    assert!(refused.contains("superuser"), "{refused}");
}
