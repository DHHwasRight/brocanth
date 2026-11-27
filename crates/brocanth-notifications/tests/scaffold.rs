//! The `notifications` module, written into a project by the CLI's
//! generator.
//!
//! Bless with `BROCANTH_BLESS=1`; build each project too with
//! `BROCANTH_BUILD=1`. See `crates/brocanth-robots/tests/scaffold.rs` for
//! the pattern.

use brocanth_notifications::scaffold::Notifications;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) -> Vec<(String, String)> {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Notifications])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let built = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Notifications])
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
        .unwrap_or_else(|| panic!("{path} is generated"))
}

/// The inbox alone: the page, the plugin, `[notifications]`, and no mail
/// or realtime code.
#[test]
fn the_inbox_is_a_page_over_the_signed_in_account() {
    let files = shape(
        "notifications",
        &[
            "config",
            "commands",
            "db",
            "auth",
            "middleware",
            "middleware-csrf",
            "notifications",
        ],
    );
    let page = file(&files, "src/pages/notifications.rs");
    assert!(page.contains("store.inbox(&account)"), "{page}");
    let plugin = file(&files, "src/plugins/notifications/mod.rs");
    assert!(
        !plugin.contains("Outbox") && !plugin.contains("publish"),
        "{plugin}"
    );
    let config = file(&files, "config/development.toml");
    assert!(config.contains("[notifications]"), "{config}");
}

/// With mail, jobs, realtime and policy: `notify` takes an address and
/// publishes the row, and the page ANDs a policy rule.
#[test]
fn with_mail_realtime_and_policy_notify_mails_and_publishes() {
    let files = shape(
        "notifications-everything",
        &[
            "config",
            "commands",
            "db",
            "jobs",
            "mail",
            "auth",
            "middleware",
            "middleware-csrf",
            "realtime",
            "policy",
            "notifications",
        ],
    );
    let plugin = file(&files, "src/plugins/notifications/mod.rs");
    assert!(plugin.contains("email: Option<&str>"), "{plugin}");
    assert!(
        plugin.contains("brocanth_notifications::publish("),
        "{plugin}"
    );
    let page = file(&files, "src/pages/notifications.rs");
    assert!(
        page.contains("condition::<Notification>(Dialect::Postgres)"),
        "{page}"
    );
    let manifest = file(&files, "Cargo.toml");
    let line = manifest
        .lines()
        .find(|l| l.starts_with("brocanth-notifications"))
        .expect("the dependency");
    assert!(line.contains("\"realtime\""), "{line}");
}

/// With policy and rls: the table's `#[db]` policy, the line `schema()`
/// takes, the reader's identity on the page, and `account` on `Visitor`.
#[test]
fn with_policy_and_rls_the_database_holds_the_inbox_rule() {
    let files = shape(
        "notifications-rls",
        &[
            "config",
            "commands",
            "db",
            "auth",
            "middleware",
            "middleware-csrf",
            "policy",
            "rls",
            "notifications",
        ],
    );
    let plugin = file(&files, "src/plugins/notifications/mod.rs");
    assert!(plugin.contains("#[db]"), "{plugin}");
    assert!(
        plugin.contains("list: account == who.account else deny,"),
        "{plugin}"
    );
    assert!(plugin.contains("create: never,"), "{plugin}");
    assert!(plugin.contains(".inserter(notifier())"), "{plugin}");
    assert!(
        plugin.contains("DbIdentity::new().role(notifier().name())"),
        "{plugin}"
    );
    assert!(
        plugin.contains("url: crate::plugins::rls::agreement_url(),"),
        "{plugin}"
    );
    assert!(plugin.contains(".as_identity(writer)"), "{plugin}");
    let schema = file(&files, "src/plugins/rls/mod.rs");
    assert!(
        schema.contains(".policies(crate::plugins::notifications::db_policies(&app_user()))"),
        "{schema}"
    );
    assert!(
        schema.contains(r#"["account_id", "admin", "account"]"#),
        "{schema}"
    );
    let rules = file(&files, "src/plugins/policy/mod.rs");
    assert!(rules.contains("pub account: Option<String>,"), "{rules}");
    let page = file(&files, "src/pages/notifications.rs");
    assert_eq!(
        page.matches(".as_identity(reader(&account))").count(),
        2,
        "the list and the mark-read post: {page}"
    );
}
