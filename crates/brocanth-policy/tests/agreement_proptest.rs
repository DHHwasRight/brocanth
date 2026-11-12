//! The property-test half of PLAN.md §9 obligation 1, over generated
//! predicates.
//!
//! The table in `agreement.rs` covers the cases somebody thought of. This
//! covers the ones nobody did: random predicates over random rows, checked
//! against PostgreSQL. It is the shape the `policy!` macro of PLAN.md §12
//! item 10 emits for an application that opts into `eval()`, which is why the
//! strategies live in the library rather than here.

#![cfg(feature = "agreement-proptest")]

mod common;
#[path = "common/postgres.rs"]
mod postgres;

use brocanth_policy::agreement::{check, strategies, HarnessError};
use brocanth_policy::{Dialect, MapRow, Pred, RowView as _, Value};
use common::{Post, PostColumn as C, SCHEMA};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestCaseError, TestRunner};

/// The integer columns the generator may touch. Not the primary key: the
/// harness joins on it, so it has to stay a distinct integer per row.
const INTS: &[C] = &[C::AuthorId, C::TeamId, C::Score];
/// The text columns.
const TEXTS: &[C] = &[C::Status];

/// Small pools, chosen so collisions are common: the interesting rows are the
/// ones that nearly match.
///
/// One pool per column type. PostgreSQL refuses `text = bigint` with a type
/// error, so a comparison across types is not a question it can answer.
fn ints() -> Vec<Value> {
    vec![Value::Null, Value::Int(1), Value::Int(2)]
}

fn texts() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Text("published".into()),
        Value::Text("draft".into()),
    ]
}

/// A predicate whose every comparison matches its column's type, with `AND`,
/// `OR` and `NOT` across the two types.
fn pred() -> impl Strategy<Value = Pred<C>> {
    prop_oneof![
        strategies::pred(INTS, ints()),
        strategies::pred(TEXTS, texts()),
    ]
    .prop_recursive(2, 16, 3, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 1..4).prop_map(Pred::All),
            proptest::collection::vec(inner.clone(), 1..4).prop_map(Pred::Any),
            inner.prop_map(|p| Pred::Not(Box::new(p))),
        ]
    })
}

/// A row with a value of the right type in every column but the key.
fn body() -> impl Strategy<Value = MapRow<C>> {
    (
        strategies::row(INTS, ints()),
        strategies::row(TEXTS, texts()),
    )
        .prop_map(|(ints, texts)| {
            let mut row = ints;
            for column in TEXTS {
                row = row.with(*column, texts.get(*column));
            }
            row
        })
}

#[test]
fn eval_agrees_with_the_query() {
    // One schema for the whole run, emptied before each case: a schema per
    // case would cost a round of DDL for every one of them.
    let Some(db) = postgres::Postgres::new(SCHEMA) else {
        return;
    };
    // The runner takes an `Fn`, and the oracle answers through `&mut`.
    let db = std::cell::RefCell::new(db);
    let config = Config {
        cases: 192,
        failure_persistence: None,
        ..Config::default()
    };
    let strategy = (pred(), proptest::collection::vec(body(), 1..6));
    let result = TestRunner::new(config).run(&strategy, |(pred, bodies)| {
        let mut db = db.borrow_mut();
        db.execute("DELETE FROM posts", &[]);
        let mut rows = Vec::with_capacity(bodies.len());
        for (index, body) in bodies.into_iter().enumerate() {
            let id = index as i64 + 1;
            let row = body.with(C::Id, id);
            db.execute(
                "INSERT INTO posts (id, author_id, team_id, status, score) VALUES ($1, $2, $3, $4, $5)",
                &[
                    Value::Int(id),
                    row.get(C::AuthorId),
                    row.get(C::TeamId),
                    row.get(C::Status),
                    row.get(C::Score),
                ],
            );
            rows.push(row);
        }

        match check::<Post, _, _>(&pred, &rows, Dialect::Postgres, &mut *db) {
            // A predicate with one interpretation cannot disagree with itself,
            // and the generator does not produce them anyway.
            Ok(()) | Err(HarnessError::NotEvaluable(_)) => Ok(()),
            Err(other) => Err(TestCaseError::fail(format!(
                "{other}\n  predicate: {pred:?}"
            ))),
        }
    });
    if let Err(failure) = result {
        panic!("{failure}");
    }
}
