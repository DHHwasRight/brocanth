//! Fixtures: a known dataset from a file, checked against the entities and
//! written through the model layer, inside the test harness's transaction.

#[path = "../fixtures/blog.rs"]
mod blog;

use blog::{migration::Migrator, post, user};
use brocanth_model::prelude::*;
use brocanth_model::testing::Harness;
use brocanth_model::Fixtures;

fn fixtures() -> Fixtures {
    Fixtures::new()
        .dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data"))
        .model::<user::Entity>()
        .model::<post::Entity>()
}

#[tokio::test]
async fn a_fixture_loads_into_the_test_transaction_and_goes_away_with_it() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    {
        let txn = harness.begin().await.expect("begin");
        let loaded = fixtures().load("blog", &*txn).await.expect("load");
        assert_eq!(loaded.rows(), 4);
        assert_eq!(
            loaded.per_model,
            vec![("user".to_owned(), 2), ("post".to_owned(), 2)]
        );

        let ada = user::Entity::find_by_id(1)
            .one(&*txn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ada.email, "ada@example.com");
        let post = post::Entity::find_by_id(2)
            .one(&*txn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((post.user_id, post.status.as_str()), (2, "draft"));
    }
    // The test transaction dropped and rolled back, savepoint and all.
    let left = user::Entity::find().all(harness.conn()).await.unwrap();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn an_unknown_column_is_located_and_nothing_is_written() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let text = r#"[
  { "model": "user", "fields": { "id": 1, "name": "Ada", "email": "a@example.com" } },
  { "model": "user", "fields": { "id": 2, "name": "Bob", "emial": "b@example.com" } }
]"#;
    let error = fixtures()
        .load_str("fixtures/two.json", text, harness.conn())
        .await
        .expect_err("emial is not a column");
    assert_eq!(error.problems.len(), 1, "{error}");
    let p = &error.problems[0];
    assert_eq!((p.line, p.column), (3, 58), "{error}");
    assert!(
        p.message
            .contains("record 2 (`user`) has no column `emial`"),
        "{error}"
    );
    assert!(p.help.contains("did you mean `email`?"), "{error}");
    assert!(p.help.contains("id, name, email"), "{error}");
    assert!(
        error.to_string().starts_with("fixtures/two.json:3:58: "),
        "{error}"
    );

    // The check runs before any write, so the good first record is not kept.
    assert!(user::Entity::find()
        .all(harness.conn())
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn every_problem_in_the_file_is_reported() {
    let text = r#"[
  { "model": "usr", "fields": {} },
  { "model": "post", "pk": 3, "fields": { "views": "many" } },
  { "model": "user", "fields": { "name": null } }
]"#;
    let error = fixtures()
        .check("f.json", text)
        .expect_err("three records, four problems");
    let messages: Vec<String> = error.problems.iter().map(ToString::to_string).collect();
    assert_eq!(error.problems.len(), 4, "{messages:#?}");

    assert!(messages[0].contains("f.json:2:"), "{}", messages[0]);
    assert!(
        messages[0].contains("unknown model `usr`"),
        "{}",
        messages[0]
    );
    assert!(
        messages[0].contains("did you mean `user`?"),
        "{}",
        messages[0]
    );

    assert!(messages[1].contains("unknown key `pk`"), "{}", messages[1]);
    assert!(
        messages[1].contains("put the primary key in `fields`"),
        "{}",
        messages[1]
    );

    assert!(
        messages[2].contains("`views` = \"many\""),
        "{}",
        messages[2]
    );
    assert!(messages[2].contains("integer"), "{}", messages[2]);

    assert!(
        messages[3].contains("`name` is not nullable"),
        "{}",
        messages[3]
    );
}

#[tokio::test]
async fn a_row_the_model_refuses_is_refused_here_too_and_the_file_is_not_half_loaded() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let txn = harness.begin().await.expect("begin");
    let text = r#"[
  { "model": "user", "fields": { "id": 1, "name": "Ada", "email": "a@example.com" } },
  { "model": "user", "fields": { "id": 2, "name": "Bob", "email": "not an address" } }
]"#;
    let error = fixtures()
        .load_str("seed.json", text, &*txn)
        .await
        .expect_err("validation refuses the second user");
    let p = &error.problems[0];
    assert_eq!(p.line, 3, "{error}");
    assert!(p.message.contains("fails validation"), "{error}");
    assert!(
        p.message.contains("Email is not an email address"),
        "{error}"
    );

    let kept = user::Entity::find().all(&*txn).await.unwrap();
    assert!(kept.is_empty(), "one transaction for the file: {kept:?}");
}

#[tokio::test]
async fn a_missing_fixture_names_the_ones_that_exist() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let error = fixtures()
        .load("blgo", harness.conn())
        .await
        .expect_err("no such file");
    let p = &error.problems[0];
    assert!(
        p.message.contains("cannot read the fixture `blgo`"),
        "{error}"
    );
    assert!(p.help.contains("blog"), "{error}");
}

#[tokio::test]
async fn a_file_that_is_not_json_says_where() {
    let error = fixtures()
        .check("bad.json", "[\n  { \"model\": \"user\", }\n]")
        .expect_err("trailing comma");
    let p = &error.problems[0];
    assert_eq!(p.line, 2, "{error}");
    assert!(p.help.contains("\"model\""), "{error}");
}
