//! The SQL each predicate shape compiles to, written out.
//!
//! A snapshot rather than a behaviour test, and the point is review: a change
//! to the compiler shows up here as a diff a reader can judge, next to the
//! shape that produced it. PostgreSQL's numbered placeholders make bind order
//! load-bearing, so the snapshots also pin the numbering.

mod common;

use brocanth_policy::{Bound, Dialect, Membership, Op, Opaque, Pred, Value};
use common::{Post, PostColumn as C, MEMBERS, MEMBERS_TEAM, MEMBERS_USER};

const PG: Dialect = Dialect::Postgres;

fn sql(pred: &Pred<C>) -> String {
    pred.condition::<Post>(PG)
        .expect("compiles")
        .sql()
        .to_owned()
}

#[test]
fn constants_are_real_booleans() {
    assert_eq!(sql(&Pred::Always), "TRUE");
    assert_eq!(sql(&Pred::Never), "FALSE");
}

#[test]
fn the_default_dialect_is_postgres() {
    // Brocanth supports PostgreSQL only. The default is never what a compile
    // entry point reaches for, because every one of them takes the argument.
    assert_eq!(Dialect::default(), Dialect::Postgres);
}

#[test]
fn comparison() {
    let pred = Pred::eq(C::AuthorId, 7i64);
    assert_eq!(sql(&pred), r#""posts"."author_id" = $1"#);

    let pred = Pred::ne(C::Status, "draft");
    assert_eq!(sql(&pred), r#""posts"."status" <> $1"#);

    let pred = Pred::ge(C::Score, 10i64);
    assert_eq!(sql(&pred), r#""posts"."score" >= $1"#);
}

#[test]
fn null_safe_comparison_uses_is_distinct_from() {
    let pred = Pred::is(C::TeamId, 3i64);
    assert_eq!(sql(&pred), r#""posts"."team_id" IS NOT DISTINCT FROM $1"#);

    let pred = Pred::is_not(C::TeamId, 3i64);
    assert_eq!(sql(&pred), r#""posts"."team_id" IS DISTINCT FROM $1"#);
}

#[test]
fn null_tests_bind_nothing() {
    // `IS NULL` rather than `IS NOT DISTINCT FROM $1` with a NULL bind: no
    // untyped parameter for PostgreSQL to puzzle over.
    let pred = Pred::<C>::is_null(C::TeamId);
    assert_eq!(sql(&pred), r#""posts"."team_id" IS NULL"#);
    assert!(pred.condition::<Post>(PG).unwrap().binds().is_empty());

    let pred = Pred::<C>::is_not_null(C::TeamId);
    assert_eq!(sql(&pred), r#""posts"."team_id" IS NOT NULL"#);
}

#[test]
fn sets() {
    let pred = Pred::one_of(C::Status, ["draft", "published"]);
    assert_eq!(sql(&pred), r#""posts"."status" IN ($1, $2)"#);

    let pred = Pred::none_of(C::Status, ["spam"]);
    assert_eq!(sql(&pred), r#""posts"."status" NOT IN ($1)"#);

    // An empty set folds: `IN ()` is a syntax error everywhere, and the empty
    // disjunction is false.
    assert!(Pred::<C>::one_of(C::Status, Vec::<&str>::new()).is_never());
    assert!(Pred::<C>::none_of(C::Status, Vec::<&str>::new()).is_always());
}

#[test]
fn conjunction_disjunction_negation() {
    let pred = Pred::all([Pred::eq(C::Status, "published"), Pred::eq(C::TeamId, 3i64)]);
    assert_eq!(
        sql(&pred),
        r#"("posts"."status" = $1) AND ("posts"."team_id" = $2)"#
    );

    let pred = Pred::any([
        Pred::eq(C::Status, "published"),
        Pred::eq(C::AuthorId, 7i64),
    ]);
    assert_eq!(
        sql(&pred),
        r#"("posts"."status" = $1) OR ("posts"."author_id" = $2)"#
    );

    let pred = Pred::negate(Pred::eq(C::Status, "spam"));
    assert_eq!(sql(&pred), r#"NOT ("posts"."status" = $1)"#);

    // The numbering runs across the whole statement, not per term.
    let pred = Pred::all([
        Pred::eq(C::Status, "published"),
        Pred::any([Pred::eq(C::TeamId, 3i64), Pred::eq(C::AuthorId, 7i64)]),
    ]);
    assert_eq!(
        sql(&pred),
        r#"("posts"."status" = $1) AND (("posts"."team_id" = $2) OR ("posts"."author_id" = $3))"#
    );
}

#[test]
fn membership() {
    let pred = Membership::new(C::TeamId, MEMBERS, MEMBERS_TEAM)
        .filter_by(MEMBERS_USER, Op::Eq, 7i64)
        .expect("a present subject")
        .into_pred();
    assert_eq!(
        sql(&pred),
        r#"EXISTS (SELECT 1 FROM "team_members" WHERE "team_members"."team_id" = "posts"."team_id" AND "team_members"."user_id" = $1)"#
    );

    let pred = Membership::new(C::TeamId, MEMBERS, MEMBERS_TEAM)
        .filter_by(MEMBERS_USER, Op::Eq, 7i64)
        .expect("a present subject")
        .negate()
        .into_pred();
    assert!(sql(&pred).starts_with("NOT EXISTS (SELECT 1 FROM"));
}

#[test]
fn opaque_fragment_is_parenthesised_and_renumbered() {
    let pred: Pred<C> = Pred::Opaque(Opaque::new(
        "a window function the algebra has no shape for",
        "posts.score > (SELECT AVG(score) FROM posts WHERE team_id = ?)",
        vec![Value::Int(3)],
    ));
    assert_eq!(
        sql(&pred),
        "(posts.score > (SELECT AVG(score) FROM posts WHERE team_id = $1))"
    );
}

#[test]
fn list_query() {
    let pred = Pred::eq(C::AuthorId, 7i64);
    let query = pred.matching_ids::<Post>(PG).expect("compiles");
    assert_eq!(
        query.sql(),
        r#"SELECT "posts"."id" FROM "posts" WHERE "posts"."author_id" = $1"#
    );
}

#[test]
fn admission_query_is_pocketbases_can_access_record() {
    let pred = Pred::eq(C::AuthorId, 7i64);
    let admission = pred.admission::<Post>(PG, 42i64).expect("compiles");
    assert_eq!(
        admission.query().sql(),
        r#"SELECT 1 FROM "posts" WHERE "posts"."id" = $1 AND ("posts"."author_id" = $2) LIMIT 1"#
    );
    // The id binds first, then the condition's own values, in order.
    assert_eq!(
        admission.query().binds(),
        [Value::Int(42), Value::Int(7)].as_slice()
    );
}

#[test]
fn write_statements_carry_the_condition() {
    let pred = Pred::eq(C::AuthorId, 7i64);
    let verified = pred
        .admission::<Post>(PG, 42i64)
        .expect("compiles")
        .resolve(1)
        .expect("one row");
    let write = verified
        .update(PG, [(C::Status, Value::Text("published".into()))])
        .expect("compiles");
    assert_eq!(
        write.query().sql(),
        r#"UPDATE "posts" SET "status" = $1 WHERE "posts"."id" = $2 AND ("posts"."author_id" = $3)"#
    );
    assert_eq!(
        write.query().binds(),
        [
            Value::Text("published".into()),
            Value::Int(42),
            Value::Int(7)
        ]
        .as_slice()
    );

    let verified = pred
        .admission::<Post>(PG, 42i64)
        .expect("compiles")
        .resolve(1)
        .expect("one row");
    let write = verified.delete(PG).expect("compiles");
    assert_eq!(
        write.query().sql(),
        r#"DELETE FROM "posts" WHERE "posts"."id" = $1 AND ("posts"."author_id" = $2)"#
    );
}

#[test]
fn postgres_renumbers_a_spliced_condition() {
    // The condition compiled on its own starts at $1. Spliced after the id
    // bind it has to start at $2.
    let pred = Pred::all([Pred::eq(C::AuthorId, 7i64), Pred::eq(C::TeamId, 3i64)]);
    let verified = pred
        .admission::<Post>(PG, 42i64)
        .expect("compiles")
        .resolve(1)
        .expect("one row");
    let write = verified
        .update(PG, [(C::Status, Value::Text("x".into()))])
        .expect("compiles");
    assert_eq!(
        write.query().sql(),
        r#"UPDATE "posts" SET "status" = $1 WHERE "posts"."id" = $2 AND (("posts"."author_id" = $3) AND ("posts"."team_id" = $4))"#
    );
    assert_eq!(
        write.query().binds(),
        [
            Value::Text("x".into()),
            Value::Int(42),
            Value::Int(7),
            Value::Int(3)
        ]
        .as_slice()
    );
}

#[test]
fn an_absent_value_shows_in_the_sql_as_a_constant() {
    let anonymous: Option<i64> = None;
    let pred = Pred::eq(C::AuthorId, Bound::or_deny(anonymous));
    assert_eq!(sql(&pred), "FALSE");

    let pred = Pred::eq(C::AuthorId, Bound::or_allow(anonymous));
    assert_eq!(sql(&pred), "TRUE");

    // `or_null` is the only one that keeps a comparison, and it is the only
    // one whose answer is Unknown rather than a constant.
    let pred = Pred::eq(C::AuthorId, Bound::or_null(anonymous));
    assert_eq!(sql(&pred), r#""posts"."author_id" = $1"#);
    assert_eq!(
        pred.condition::<Post>(PG).unwrap().binds(),
        [Value::Null].as_slice()
    );
}
