//! The `mail` module, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked with rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build each
//! project too with `BROCANTH_BUILD=1`.

#![cfg(feature = "scaffold")]

use brocanth_mail::scaffold::Mail;
use brocanth_scaffold::testing;

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, &[&Mail])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, &[&Mail])
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// With a config file and no queue: `deliver_later` sends at once, and the
/// development file sends mail to the log.
#[test]
fn a_project_without_a_queue_sends_at_once() {
    shape("mail", &["config", "mail"]);
}

/// With `jobs`: the `mail` queue, its job, and a worker of its own.
#[test]
fn a_project_with_a_queue_delivers_later_from_a_worker() {
    shape("mail-jobs", &["config", "jobs", "mail"]);
}

/// No config file: the settings come from the environment and the flags,
/// and the SMTP password has no flag.
#[test]
fn without_a_config_file_the_password_has_no_flag() {
    let files = brocanth_cli::testing::generate("my-blog", &["mail"], &[&Mail])
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    let flags = files
        .iter()
        .find(|(path, _)| path == "src/plugins/mod.rs")
        .map(|(_, text)| text.as_str())
        .expect("src/plugins/mod.rs");
    assert!(flags.contains("mail.smtp_host"), "{flags}");
    assert!(!flags.contains("--mail.smtp_password"), "{flags}");
}
