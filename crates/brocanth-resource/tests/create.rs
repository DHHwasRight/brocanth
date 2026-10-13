//! The create path: the synthesised one-row admission test, in both dialects
//! and against a real engine.

mod common;

use brocanth_policy::{Bound, Dialect, Entity, Pred, Value};
use brocanth_resource::sql::create_admission;
use brocanth_resource::{flow, Failure};
use common::{Post, PostColumn, Posts, Visitor};

#[test]
fn the_sqlite_admission_puts_the_cte_binds_first_because_sqlite_counts_by_position() {
    let pred: brocanth_policy::EntityPred<Post> =
        Pred::eq(PostColumn::AuthorId, Bound::of(Value::Int(7)));
    let condition = pred
        .condition::<Post>(Dialect::Sqlite)
        .expect("it compiles");

    let statement = create_admission::<Post>(
        Dialect::Sqlite,
        &condition,
        &[
            (PostColumn::AuthorId, Value::Int(7)),
            (PostColumn::Title, Value::Text("hello".to_owned())),
        ],
    )
    .expect("it compiles");

    assert_eq!(
        statement.sql(),
        "WITH \"posts\" AS (SELECT NULL AS \"id\", ? AS \"author_id\", NULL AS \"status\", \
         ? AS \"title\") SELECT 1 FROM \"posts\" WHERE (\"posts\".\"author_id\" = ?) LIMIT 1"
    );
    // The CTE is written first, so its values bind first.
    assert_eq!(
        statement.binds(),
        &[
            Value::Int(7),
            Value::Text("hello".to_owned()),
            Value::Int(7)
        ]
    );
}

#[test]
fn the_postgres_admission_continues_the_conditions_numbering_and_casts_every_bind() {
    let pred: brocanth_policy::EntityPred<Post> =
        Pred::eq(PostColumn::AuthorId, Bound::of(Value::Int(7)));
    let condition = pred
        .condition::<Post>(Dialect::Postgres)
        .expect("it compiles");

    let statement = create_admission::<Post>(
        Dialect::Postgres,
        &condition,
        &[
            (PostColumn::AuthorId, Value::Int(7)),
            (PostColumn::Title, Value::Text("hello".to_owned())),
        ],
    )
    .expect("it compiles");

    // `$1` is the condition's, so the CTE starts at `$2`. A parameter in a
    // `SELECT` list has nothing to infer a type from, so each one says.
    assert_eq!(
        statement.sql(),
        "WITH \"posts\" AS (SELECT NULL AS \"id\", $2::bigint AS \"author_id\", \
         NULL AS \"status\", $3::text AS \"title\") SELECT 1 FROM \"posts\" \
         WHERE (\"posts\".\"author_id\" = $1) LIMIT 1"
    );
    assert_eq!(
        statement.binds(),
        &[
            Value::Int(7),
            Value::Int(7),
            Value::Text("hello".to_owned())
        ]
    );
}

#[test]
fn a_condition_compiled_for_another_dialect_is_refused_rather_than_retargeted() {
    let pred: brocanth_policy::EntityPred<Post> = Pred::eq(PostColumn::AuthorId, 7i64);
    let condition = pred
        .condition::<Post>(Dialect::Sqlite)
        .expect("it compiles");

    let refused = create_admission::<Post>(Dialect::Postgres, &condition, &[]);
    assert!(refused.is_err());
}

#[test]
fn the_admission_runs_against_a_real_engine_and_admits_only_your_own_row() {
    let posts = Posts::new();

    let created = flow::create(&posts, &Visitor::user(7), b"author_id=7&title=mine")
        .expect("the policy admits a row whose author is the subject");
    assert_eq!(created.id, Value::Int(1));
    assert_eq!(created.row.title, "mine");

    // The same request with somebody else's author id. The predicate is
    // evaluated by the database against the row that was proposed, so it never
    // reaches the table.
    let refused = flow::create(&posts, &Visitor::user(7), b"author_id=8&title=theirs");
    assert!(matches!(refused, Err(Failure::NoMatch)), "{refused:?}");

    let rows = posts
        .db
        .execute("DELETE FROM posts WHERE title = 'theirs'", &[]);
    assert_eq!(rows, 0, "and nothing was written");
}

#[test]
fn an_anonymous_create_is_a_refusal_before_the_body_is_read() {
    let posts = Posts::new();

    let refused = flow::create(&posts, &Visitor::anonymous(), b"author_id=7&title=x");

    assert!(matches!(refused, Err(Failure::NoRule)));
    assert_eq!(
        brocanth_resource::status::outcome(
            brocanth_resource::status::Action::Create,
            &Failure::NoRule
        )
        .status,
        403,
        "a shut door is a 403; a body the rule refuses is a 400"
    );
}

#[test]
fn the_mask_applies_to_a_create_as_well() {
    let posts = Posts::new();

    // The author may set the title and the author column, never the id.
    let refused = flow::create(&posts, &Visitor::user(7), b"id=1&author_id=7&title=x");
    assert!(
        matches!(refused, Err(Failure::NotWritable(_))),
        "{refused:?}"
    );
}

#[test]
fn a_create_the_policy_refuses_is_a_400_and_a_create_nobody_may_make_is_a_403() {
    // The two are different answers because they hide different things, and
    // neither hides a record: a create has no record to hide yet.
    assert_eq!(
        brocanth_resource::status::outcome(
            brocanth_resource::status::Action::Create,
            &Failure::NoMatch
        )
        .status,
        400
    );
    assert_eq!(
        brocanth_resource::status::outcome(
            brocanth_resource::status::Action::Create,
            &Failure::NoRule
        )
        .status,
        403
    );
}

#[test]
fn the_table_the_cte_shadows_is_the_entitys_own() {
    assert_eq!(<Post as Entity>::TABLE.as_str(), "posts");
}

#[test]
fn the_in_memory_interpretation_answers_the_same_question_as_the_database_one() {
    use common::InMemoryPosts;

    let posts = InMemoryPosts::new();

    let created = flow::create(&posts, &Visitor::user(7), b"author_id=7&title=mine")
        .expect("the predicate admits the proposed row");
    assert_eq!(created.row.title, "mine");

    let refused = flow::create(&posts, &Visitor::user(7), b"author_id=8&title=theirs");
    assert!(matches!(refused, Err(Failure::NoMatch)), "{refused:?}");

    // And it asked no database: the admission is the only query a create
    // sends, and this interpretation does not send it.
    assert!(posts.0.db.seen.lock().unwrap().is_empty());
}

#[test]
fn a_predicate_the_in_memory_check_cannot_decide_is_loud_rather_than_quiet() {
    // `EvalError` reaches the client as a 500, not as a denial. A quiet denial
    // would be a resource that refuses every create for a reason nobody sees.
    let failure = Failure::Eval(brocanth_policy::EvalError::NeedsDatabase {
        table: "team_members",
    });
    let outcome =
        brocanth_resource::status::outcome(brocanth_resource::status::Action::Create, &failure);
    assert_eq!(outcome.status, 500);
    assert!(
        outcome.internal,
        "and it goes to the application's error hooks"
    );
}
