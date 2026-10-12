//! The eager-load correctness fix.
//!
//! The first test here proves the naive path is wrong. It is written first on
//! purpose: a silent wrong answer deserves a test that would have caught it.

#[path = "../fixtures/blog.rs"]
mod blog;

use blog::{migration::Migrator, post, user};
use brocanth_model::prelude::*;
use brocanth_model::testing::Harness;
use brocanth_model::{EagerStrategy, Probe};
use sea_orm::QuerySelect;

/// What SeaORM does on its own, and why it is wrong.
///
/// Ten users, three posts each. `LIMIT 10` on a `LEFT JOIN` counts joined rows,
/// not parents, so ten of the thirty rows come back — and they belong to four
/// users, not ten. No error, no warning. The wrong number.
#[tokio::test]
async fn the_naive_limited_eager_load_returns_the_wrong_number_of_parents() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 10, 3).await.expect("seed");

    let rows = user::Entity::find()
        .limit(10)
        .find_with_related(post::Entity)
        .all(&*db)
        .await
        .expect("query");

    assert_ne!(
        rows.len(),
        10,
        "if SeaORM ever starts limiting parents, delete this test and the two-query path"
    );
    assert_eq!(rows.len(), 4, "ten joined rows span four of the ten users");
    assert_eq!(
        rows.iter().map(|(_, posts)| posts.len()).sum::<usize>(),
        10,
        "the LIMIT counted joined rows"
    );
}

/// What brocanth does instead.
#[tokio::test]
async fn a_limited_eager_load_returns_the_right_number_of_parents() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 10, 3).await.expect("seed");

    let query = user::Entity::find().eager().limit(10);
    assert_eq!(query.strategy(), EagerStrategy::TwoQuery);

    let rows = query
        .with_related::<post::Entity, _>(&*db)
        .await
        .expect("query");

    assert_eq!(rows.len(), 10, "ten users");
    for row in &rows {
        assert_eq!(row.related().len(), 3, "each with all three posts");
    }
}

/// An offset is the same bug with a different name, so it takes the same path.
///
/// SQLite will not accept a bare `OFFSET`, so the executed half of this test
/// pages properly; the bare case is checked on the strategy alone.
#[tokio::test]
async fn an_offset_eager_load_also_takes_the_two_query_path() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 5, 2).await.expect("seed");

    assert_eq!(
        user::Entity::find().eager().offset(2).strategy(),
        EagerStrategy::TwoQuery
    );

    let query = user::Entity::find().eager().limit(2).offset(2);
    assert_eq!(query.strategy(), EagerStrategy::TwoQuery);

    let rows = query
        .with_related::<post::Entity, _>(&*db)
        .await
        .expect("query");
    assert_eq!(rows.len(), 2, "page two of five users");
    assert!(rows.iter().all(|r| r.related().len() == 2));
}

/// An unbounded eager load keeps the single join: one statement, not two.
#[tokio::test]
async fn an_unbounded_eager_load_is_one_query() {
    let probe = Probe::default();
    let harness = Harness::sqlite_with(probe.clone()).await.expect("connect");
    harness.migrate::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 4, 3).await.expect("seed");

    let query = user::Entity::find().eager();
    assert_eq!(query.strategy(), EagerStrategy::Join);

    let unit = probe.unit_of_work("unbounded eager load");
    let rows = query
        .with_related::<post::Entity, _>(&*db)
        .await
        .expect("query");
    let report = unit.finish();

    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|r| r.related().len() == 3));
    assert_eq!(
        report.statements, 1,
        "one LEFT JOIN, not one query per user"
    );
    assert!(!report.has_n_plus_one());
}

/// The limited path costs exactly two statements, never one per parent.
#[tokio::test]
async fn a_limited_eager_load_is_two_queries_not_n_plus_one() {
    let probe = Probe::default();
    let harness = Harness::sqlite_with(probe.clone()).await.expect("connect");
    harness.migrate::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 10, 3).await.expect("seed");

    let unit = probe.unit_of_work("limited eager load");
    let rows = user::Entity::find()
        .eager()
        .limit(10)
        .with_related::<post::Entity, _>(&*db)
        .await
        .expect("query");
    let report = unit.finish();

    assert_eq!(rows.len(), 10);
    assert_eq!(report.statements, 2, "parents, then children by key");
    assert!(!report.has_n_plus_one());
}

/// A relation nobody loaded is loud, not quiet.
#[tokio::test]
async fn an_unloaded_relation_refuses_to_be_read() {
    let harness = Harness::migrated::<Migrator>().await.expect("migrate");
    let db = harness.begin().await.expect("begin");
    blog::seed::blog(&*db, 2, 1).await.expect("seed");

    let rows = user::Entity::find()
        .eager()
        .without_related::<post::Entity, _>(&*db)
        .await
        .expect("query");

    assert_eq!(rows.len(), 2);
    assert!(!rows[0].is_loaded());
    let err = rows[0].try_related().expect_err("should refuse");
    assert!(err.to_string().contains("never loaded"));
}
