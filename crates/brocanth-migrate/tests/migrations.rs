//! The generator end to end: model source in, migration source out, and that
//! source compiled and run against SQLite.
//!
//! The text is checked against `tests/golden/`; `BROCANTH_BLESS=1` rewrites
//! it. The same files are `include!`d below, so a golden file that does not
//! compile, or that does not do what its comments say, fails here too.

use std::path::Path;

use brocanth_migrate::render::render;
use brocanth_migrate::scan::{scan_source, schema_of};
use brocanth_migrate::schema::Field;
use brocanth_migrate::{diff, Change, Schema};

fn schema(fixture: &str) -> Schema {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    let text = std::fs::read_to_string(&path).expect("fixture");
    schema_of(&scan_source(fixture, &text).expect("scan")).expect("ids")
}

/// v1 to v3: v3 is v2 with `post.body` stored as `text`, a type change.
fn v3() -> Schema {
    let mut s = schema("v2.rs");
    let post = s.tables.iter_mut().find(|t| t.name == "post").unwrap();
    let body: &mut Field = post.fields.iter_mut().find(|f| f.name == "body").unwrap();
    body.kind = "text".into();
    s
}

fn migrations() -> Vec<(&'static str, String)> {
    let (v1, v2) = (schema("v1.rs"), schema("v2.rs"));
    vec![
        (
            "m0001_initial",
            render("m0001_initial", &diff(&Schema::default(), &v1)),
        ),
        ("m0002_renames", render("m0002_renames", &diff(&v1, &v2))),
        (
            "m0003_body_text",
            render("m0003_body_text", &diff(&v2, &v3())),
        ),
    ]
}

#[test]
fn the_renames_are_renames() {
    let changes = diff(&schema("v1.rs"), &schema("v2.rs"));
    let said: Vec<String> = changes.iter().map(Change::describe).collect();
    assert_eq!(
        said,
        [
            "rename table `user` to `account`",
            "rename column `post.title` to `headline`",
            "add column `post.body` (string)",
            "drop column `post.views`",
        ]
    );
}

#[test]
fn the_generator_writes_what_it_wrote_yesterday() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let bless = std::env::var_os("BROCANTH_BLESS").is_some();
    for (name, have) in migrations() {
        let file = dir.join(format!("{name}.rs"));
        if bless {
            std::fs::write(&file, &have).expect("write golden");
            continue;
        }
        let want = std::fs::read_to_string(&file).expect("golden; run with BROCANTH_BLESS=1");
        assert_eq!(
            want, have,
            "{name} changed; run with BROCANTH_BLESS=1 if that is right"
        );
    }
}

mod m0001 {
    include!("golden/m0001_initial.rs");
}
mod m0002 {
    include!("golden/m0002_renames.rs");
}
// Compiled, not run: SQLite cannot alter a column.
#[allow(dead_code)]
mod m0003 {
    include!("golden/m0003_body_text.rs");
}

#[tokio::test]
async fn the_migrations_run_up_and_down_on_sqlite() {
    use brocanth_model::sea_orm::Database;
    use brocanth_model::sea_orm_migration::prelude::*;

    let db = Database::connect("sqlite::memory:").await.expect("sqlite");
    let manager = SchemaManager::new(&db);
    let has = |table: &'static str, column: &'static str| {
        let manager = &manager;
        async move { manager.has_column(table, column).await.unwrap() }
    };

    assert_eq!(m0001::Migration.name(), "m0001_initial");
    m0001::Migration.up(&manager).await.expect("initial up");
    assert!(has("user", "email").await);
    assert!(has("post", "title").await);
    assert!(has("post", "views").await);

    m0002::Migration.up(&manager).await.expect("renames up");
    assert!(manager.has_table("account").await.unwrap());
    assert!(!manager.has_table("user").await.unwrap());
    assert!(has("post", "headline").await);
    assert!(!has("post", "title").await);
    assert!(has("post", "body").await);
    assert!(!has("post", "views").await);

    m0002::Migration.down(&manager).await.expect("renames down");
    assert!(manager.has_table("user").await.unwrap());
    assert!(has("post", "title").await);
    assert!(has("post", "views").await);
    assert!(!has("post", "body").await);

    m0001::Migration.down(&manager).await.expect("initial down");
    assert!(!manager.has_table("user").await.unwrap());
    assert!(!manager.has_table("post").await.unwrap());
}
