//! `brocanth migrate`, run as a binary over a project on disk.
//!
//! The project is the smallest one the command reads: a `brocanth.toml` and
//! one model file. No build and no database: `ids` and `make` read text and
//! write text.

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

fn project(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brocanth-migrate-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/models")).unwrap();
    std::fs::write(
        dir.join("brocanth.toml"),
        "[project]\nname = \"t\"\n\n[modules]\nenabled = [\"model\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/models/post.rs"), POST).unwrap();
    dir
}

fn migrations(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("migrations"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|p| p.extension().is_some_and(|e| e == "rs"));
    files.sort();
    files
}

#[test]
fn make_refuses_a_model_with_no_ids_and_names_each_one() {
    let dir = project("refuse");
    let r = run(&dir, &["migrate", "make", "create_posts"]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(
        r.out.contains(
            "Model `Model` (table `post`) has no id. Run `brocanth migrate ids`. \
             Then run this command again."
        ),
        "{}",
        r.out
    );
    assert!(r.out.contains("Field `post.title` has no id."), "{}", r.out);
    assert!(r.out.contains("--> src/models/post.rs:9:9"), "{}", r.out);
    assert!(
        migrations(&dir).is_empty(),
        "nothing is written on a refusal"
    );
}

#[test]
fn ids_then_make_then_a_rename_is_a_rename() {
    let dir = project("rename");
    let model = dir.join("src/models/post.rs");

    let r = run(&dir, &["migrate", "ids"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(r.out.contains("wrote 3 ids"), "{}", r.out);
    let stamped = std::fs::read_to_string(&model).unwrap();
    assert!(stamped.starts_with("use brocanth_model::prelude::*;\n\n#[brocanth_model::ids]\n"));
    assert_eq!(
        stamped.matches("#[brocanth(id = \"").count(),
        3,
        "{stamped}"
    );

    let r = run(&dir, &["migrate", "ids"]);
    assert!(r.out.contains("No file is changed."), "{}", r.out);
    assert_eq!(std::fs::read_to_string(&model).unwrap(), stamped);

    let r = run(&dir, &["migrate", "make", "create_posts"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let first = migrations(&dir);
    assert_eq!(first.len(), 1);
    let name = first[0].file_stem().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with('m') && name.ends_with("_create_posts"),
        "{name}"
    );
    let text = std::fs::read_to_string(&first[0]).unwrap();
    assert!(text.contains(".create_table("), "{text}");
    assert!(
        text.contains(&format!("\"{name}\"")),
        "the name is the file stem:\n{text}"
    );
    assert!(dir.join("migrations/schema.json").is_file());

    let r = run(&dir, &["migrate", "make", "nothing"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        r.out.contains("No change since the last migration."),
        "{}",
        r.out
    );
    assert_eq!(migrations(&dir).len(), 1);

    // The developer renames the field and leaves its id where it is.
    std::fs::write(
        &model,
        stamped.replace("pub title: String", "pub headline: String"),
    )
    .unwrap();
    // One second apart, so the two files cannot share a time stamp.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let r = run(&dir, &["migrate", "make", "rename_title"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        r.out.contains("rename column `post.title` to `headline`"),
        "{}",
        r.out
    );
    let all = migrations(&dir);
    assert_eq!(all.len(), 2);
    let text = std::fs::read_to_string(&all[1]).unwrap();
    assert!(
        text.contains(".rename_column(Alias::new(\"title\"), Alias::new(\"headline\"))"),
        "{text}"
    );
    assert!(
        !text.contains("drop_column"),
        "a rename is not a drop:\n{text}"
    );
    let snapshot = std::fs::read_to_string(dir.join("migrations/schema.json")).unwrap();
    assert!(snapshot.contains("\"headline\"") && !snapshot.contains("\"title\""));
}

#[test]
fn make_refuses_a_name_that_is_not_snake_case() {
    let dir = project("name");
    let r = run(&dir, &["migrate", "make", "Rename-Title"]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(
        r.out.contains("The name `Rename-Title` is not snake case."),
        "{}",
        r.out
    );
}
