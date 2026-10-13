//! A value never becomes SQL text.
//!
//! The claim is structural — [`brocanth_policy::sql`] has exactly one path
//! from a `Value` into a statement and it appends to the bind list — so this
//! file is the proof rather than the mechanism. It throws the usual payloads
//! at every predicate shape, in both dialects, and asserts that nothing the
//! attacker wrote appears in the generated text.

mod common;

use brocanth_policy::{Dialect, Membership, Op, Pred, Value};
use common::{Post, PostColumn as C, MEMBERS, MEMBERS_TEAM, MEMBERS_USER};

/// What a request body might carry.
const PAYLOADS: &[&str] = &[
    "'; DROP TABLE posts; --",
    "\" OR \"\"=\"",
    "1 OR 1=1",
    "admin'--",
    "'/**/UNION/**/SELECT/**/1--",
    "\\'; DELETE FROM posts WHERE '1'='1",
    "x\u{0}y",
];

/// Values that spell a placeholder. They cannot be tested by absence — the
/// generated SQL contains `?` because every bind does — so
/// [`a_value_that_looks_like_a_placeholder_changes_nothing`] tests them by
/// shape instead.
const PLACEHOLDER_LOOKALIKES: &[&str] = &["?", "$1", "?, ?", "$2"];

/// Every shape that can hold a value, built around one payload.
fn shapes(payload: &str) -> Vec<Pred<C>> {
    vec![
        Pred::eq(C::Status, payload),
        Pred::ne(C::Status, payload),
        Pred::is(C::Status, payload),
        Pred::is_not(C::Status, payload),
        Pred::lt(C::Status, payload),
        Pred::one_of(C::Status, [payload, payload]),
        Pred::none_of(C::Status, [payload]),
        Pred::negate(Pred::eq(C::Status, payload)),
        Pred::all([Pred::eq(C::Status, payload), Pred::eq(C::Score, 1i64)]),
        Pred::any([Pred::eq(C::Status, payload), Pred::eq(C::Score, 1i64)]),
        Membership::new(C::TeamId, MEMBERS, MEMBERS_TEAM)
            .filter_by(MEMBERS_USER, Op::Eq, payload)
            .expect("present")
            .into_pred(),
    ]
}

#[test]
fn no_payload_reaches_the_sql_text() {
    for payload in PAYLOADS {
        for dialect in [Dialect::Sqlite, Dialect::Postgres] {
            for pred in shapes(payload) {
                let condition = pred.condition::<Post>(dialect).expect("compiles");
                assert!(
                    !condition.sql().contains(payload),
                    "payload {payload:?} appeared in {:?} SQL: {}",
                    dialect,
                    condition.sql()
                );
                assert!(
                    condition
                        .binds()
                        .iter()
                        .any(|v| matches!(v, Value::Text(t) if t == payload)),
                    "payload {payload:?} did not reach the bind list for {}",
                    condition.sql()
                );
            }
        }
    }
}

#[test]
fn no_payload_reaches_a_whole_statement() {
    for payload in PAYLOADS {
        for dialect in [Dialect::Sqlite, Dialect::Postgres] {
            let pred = Pred::eq(C::Status, *payload);

            let listing = pred.matching_ids::<Post>(dialect).expect("compiles");
            assert!(!listing.sql().contains(payload));

            let admission = pred.admission::<Post>(dialect, *payload).expect("compiles");
            assert!(!admission.query().sql().contains(payload));

            let write = admission
                .resolve(1)
                .expect("one row")
                .update(dialect, [(C::Status, Value::Text((*payload).to_owned()))])
                .expect("compiles");
            assert!(
                !write.query().sql().contains(payload),
                "payload in UPDATE: {}",
                write.query().sql()
            );
        }
    }
}

#[test]
fn the_generated_text_holds_no_quote_at_all() {
    // A stronger statement than "the payload is absent": nothing this crate
    // writes is ever a string literal, so a single quote in the output would
    // itself be the bug.
    for payload in PAYLOADS {
        for pred in shapes(payload) {
            for dialect in [Dialect::Sqlite, Dialect::Postgres] {
                let sql = pred
                    .condition::<Post>(dialect)
                    .expect("compiles")
                    .sql()
                    .to_owned();
                assert!(!sql.contains('\''), "a quote appeared in: {sql}");
            }
        }
    }
}

#[test]
fn a_value_that_looks_like_a_placeholder_changes_nothing() {
    // A bind is a bind. A value that spells `?` or `$1` cannot add, remove or
    // renumber a placeholder, because the text is written from the shape and
    // the value is never read while writing it.
    let benign = Pred::all([Pred::eq(C::Status, "ok"), Pred::eq(C::Score, 1i64)]);
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let expected = benign.condition::<Post>(dialect).expect("compiles");
        for payload in PLACEHOLDER_LOOKALIKES {
            let pred = Pred::all([Pred::eq(C::Status, *payload), Pred::eq(C::Score, 1i64)]);
            let condition = pred.condition::<Post>(dialect).expect("compiles");
            assert_eq!(condition.sql(), expected.sql());
            assert_eq!(condition.binds().len(), 2);
            assert_eq!(condition.binds()[0], Value::Text((*payload).to_owned()));
        }
    }
}

#[test]
fn an_identifier_that_is_not_an_identifier_is_refused() {
    use brocanth_policy::{CompileError, Ident};

    assert!(matches!(
        Ident::new("posts; DROP TABLE users"),
        Err(CompileError::BadIdentifier { .. })
    ));
    assert!(matches!(
        Ident::new("a\"b"),
        Err(CompileError::BadIdentifier { .. })
    ));
    assert!(matches!(Ident::new(""), Err(CompileError::EmptyIdentifier)));
    assert!(Ident::new("author_id").is_ok());
    assert!(Ident::new("_x9").is_ok());
    // A leading digit is not an identifier, which is also what every dialect
    // says.
    assert!(Ident::new("9lives").is_err());
}

#[test]
fn a_column_whose_name_is_not_an_identifier_fails_the_query() {
    // The last line of defence: even a hand-written `Column` impl that returns
    // something unspeakable cannot get it into a statement.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct Hostile;

    impl brocanth_policy::Column for Hostile {
        const ALL: &'static [Self] = &[Hostile];
        fn name(&self) -> &'static str {
            "x\" FROM posts; DROP TABLE posts; --"
        }
    }

    struct Bad;
    impl brocanth_policy::Entity for Bad {
        type Column = Hostile;
        const TABLE: brocanth_policy::Ident = brocanth_policy::Ident::must("posts");
        const PRIMARY_KEY: Self::Column = Hostile;
    }

    let pred = Pred::eq(Hostile, 1i64);
    assert!(matches!(
        pred.condition::<Bad>(Dialect::Sqlite),
        Err(brocanth_policy::CompileError::BadIdentifier { .. })
    ));
}

#[test]
fn an_opaque_fragment_may_not_carry_a_literal() {
    use brocanth_policy::{CompileError, Opaque};

    // The escape hatch obeys the same rule: every value binds. A fragment with
    // a literal is refused, which also keeps the `?` count honest.
    let pred: Pred<C> = Pred::Opaque(Opaque::new(
        "a hand-written fragment",
        "status = 'published'",
        vec![],
    ));
    assert!(matches!(
        pred.condition::<Post>(Dialect::Sqlite),
        Err(CompileError::LiteralInFragment { .. })
    ));

    let pred: Pred<C> = Pred::Opaque(Opaque::new(
        "mismatched",
        "score > ? AND score < ?",
        vec![Value::Int(1)],
    ));
    assert!(matches!(
        pred.condition::<Post>(Dialect::Sqlite),
        Err(CompileError::PlaceholderMismatch {
            placeholders: 2,
            binds: 1,
            ..
        })
    ));

    let pred: Pred<C> = Pred::Opaque(Opaque::new(
        "wrong syntax",
        "score > $1",
        vec![Value::Int(1)],
    ));
    assert!(matches!(
        pred.condition::<Post>(Dialect::Sqlite),
        Err(CompileError::PlaceholderSyntax { .. })
    ));
}
