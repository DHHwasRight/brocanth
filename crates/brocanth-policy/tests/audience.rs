//! Inverting a predicate into an audience, which PLAN.md §10 says is the
//! reason the predicate is a value at all.

mod common;

use brocanth_policy::{Audience, Bound, Membership, Op, Opaque, Pred, Value};
use common::{PostColumn as C, MEMBERS, MEMBERS_TEAM, MEMBERS_USER};

#[test]
fn the_four_coarse_partitions_section_ten_names_all_invert() {
    // "public"
    assert_eq!(Pred::<C>::Always.audience(), Ok(Audience::Everyone));

    // "any authenticated subject" — the policy's own answer for an anonymous
    // visitor, which is nobody, and for a signed-in one, which is a key.
    let anonymous: Option<i64> = None;
    assert_eq!(
        Pred::eq(C::AuthorId, Bound::or_deny(anonymous)).audience(),
        Ok(Audience::NoOne)
    );

    // "the owner"
    assert_eq!(
        Pred::eq(C::AuthorId, 7i64).audience(),
        Ok(Audience::Keyed {
            column: C::AuthorId,
            value: Value::Int(7)
        })
    );

    // "members of a team"
    let membership = Membership::new(C::TeamId, MEMBERS, MEMBERS_TEAM)
        .filter_by(MEMBERS_USER, Op::Eq, 7i64)
        .expect("present")
        .into_pred();
    assert_eq!(
        membership.audience(),
        Ok(Audience::Membership {
            local: C::TeamId,
            table: MEMBERS,
            foreign: MEMBERS_TEAM,
            filter: vec![(MEMBERS_USER, Value::Int(7))],
        })
    );
}

#[test]
fn the_single_key_case_has_a_fast_path() {
    let audience = Pred::eq(C::AuthorId, 7i64).audience().expect("invertible");
    assert_eq!(audience.key(), Some((C::AuthorId, &Value::Int(7))));
    assert_eq!(audience.columns(), vec![C::AuthorId]);
    assert!(!audience.is_everyone());
    assert!(!audience.is_no_one());
}

#[test]
fn a_realistic_policy_inverts_to_the_columns_realtime_must_read() {
    // "published, or mine" — the shape most applications actually write.
    let pred = Pred::any([
        Pred::eq(C::Status, "published"),
        Pred::eq(C::AuthorId, 7i64),
    ]);
    let audience = pred.audience().expect("invertible");
    assert_eq!(
        audience,
        Audience::AnyOf(vec![
            Audience::Keyed {
                column: C::Status,
                value: Value::Text("published".into())
            },
            Audience::Keyed {
                column: C::AuthorId,
                value: Value::Int(7)
            },
        ])
    );
    // One read per column off the changed row, then two map lookups. No query.
    assert_eq!(audience.columns(), vec![C::Status, C::AuthorId]);
}

#[test]
fn a_set_inverts_to_one_key_per_member() {
    let audience = Pred::one_of(C::Status, ["published", "featured"])
        .audience()
        .expect("invertible");
    assert_eq!(
        audience,
        Audience::AnyOf(vec![
            Audience::Keyed {
                column: C::Status,
                value: Value::Text("published".into())
            },
            Audience::Keyed {
                column: C::Status,
                value: Value::Text("featured".into())
            },
        ])
    );
}

#[test]
fn a_comparison_against_null_admits_nobody() {
    let absent: Option<i64> = None;
    assert_eq!(
        Pred::eq(C::AuthorId, Bound::or_null(absent)).audience(),
        Ok(Audience::NoOne)
    );
}

#[test]
fn what_cannot_be_inverted_says_why() {
    // Each of these falls back to per-subscriber evaluation, which is correct
    // and slow. The reason is what tells an operator which policy costs them.
    let cases: Vec<Pred<C>> = vec![
        Pred::negate(Pred::eq(C::Status, "spam")),
        Pred::ne(C::Status, "spam"),
        Pred::gt(C::Score, 10i64),
        Pred::none_of(C::Status, ["spam"]),
        Membership::new(C::TeamId, MEMBERS, MEMBERS_TEAM)
            .filter_by(MEMBERS_USER, Op::Eq, 7i64)
            .expect("present")
            .negate()
            .into_pred(),
        Pred::Opaque(Opaque::new(
            "a window function",
            "score > ?",
            vec![Value::Int(1)],
        )),
    ];
    for pred in cases {
        let reason = pred.audience().expect_err("not invertible").reason;
        assert!(!reason.is_empty(), "a fallback must say why");
    }

    // An opaque fragment reports the author's own words.
    let opaque: Pred<C> = Pred::Opaque(Opaque::new(
        "a window function",
        "score > ?",
        vec![Value::Int(1)],
    ));
    assert_eq!(
        opaque.audience().expect_err("not invertible").reason,
        "a window function"
    );
    assert!(opaque.has_opaque());
    assert!(Pred::all([Pred::<C>::Always, opaque]).has_opaque());
}

#[test]
fn an_uninvertible_part_makes_the_whole_uninvertible() {
    // Conservative on purpose: the inversion says yes only where the answer is
    // certain, and "cannot invert" already has a defined meaning, so a later
    // version can invert more shapes without any caller changing.
    let pred = Pred::all([Pred::eq(C::AuthorId, 7i64), Pred::gt(C::Score, 10i64)]);
    assert!(pred.audience().is_err());
}

#[test]
fn the_shape_summary_matches_the_variants() {
    use brocanth_policy::Shape;

    assert!(matches!(Pred::<C>::Always.shape(), Shape::Everyone));
    assert!(matches!(Pred::<C>::Never.shape(), Shape::NoOne));
    assert!(matches!(
        Pred::eq(C::AuthorId, 7i64).shape(),
        Shape::Column {
            column: C::AuthorId,
            op: Op::Eq,
            ..
        }
    ));
    assert!(matches!(
        Pred::one_of(C::Status, ["a"]).shape(),
        Shape::Set { negated: false, .. }
    ));
    assert!(matches!(
        Pred::all([Pred::<C>::eq(C::Score, 1i64), Pred::eq(C::Score, 2i64)]).shape(),
        Shape::All(_)
    ));
    assert!(matches!(
        Pred::any([Pred::<C>::eq(C::Score, 1i64), Pred::eq(C::Score, 2i64)]).shape(),
        Shape::Any(_)
    ));
    assert!(matches!(
        Pred::negate(Pred::<C>::eq(C::Score, 1i64)).shape(),
        Shape::Negated(_)
    ));
}
