//! The `migrate` tool, run as a binary over a project on disk.
//!
//! The project is the smallest one the tool reads: a `brocanth.toml` and
//! one model file. No build and no database: `ids` and `make` read text and
//! write text. These tests ran against `brocanth migrate` before the tool
//! left the CLI; the assertions are the same.

use std::path::{Path, PathBuf};
use std::process::Command;

const TOOL: &str = env!("CARGO_BIN_EXE_brocanth-migrate");

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
    let out = Command::new(TOOL)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run the migrate tool");
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
    let dir = std::env::temp_dir().join(format!(
        "brocanth-migrate-tool-{name}-{}",
        std::process::id()
    ));
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
    files.retain(|p| p.extension().is_some_and(|e| e == "rs") && !p.ends_with("mod.rs"));
    files.sort();
    files
}

/// `migrations/mod.rs`, the list `make` keeps.
fn migrator(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("migrations/mod.rs")).expect("make writes the list")
}

#[test]
fn make_refuses_a_model_with_no_ids_and_names_each_one() {
    let dir = project("refuse");
    let r = run(&dir, &["make", "create_posts"]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(
        r.out.contains(
            "Model `Model` (table `post`) has no id. Run `migrate ids`. \
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

    let r = run(&dir, &["ids"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(r.out.contains("wrote 3 ids"), "{}", r.out);
    let stamped = std::fs::read_to_string(&model).unwrap();
    assert!(stamped.starts_with("use brocanth_model::prelude::*;\n\n#[brocanth_model::ids]\n"));
    assert_eq!(
        stamped.matches("#[brocanth(id = \"").count(),
        3,
        "{stamped}"
    );

    let r = run(&dir, &["ids"]);
    assert!(r.out.contains("No file is changed."), "{}", r.out);
    assert_eq!(std::fs::read_to_string(&model).unwrap(), stamped);

    let r = run(&dir, &["make", "create_posts"]);
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
    let list = migrator(&dir);
    assert!(list.contains(&format!("mod {name};")), "{list}");
    assert!(
        list.contains(&format!("Box::new({name}::Migration)")),
        "{list}"
    );

    let r = run(&dir, &["make", "nothing"]);
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
    let r = run(&dir, &["make", "rename_title"]);
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
    // Both files, oldest first: the list is the directory, sorted.
    let list = migrator(&dir);
    let stems: Vec<String> = all
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    let (first, second) = (
        list.find(&format!("Box::new({}::Migration)", stems[0])),
        list.find(&format!("Box::new({}::Migration)", stems[1])),
    );
    assert!(first.is_some() && first < second, "{list}");
    let snapshot = std::fs::read_to_string(dir.join("migrations/schema.json")).unwrap();
    assert!(snapshot.contains("\"headline\"") && !snapshot.contains("\"title\""));
}

#[test]
fn make_refuses_a_name_that_is_not_snake_case() {
    let dir = project("name");
    let r = run(&dir, &["make", "Rename-Title"]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(
        r.out.contains("The name `Rename-Title` is not snake case."),
        "{}",
        r.out
    );
}

#[test]
fn an_edited_list_is_not_rewritten_and_the_missing_file_is_named() {
    let dir = project("edited");
    assert_eq!(run(&dir, &["ids"]).code, 0);
    let r = run(&dir, &["make", "create_posts"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let list = dir.join("migrations/mod.rs");
    let edited = format!("{}// mine\n", migrator(&dir));
    std::fs::write(&list, &edited).unwrap();

    std::fs::write(
        dir.join("migrations/m20990101_000000_by_hand.rs"),
        "// written by hand\n",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(
        dir.join("src/models/post.rs"),
        std::fs::read_to_string(dir.join("src/models/post.rs"))
            .unwrap()
            .replace("pub title: String", "pub headline: String"),
    )
    .unwrap();
    let r = run(&dir, &["make", "rename_title"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(
        r.out.contains("was edited, so it was not rewritten"),
        "{}",
        r.out
    );
    assert!(r.out.contains("m20990101_000000_by_hand"), "{}", r.out);
    assert_eq!(std::fs::read_to_string(&list).unwrap(), edited);
}

#[test]
fn the_tool_runs_from_a_subdirectory_and_refuses_outside_a_project() {
    let dir = project("root");
    let r = run(&dir.join("src/models"), &["ids"]);
    assert_eq!(r.code, 0, "{}", r.out);
    assert!(r.out.contains("wrote 3 ids"), "{}", r.out);

    let outside =
        std::env::temp_dir().join(format!("brocanth-migrate-none-{}", std::process::id()));
    std::fs::create_dir_all(&outside).unwrap();
    let r = run(&outside, &["ids"]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(r.out.contains("error[migrate/no-project]"), "{}", r.out);
    assert!(r.out.contains("help: "), "{}", r.out);
}

#[test]
fn sync_lists_a_file_written_by_hand() {
    let dir = project("sync");
    std::fs::create_dir_all(dir.join("migrations")).unwrap();
    std::fs::write(
        dir.join("migrations/m20990101_000000_by_hand.rs"),
        "// written by hand\n",
    )
    .unwrap();
    let r = run(&dir, &["sync"]);
    assert_eq!(r.code, 0, "{}", r.out);
    let list = migrator(&dir);
    assert!(list.contains("mod m20990101_000000_by_hand;"), "{list}");
}

#[test]
fn up_is_the_applications_and_the_tool_says_so() {
    let dir = project("up");
    let r = run(&dir, &["up"]);
    assert_eq!(r.code, 1, "{}", r.out);
    assert!(
        r.out.contains("application's own `migrate` command"),
        "{}",
        r.out
    );
}
