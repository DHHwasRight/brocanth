//! `NULL`, case by case, each asserted rather than implied.
//!
//! PLAN.md §9's vulnerability class is a `NULL` that a framework resolved in
//! the permissive direction on the author's behalf. So every `NULL` case gets
//! a line here, including the ones whose answer is obvious, because the ones
//! whose answer is obvious are the ones nobody checks.

mod common;

use brocanth_policy::{Bound, EvalError, MapRow, Pred, Truth};
use common::PostColumn as C;

fn row() -> MapRow<C> {
    MapRow::new()
        .with(C::Id, 1i64)
        .with(C::AuthorId, 7i64)
        .with(C::Status, "published")
        .with_null(C::TeamId)
}

fn truth(pred: &Pred<C>) -> Truth {
    pred.eval(&row()).expect("evaluable")
}

#[test]
fn the_truth_tables_are_sqls() {
    use Truth::{False, True, Unknown};

    // AND
    assert_eq!(True.and(True), True);
    assert_eq!(True.and(Unknown), Unknown);
    assert_eq!(True.and(False), False);
    assert_eq!(Unknown.and(Unknown), Unknown);
    assert_eq!(Unknown.and(False), False);
    assert_eq!(False.and(False), False);

    // OR
    assert_eq!(True.or(True), True);
    assert_eq!(True.or(Unknown), True);
    assert_eq!(True.or(False), True);
    assert_eq!(Unknown.or(Unknown), Unknown);
    assert_eq!(Unknown.or(False), Unknown);
    assert_eq!(False.or(False), False);

    // NOT — the line that matters.
    assert_eq!(!True, False);
    assert_eq!(!False, True);
    assert_eq!(!Unknown, Unknown);
}

#[test]
fn the_collapse_happens_once_and_denies() {
    assert!(Truth::True.admits());
    assert!(!Truth::False.admits());
    assert!(!Truth::Unknown.admits(), "Unknown must not admit a row");
}

#[test]
fn a_comparison_with_a_null_column_is_unknown() {
    assert_eq!(truth(&Pred::eq(C::TeamId, 3i64)), Truth::Unknown);
    assert_eq!(truth(&Pred::ne(C::TeamId, 3i64)), Truth::Unknown);
    assert_eq!(truth(&Pred::lt(C::TeamId, 3i64)), Truth::Unknown);
    assert_eq!(truth(&Pred::ge(C::TeamId, 3i64)), Truth::Unknown);
}

#[test]
fn a_comparison_with_a_null_value_is_unknown() {
    let absent: Option<i64> = None;
    assert_eq!(
        truth(&Pred::eq(C::AuthorId, Bound::or_null(absent))),
        Truth::Unknown
    );
    assert_eq!(
        truth(&Pred::ne(C::AuthorId, Bound::or_null(absent))),
        Truth::Unknown
    );
}

#[test]
fn the_pocketbase_shape_denies_here() {
    // `@request.auth.rolee != "banned"` with an unresolvable left side.
    // PocketBase: COALESCE(NULL,'') IS NOT COALESCE('banned','') -> TRUE.
    // Here: Unknown, and Unknown does not admit.
    let unresolvable: Option<String> = None;
    let pred = Pred::ne(C::Status, Bound::or_null(unresolvable));
    assert_eq!(truth(&pred), Truth::Unknown);
    assert!(!pred.admits(&row()).expect("evaluable"));

    // And negating it does not rescue it, which is the whole repair.
    assert_eq!(truth(&Pred::negate(pred)), Truth::Unknown);
}

#[test]
fn null_safe_operators_are_two_valued() {
    // `IS` and `IS NOT` never answer Unknown. This is the author saying
    // "absence is a value like any other", out loud.
    assert_eq!(truth(&Pred::is(C::TeamId, 3i64)), Truth::False);
    assert_eq!(truth(&Pred::is_not(C::TeamId, 3i64)), Truth::True);
    assert_eq!(truth(&Pred::<C>::is_null(C::TeamId)), Truth::True);
    assert_eq!(truth(&Pred::<C>::is_not_null(C::TeamId)), Truth::False);
    assert_eq!(truth(&Pred::<C>::is_null(C::AuthorId)), Truth::False);
    assert_eq!(truth(&Pred::<C>::is_not_null(C::AuthorId)), Truth::True);

    let absent: Option<i64> = None;
    // NULL IS NULL is true, and only the null-safe operator says so.
    assert_eq!(
        truth(&Pred::is(C::TeamId, Bound::or_null(absent))),
        Truth::True
    );
    assert_eq!(
        truth(&Pred::eq(C::TeamId, Bound::or_null(absent))),
        Truth::Unknown
    );
}

#[test]
fn in_and_not_in_follow_sql() {
    use brocanth_policy::Value;

    // A NULL left side is unknown, whatever the set holds.
    assert_eq!(truth(&Pred::one_of(C::TeamId, [1i64, 2])), Truth::Unknown);

    // A miss against a set containing NULL is unknown, not false: the NULL
    // might have been the match.
    let with_null = Pred::In {
        column: C::AuthorId,
        values: vec![Value::Int(9), Value::Null],
        negated: false,
    };
    assert_eq!(truth(&with_null), Truth::Unknown);

    // A hit is true even so.
    let hit = Pred::In {
        column: C::AuthorId,
        values: vec![Value::Int(7), Value::Null],
        negated: false,
    };
    assert_eq!(truth(&hit), Truth::True);

    // `NOT IN` over a set containing a NULL matches nothing. This is SQL's
    // most-reported surprise and it is reproduced rather than smoothed over.
    let not_in = Pred::In {
        column: C::AuthorId,
        values: vec![Value::Int(9), Value::Null],
        negated: true,
    };
    assert_eq!(truth(&not_in), Truth::Unknown);
}

#[test]
fn unknown_propagates_through_and_or_and_not() {
    let unknown = Pred::eq(C::TeamId, 3i64);
    let known_true = Pred::eq(C::AuthorId, 7i64);
    let known_false = Pred::eq(C::AuthorId, 8i64);

    assert_eq!(
        truth(&Pred::all([unknown.clone(), known_true.clone()])),
        Truth::Unknown
    );
    assert_eq!(
        truth(&Pred::all([unknown.clone(), known_false.clone()])),
        Truth::False
    );
    assert_eq!(
        truth(&Pred::any([unknown.clone(), known_true])),
        Truth::True
    );
    assert_eq!(
        truth(&Pred::any([unknown.clone(), known_false])),
        Truth::Unknown
    );
    assert_eq!(truth(&Pred::negate(unknown)), Truth::Unknown);
}

#[test]
fn a_column_the_row_does_not_carry_reads_as_null() {
    // The safe direction: a row missing a column makes the comparison unknown,
    // which denies, rather than making it absent, which would admit.
    let sparse: MapRow<C> = MapRow::new().with(C::Id, 1i64);
    assert_eq!(
        Pred::eq(C::AuthorId, 7i64)
            .eval(&sparse)
            .expect("evaluable"),
        Truth::Unknown
    );
}

#[test]
fn a_predicate_about_another_table_has_no_in_memory_answer() {
    use brocanth_policy::{Membership, Op};
    use common::{MEMBERS, MEMBERS_TEAM, MEMBERS_USER};

    let pred = Membership::new(C::TeamId, MEMBERS, MEMBERS_TEAM)
        .filter_by(MEMBERS_USER, Op::Eq, 7i64)
        .expect("present")
        .into_pred();
    assert_eq!(
        pred.eval(&row()),
        Err(EvalError::NeedsDatabase {
            table: "team_members"
        })
    );
}

#[test]
fn an_opaque_fragment_without_an_evaluator_refuses_rather_than_guesses() {
    use brocanth_policy::Opaque;

    let pred: Pred<C> = Pred::Opaque(Opaque::new(
        "a window function",
        "score > ?",
        vec![1i64.into()],
    ));
    assert_eq!(
        pred.eval(&row()),
        Err(EvalError::Opaque {
            reason: "a window function"
        })
    );

    let with_eval: Pred<C> = Pred::Opaque(
        Opaque::new("a window function", "score > ?", vec![1i64.into()])
            .with_eval(|_row| Truth::True),
    );
    assert_eq!(with_eval.eval(&row()), Ok(Truth::True));
}

#[test]
fn an_absent_value_that_meant_deny_is_false_and_shows_it() {
    let anonymous: Option<i64> = None;

    let pred = Pred::eq(C::AuthorId, Bound::or_deny(anonymous));
    assert!(pred.is_never());
    assert_eq!(truth(&pred), Truth::False);

    let pred = Pred::eq(C::AuthorId, Bound::or_allow(anonymous));
    assert!(pred.is_always());
    assert_eq!(truth(&pred), Truth::True);

    // The PocketBase shape: a "deny" absence inside a negation grants. We do
    // the same thing, and unlike PocketBase we can be asked about it.
    let negated = Pred::negate(Pred::eq(C::AuthorId, Bound::or_deny(anonymous)));
    assert!(negated.is_always());
}
