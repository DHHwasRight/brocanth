//! `brocanth dbshell --print`, run as a binary over a project on disk.
//!
//! `--print` is the whole test surface: without it the command replaces
//! itself with `psql` or `sqlite3`, and a test cannot assume either is
//! installed.

use std::path::{Path, PathBuf};
use std::process::Command;

const BROCANTH: &str = env!("CARGO_BIN_EXE_brocanth");

struct Run {
    stdout: String,
    stderr: String,
    code: i32,
}

fn run(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
    let mut command = Command::new(BROCANTH);
    command
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("CLICOLOR_FORCE")
        .env_remove("SHOP_ENV")
        .env_remove("SHOP_MODEL__URL")
        .env("NO_COLOR", "1");
    for (k, v) in env {
        command.env(k, v);
    }
    let out = command.output().expect("run brocanth");
    Run {
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        code: out.status.code().unwrap_or(-1),
    }
}

fn project(name: &str, modules: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brocanth-dbshell-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config")).unwrap();
    std::fs::create_dir_all(dir.join("db")).unwrap();
    std::fs::write(
        dir.join("brocanth.toml"),
        format!("[project]\nname = \"shop\"\n\n[modules]\nenabled = [{modules}]\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("config/development.toml"),
        "[model]\nurl = \"sqlite://db/development.sqlite?mode=rwc\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("db/development.sqlite"), b"").unwrap();
    dir
}

#[test]
fn development_opens_the_sqlite_file_under_the_project() {
    let dir = project("sqlite", "\"model\"");
    let r = run(&dir, &["dbshell", "--print"], &[]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let file = dir.join("db/development.sqlite");
    assert_eq!(r.stdout.trim(), format!("sqlite3 {}", file.display()));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_variable_wins_over_the_file_and_the_password_stays_off_the_line() {
    let dir = project("pg", "\"model\"");
    let r = run(
        &dir,
        &["dbshell", "--print", "--", "-c", "select 1"],
        &[("SHOP_MODEL__URL", "postgres://shop:hunter2@db:5432/shop")],
    );
    assert_eq!(r.code, 0, "{}", r.stderr);
    assert_eq!(
        r.stdout.trim(),
        "PGPASSWORD=... psql postgres://shop@db:5432/shop -c 'select 1'"
    );
    assert!(!r.stdout.contains("hunter2"));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_environment_with_no_url_says_where_to_put_one() {
    let dir = project("prod", "\"model\"");
    let r = run(&dir, &["dbshell", "--env", "production", "--print"], &[]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("error[dbshell/no-url]"), "{}", r.stderr);
    assert!(
        r.stderr.contains(
            "help: set `url` under `[model]` in config/production.toml, or set $SHOP_MODEL__URL"
        ),
        "{}",
        r.stderr
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_project_with_no_model_layer_is_told_how_to_get_one() {
    let dir = project("nomodel", "\"logging\"");
    let r = run(&dir, &["dbshell", "--print"], &[]);
    assert_eq!(r.code, 1);
    assert!(r.stderr.contains("error[dbshell/no-model]"), "{}", r.stderr);
    assert!(
        r.stderr.contains("help: `brocanth add model`"),
        "{}",
        r.stderr
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
