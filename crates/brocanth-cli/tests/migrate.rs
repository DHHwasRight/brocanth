//! `brocanth migrate`, which `brocanth` no longer defines.
//!
//! The commands moved into the project the `model` module writes: `up`,
//! `down` and `status` in its binary, `ids`, `make` and `sync` in its
//! `migrate` tool. `brocanth migrate …` still works, through the hand-off
//! every project command takes. The tool's own tests are in
//! `crates/brocanth-migrate/tests/tool.rs`.

use std::path::{Path, PathBuf};
use std::process::Command;

const BROCANTH: &str = env!("CARGO_BIN_EXE_brocanth");

const POST: &str = r#"use brocanth_model::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "post")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub title: String,
}
"#;

struct Run {
    out: String,
    code: i32,
}

fn run(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(BROCANTH)
        .arg("-C")
        .arg(dir)
        .args(args)
        .env_remove("CLICOLOR_FORCE")
        .env("NO_COLOR", "1")
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

fn migrations(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("migrations"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e == "rs") && !p.ends_with("mod.rs"));
    files.sort();
    files
}

#[test]
fn migrate_is_not_a_command_of_brocanth() {
    let out = Command::new(BROCANTH)
        .arg("--help")
        .env("NO_COLOR", "1")
        .output()
        .expect("run brocanth");
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(
        !help.lines().any(|l| l.trim_start().starts_with("migrate ")),
        "`migrate` belongs to a project with `model`, not to brocanth:\n{help}"
    );
}

#[test]
fn migrate_outside_a_project_with_a_command_line_says_what_to_add() {
    let dir = std::env::temp_dir().join(format!("brocanth-migrate-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("brocanth.toml"),
        "[project]\nname = \"t\"\n\n[modules]\nenabled = [\"logging\"]\n",
    )
    .unwrap();
    let r = run(&dir, &["migrate", "up"]);
    assert_ne!(r.code, 0, "{}", r.out);
    assert!(r.out.contains("help: "), "{}", r.out);
}

/// The whole loop in a generated project, through `brocanth`: `ids` and
/// `make` run the project's `migrate` tool, and `up`, `status` and `down`
/// run in its binary against `[db] url`.
///
/// Needs a Postgres in `BROCANTH_TEST_DATABASE_URL`, because the `db`
/// module is Postgres. `cargo test -p brocanth-cli -- --ignored`.
#[test]
#[ignore = "runs cargo build over a generated application and needs Postgres"]
fn ids_make_up_status_and_down_run_in_the_project() {
    let Some(url) = std::env::var("BROCANTH_TEST_DATABASE_URL")
        .ok()
        .filter(|u| !u.trim().is_empty())
    else {
        eprintln!("SKIPPED: BROCANTH_TEST_DATABASE_URL is not set");
        return;
    };
    let base = std::env::temp_dir().join(format!("brocanth-migrate-up-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let brocanth = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("brocanth")
        .canonicalize()
        .unwrap();
    let r = run(
        &base,
        &[
            "create",
            "shop",
            "--with",
            "config,commands,db,model",
            "--no-git",
            "--brocanth-path",
            &brocanth.display().to_string(),
        ],
    );
    assert_eq!(r.code, 0, "{}", r.out);
    let dir = base.join("shop");
    std::fs::create_dir_all(dir.join("src/models")).unwrap();
    std::fs::write(dir.join("src/models/post.rs"), POST).unwrap();
    let with_db = |args: &[&str]| {
        let out = Command::new(BROCANTH)
            .arg("-C")
            .arg(&dir)
            .args(args)
            .env("NO_COLOR", "1")
            .env("SHOP_DB__URL", &url)
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
    };
    let r = with_db(&["migrate", "ids"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let r = with_db(&["migrate", "make", "create_posts"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let name = migrations(&dir)[0]
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    let r = with_db(&["migrate", "status"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(r.out.contains(&format!("pending      {name}")), "{}", r.out);

    let r = with_db(&["migrate", "up"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(r.out.contains(&format!("applied      {name}")), "{}", r.out);

    let r = with_db(&["migrate", "down"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(r.out.contains(&format!("rolled back  {name}")), "{}", r.out);

    // A file written by hand and not listed is refused, not skipped.
    std::fs::write(
        dir.join("migrations/m20990101_000000_by_hand.rs"),
        "// written by hand\n",
    )
    .unwrap();
    let r = with_db(&["migrate", "up"]);
    assert_ne!(r.code, 0, "{}", r.out);
    assert!(r.out.contains("m20990101_000000_by_hand"), "{}", r.out);
    assert!(r.out.contains("migrate sync"), "{}", r.out);
    let _ = std::fs::remove_dir_all(&base);
}
