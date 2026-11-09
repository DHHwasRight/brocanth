//! The modules this crate writes beside `auth`, generated into a project by
//! the CLI's generator and compared with `tests/golden/`.
//!
//! Bless with `BROCANTH_BLESS=1`. Build each project too with
//! `BROCANTH_BUILD=1`, which runs `cargo build` in a temporary directory.

use brocanth_auth::scaffold::{AuthOauth, Roles, Superuser};
use brocanth_scaffold::testing;

/// Generates `picks`, checks that rustfmt leaves it alone, and compares it
/// with `tests/golden/<name>.txt`.
fn shape(name: &str, picks: &[&'static str]) {
    let modules: [&dyn brocanth_scaffold::Module; 3] = [&AuthOauth, &Superuser, &Roles];
    let files = brocanth_cli::testing::generate("my-blog", picks, &modules)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &modules)
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// Provider sign-in with a config file and the CSRF hook: the settings are
/// a section, and the button forms carry the token.
#[test]
fn provider_sign_in_mounts_the_flow_and_reads_its_section() {
    shape(
        "auth-oauth",
        &[
            "config",
            "middleware",
            "middleware-csrf",
            "auth",
            "auth-oauth",
        ],
    );
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
    let refused = brocanth_cli::testing::generate(
        "my-blog",
        &["auth", "roles"],
        &[&AuthOauth, &Superuser, &Roles],
    )
    .expect_err("roles without superuser is refused");
    assert!(refused.contains("superuser"), "{refused}");
}
