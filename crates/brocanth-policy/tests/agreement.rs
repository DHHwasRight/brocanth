//! `eval(row) == (row in query(condition))`, over a table of hand-written
//! cases and a real SQLite database.
//!
//! PLAN.md §9 obligation 1 says the two interpretations can disagree and
//! PocketBase cannot have that bug because it has only one. This is the test
//! that would find it, and it is written against the engine rather than
//! against a second guess of what the engine does.

mod common;
#[path = "common/sqlite.rs"]
mod sqlite;

use brocanth_policy::agreement::{check, HarnessError};
use brocanth_policy::{Dialect, MapRow, Pred, Value};
use common::{Post, PostColumn as C, SCHEMA};

/// Four rows that between them cover a NULL in every nullable column.
fn fixture() -> (sqlite::Sqlite, Vec<MapRow<C>>) {
    let db = sqlite::Sqlite::new(SCHEMA);
    let rows = vec![
        row(1, Some(7), Some(3), Some("published"), Some(10)),
        row(2, Some(7), None, Some("draft"), Some(5)),
        row(3, Some(8), Some(3), None, Some(10)),
        row(4, None, None, Some("published"), None),
    ];
    for r in &rows {
        db.execute(
            "INSERT INTO posts (id, author_id, team_id, status, score) VALUES (?, ?, ?, ?, ?)",
            &[
                cell(r, C::Id),
                cell(r, C::AuthorId),
                cell(r, C::TeamId),
                cell(r, C::Status),
                cell(r, C::Score),
            ],
        );
    }
    (db, rows)
}

fn cell(row: &MapRow<C>, column: C) -> Value {
    use brocanth_policy::RowView as _;
    row.get(column)
}

fn row(
    id: i64,
    author: Option<i64>,
    team: Option<i64>,
    status: Option<&str>,
    score: Option<i64>,
) -> MapRow<C> {
    let mut out = MapRow::new().with(C::Id, id);
    out = match author {
        Some(v) => out.with(C::AuthorId, v),
        None => out.with_null(C::AuthorId),
    };
    out = match team {
        Some(v) => out.with(C::TeamId, v),
        None => out.with_null(C::TeamId),
    };
    out = match status {
        Some(v) => out.with(C::Status, v),
        None => out.with_null(C::Status),
    };
    match score {
        Some(v) => out.with(C::Score, v),
        None => out.with_null(C::Score),
    }
}

/// Every shape with an in-memory interpretation, including the NULL cases that
/// are the reason this test exists.
fn cases() -> Vec<(&'static str, Pred<C>)> {
    vec![
        ("always", Pred::Always),
        ("never", Pred::Never),
        ("eq", Pred::eq(C::AuthorId, 7i64)),
        ("eq against a null column", Pred::eq(C::TeamId, 3i64)),
        ("ne", Pred::ne(C::Status, "draft")),
        ("ne against a null column", Pred::ne(C::Status, "published")),
        ("lt", Pred::lt(C::Score, 10i64)),
        ("le", Pred::le(C::Score, 10i64)),
        ("gt", Pred::gt(C::Score, 5i64)),
        ("ge", Pred::ge(C::Score, 10i64)),
        ("is null", Pred::is_null(C::TeamId)),
        ("is not null", Pred::is_not_null(C::TeamId)),
        ("null-safe eq", Pred::is(C::TeamId, 3i64)),
        ("null-safe ne", Pred::is_not(C::TeamId, 3i64)),
        ("in", Pred::one_of(C::Status, ["published", "draft"])),
        ("not in", Pred::none_of(C::Status, ["draft"])),
        (
            "in with a null in the set",
            Pred::In {
                column: C::Status,
                values: vec![Value::Text("draft".into()), Value::Null],
                negated: false,
            },
        ),
        (
            "not in with a null in the set",
            Pred::In {
                column: C::Status,
                values: vec![Value::Text("draft".into()), Value::Null],
                negated: true,
            },
        ),
        (
            "and",
            Pred::all([Pred::eq(C::AuthorId, 7i64), Pred::eq(C::TeamId, 3i64)]),
        ),
        (
            "or",
            Pred::any([
                Pred::eq(C::Status, "published"),
                Pred::eq(C::AuthorId, 7i64),
            ]),
        ),
        ("not", Pred::negate(Pred::eq(C::Status, "draft"))),
        (
            "not over an unknown",
            Pred::negate(Pred::eq(C::TeamId, 3i64)),
        ),
        (
            "a realistic policy: published, or mine",
            Pred::any([
                Pred::eq(C::Status, "published"),
                Pred::eq(C::AuthorId, 7i64),
            ]),
        ),
        (
            "nested, with a null on every arm",
            Pred::all([
                Pred::any([Pred::eq(C::TeamId, 3i64), Pred::is_null(C::TeamId)]),
                Pred::negate(Pred::eq(C::Status, "draft")),
            ]),
        ),
    ]
}

#[test]
fn the_two_interpretations_agree() {
    let (mut db, rows) = fixture();
    for (name, pred) in cases() {
        match check::<Post, _, _>(&pred, &rows, Dialect::Sqlite, &mut db) {
            Ok(()) => {}
            Err(HarnessError::Disagreed(d)) => panic!(
                "case `{name}` disagreed: eval() said {:?}, the query said {}, for row {}\n  {}",
                d.evaluated, d.selected, d.id, d.sql
            ),
            Err(other) => panic!("case `{name}` could not be checked: {other}"),
        }
    }
}

#[test]
fn the_harness_reports_a_disagreement_rather_than_passing() {
    // The harness has to be able to fail, or it proves nothing. A predicate
    // whose two interpretations are deliberately different must be caught.
    use brocanth_policy::{Opaque, Truth};

    let (mut db, rows) = fixture();
    let lying: Pred<C> = Pred::Opaque(
        Opaque::new(
            "deliberately inconsistent",
            "status = ?",
            vec![Value::Text("draft".into())],
        )
        .with_eval(|_row| Truth::True),
    );
    assert!(matches!(
        check::<Post, _, _>(&lying, &rows, Dialect::Sqlite, &mut db),
        Err(HarnessError::Disagreed(_))
    ));
}

#[test]
fn a_predicate_with_only_one_interpretation_is_not_a_disagreement() {
    use brocanth_policy::{Membership, Op};
    use common::{MEMBERS, MEMBERS_TEAM, MEMBERS_USER};

    let (mut db, rows) = fixture();
    let pred = Membership::new(C::TeamId, MEMBERS, MEMBERS_TEAM)
        .filter_by(MEMBERS_USER, Op::Eq, 7i64)
        .expect("present")
        .into_pred();
    assert!(matches!(
        check::<Post, _, _>(&pred, &rows, Dialect::Sqlite, &mut db),
        Err(HarnessError::NotEvaluable(_))
    ));
}

#[test]
fn the_admission_query_and_the_list_query_select_the_same_rows() {
    // The default §9 asks for is one interpretation used everywhere. This is
    // the assertion that the per-row guard and the list filter are the same
    // question, which is the property PocketBase gets for free by having no
    // evaluator at all.
    use brocanth_policy::RowView as _;

    let (mut db, rows) = fixture();
    for (name, pred) in cases() {
        let listed = {
            let query = pred
                .matching_ids::<Post>(Dialect::Sqlite)
                .expect("compiles");
            Oracle::<Post>::matching(&mut db, &query).expect("the query runs")
        };
        for row in &rows {
            let id = row.get(C::Id);
            let admission = pred
                .admission::<Post>(Dialect::Sqlite, id.clone())
                .expect("compiles");
            let matched = {
                // `SELECT 1 …` returns a 1, which the oracle reads as an id.
                // Only the row count matters here.
                Oracle::<Post>::matching(&mut db, admission.query())
                    .expect("the query runs")
                    .len()
            };
            assert_eq!(
                matched > 0,
                listed.contains(&id),
                "case `{name}` disagreed about row {id} between the list and the guard"
            );
        }
    }
}

use brocanth_policy::agreement::Oracle;
