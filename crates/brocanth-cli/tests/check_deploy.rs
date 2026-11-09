//! `brocanth check --deploy` refuses a development-only module
//! (`cache-memory`), and each escape hatch lets it through.
//!
//! The project here has no `commands` module, so the CLI does not delegate
//! to a project `check`: the run stays in milliseconds and tests only the
//! CLI's own checks.

use std::path::{Path, PathBuf};
use std::process::Command;

const BROCANTH: &str = env!("CARGO_BIN_EXE_brocanth");
const FINDING: &str = "deploy/development-only";

struct Run {
    out: String,
    code: i32,
}

fn check(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(BROCANTH)
        .arg("-C")
        .arg(dir)
        .arg("check")
        .args(args)
        .env_remove("NO_COLOR")
        .env_remove("CLICOLOR_FORCE")
        .output()
        .expect("run brocanth");
    Run {
        out: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stderr),
            String::from_utf8_lossy(&out.stdout)
        ),
        code: out.status.code().unwrap_or(-1),
    }
}

/// A project marker with these modules, and nothing else.
fn project(name: &str, modules: &[&str]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "brocanth-check-deploy-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let list: Vec<String> = modules.iter().map(|m| format!("{m:?}")).collect();
    std::fs::write(
        dir.join("brocanth.toml"),
        format!("[modules]\nenabled = [{}]\n", list.join(", ")),
    )
    .unwrap();
    dir
}

#[test]
fn a_production_deploy_refuses_cache_memory_and_says_what_to_do() {
    let dir = project("refused", &["config", "cache", "cache-memory"]);
    for args in [
        &["--deploy"][..],
        &["--deploy", "--env", "production"],
        &["--deploy", "--env", "staging"],
    ] {
        let run = check(&dir, args);
        assert_eq!(
            run.code, 1,
            "{args:?}: a development module fails the deploy\n{}",
            run.out
        );
        assert!(run.out.contains(FINDING), "{args:?}\n{}", run.out);
        assert!(run.out.contains("`cache-memory`"), "{args:?}\n{}", run.out);
        assert!(
            run.out.contains("help:") && run.out.contains("brocanth remove cache-memory"),
            "{args:?}: the help names the command\n{}",
            run.out
        );
        assert!(
            run.out.contains("--allow deploy/development-only"),
            "{args:?}: the help names the escape hatch\n{}",
            run.out
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_escape_hatch_lets_cache_memory_through() {
    let dir = project("allowed", &["config", "cache", "cache-memory"]);
    for args in [
        &["--deploy", "--env", "development"][..],
        &["--deploy", "--env", "test"],
        &["--deploy", "--allow", FINDING],
        &["--deploy", "--env", "staging", "--allow", FINDING],
        // Without --deploy the production checks do not run at all.
        &[],
    ] {
        let run = check(&dir, args);
        assert_eq!(run.code, 0, "{args:?}\n{}", run.out);
        assert!(!run.out.contains(FINDING), "{args:?}\n{}", run.out);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn allowing_another_finding_does_not_let_cache_memory_through() {
    let dir = project("other", &["config", "cache", "cache-memory"]);
    let run = check(&dir, &["--deploy", "--allow", "deploy/hsts"]);
    assert_eq!(run.code, 1, "{}", run.out);
    assert!(run.out.contains(FINDING), "{}", run.out);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_postgres_and_redis_backends_pass_a_production_deploy() {
    for (name, modules) in [
        ("pg", &["config", "cache"][..]),
        ("redis", &["config", "cache", "cache-redis"]),
    ] {
        let dir = project(name, modules);
        let run = check(&dir, &["--deploy"]);
        assert!(!run.out.contains(FINDING), "{modules:?}\n{}", run.out);
        assert_eq!(run.code, 0, "{modules:?}\n{}", run.out);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
