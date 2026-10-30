//! `brocanth check` and `check --deploy`, run as a binary over a project on
//! disk. Nothing is built and nothing connects.

use std::path::{Path, PathBuf};
use std::process::Command;

const BROCANTH: &str = env!("CARGO_BIN_EXE_brocanth");

struct Run {
    stderr: String,
    code: i32,
}

fn run(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(BROCANTH)
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("CLICOLOR_FORCE")
        .env_remove("SHOP_ENV")
        .env_remove("SHOP_SESSION_KEY")
        .env("NO_COLOR", "1")
        .output()
        .expect("run brocanth");
    Run {
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code().unwrap_or(-1),
    }
}

/// What `brocanth create` writes for development, copied to production as
/// somebody would on the first deploy.
const COPIED: &str = r#"[logging]
level = "debug"

[sessions]
secure = false

[model]
url = "sqlite://db/production.sqlite?mode=rwc"

[auth]
signing_key_env = "SHOP_SESSION_KEY"
cookie_name = "session"
secure = false

[policy]
dialect = "sqlite"

[storage]
secret = "development only: replace before anything is uploaded in anger"
"#;

const PLUGINS: &str = r#"
        // let builder = builder.plugin_hooking(
        //     <brocanth_middleware::Csrf as brocanth::plugin_api::Plugin>::new(
        //         brocanth_middleware::CsrfConfig::default(),
        //     ),
        // );
"#;

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brocanth-check-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::create_dir_all(dir.join("src/plugins")).unwrap();
    std::fs::write(
        dir.join("brocanth.toml"),
        "[project]\nname = \"shop\"\n\n[modules]\nenabled = [\"sessions\", \"model\", \"auth\", \"middleware\", \"middleware-csrf\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("config/production.toml"), COPIED).unwrap();
    std::fs::write(dir.join("src/plugins/mod.rs"), PLUGINS).unwrap();
    dir
}

#[test]
fn a_copied_development_config_fails_the_deploy_checks_with_a_help_line_each() {
    let dir = project("copied");
    let r = run(&dir, &["check", "--deploy"]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    for id in [
        "error[deploy/cookie-insecure]",
        "warning[deploy/cookie-prefix]",
        "error[deploy/signing-key]",
        "error[deploy/default-secret]",
        "warning[deploy/debug-logging]",
        "error[deploy/csrf-off]",
        "warning[deploy/hsts]",
    ] {
        assert!(r.stderr.contains(id), "{id} missing:\n{}", r.stderr);
    }
    assert!(
        r.stderr.contains("--> config/production.toml:5"),
        "{}",
        r.stderr
    );
    let findings = r.stderr.matches("error[").count() + r.stderr.matches("warning[").count();
    assert_eq!(
        r.stderr.matches("= help:").count(),
        findings,
        "{}",
        r.stderr
    );
    assert!(
        r.stderr.contains("deploy checks for `production`"),
        "{}",
        r.stderr
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_deploy_only_the_checks_for_every_environment_run() {
    let dir = project("plain");
    let r = run(&dir, &["check", "--env", "production"]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(
        r.stderr.contains("checks pass for `production`"),
        "{}",
        r.stderr
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn allow_silences_one_finding_and_strict_fails_on_a_warning() {
    let dir = project("allow");
    std::fs::write(
        dir.join("config/production.toml"),
        "[logging]\nlevel = \"info\"\n\n[sessions]\nsecure = true\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/plugins/mod.rs"),
        "let builder = builder.plugin_hooking(\n    <brocanth_middleware::Csrf as Plugin>::new(c));\nbrocanth_server::serve(addr, app, router)\n",
    )
    .unwrap();

    let r = run(&dir, &["check", "--deploy"]);
    assert_eq!(r.code, 0, "only a warning is left:\n{}", r.stderr);
    assert!(r.stderr.contains("warning[deploy/hsts]"), "{}", r.stderr);

    let r = run(&dir, &["check", "--deploy", "--strict"]);
    assert_eq!(r.code, 1, "{}", r.stderr);

    let r = run(
        &dir,
        &["check", "--deploy", "--strict", "--allow", "deploy/hsts"],
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert!(r.stderr.contains("deploy checks pass"), "{}", r.stderr);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_dialect_that_disagrees_with_the_database_fails_in_every_environment() {
    let dir = project("dialect");
    std::fs::write(
        dir.join("config/development.toml"),
        "[model]\nurl = \"postgres://localhost/shop\"\n\n[policy]\ndialect = \"sqlite\"\n",
    )
    .unwrap();
    let r = run(&dir, &["check"]);
    assert_eq!(r.code, 1, "{}", r.stderr);
    assert!(r.stderr.contains("error[check/dialect]"), "{}", r.stderr);
    assert!(
        r.stderr.contains("--> config/development.toml:5"),
        "{}",
        r.stderr
    );
    assert!(
        r.stderr.contains("help: set `dialect = \"postgres\"`"),
        "{}",
        r.stderr
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
