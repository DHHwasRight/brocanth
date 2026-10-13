//! The property-test half of PLAN.md §9 obligation 1, over generated
//! predicates.
//!
//! The table in `agreement.rs` covers the cases somebody thought of. This
//! covers the ones nobody did: random predicates over random rows, checked
//! against SQLite. It is the shape the `policy!` macro of PLAN.md §12 item 10
//! will emit for an application that opts into `eval()`, which is why the
//! strategies live in the library rather than here.

#![cfg(feature = "agreement-proptest")]

mod common;
#[path = "common/sqlite.rs"]
mod sqlite;

use brocanth_policy::agreement::{check, strategies, HarnessError};
use brocanth_policy::{Dialect, RowView as _, Value};
use common::{Post, PostColumn as C, SCHEMA};
use proptest::prelude::*;

/// A small pool, chosen so collisions are common: the interesting rows are the
/// ones that nearly match.
fn pool() -> Vec<Value> {
    vec![
        Value::Null,
        Value::Int(1),
        Value::Int(2),
        Value::Text("published".into()),
        Value::Text("draft".into()),
    ]
}

/// The columns the generator may touch. Not the primary key: the harness joins
/// on it, so it has to stay a distinct integer per row.
const SUBJECTS: &[C] = &[C::AuthorId, C::TeamId, C::Status, C::Score];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(192))]

    #[test]
    fn eval_agrees_with_the_query(
        pred in strategies::pred(SUBJECTS, pool()),
        bodies in proptest::collection::vec(strategies::row(SUBJECTS, pool()), 1..6),
    ) {
        let db = sqlite::Sqlite::new(SCHEMA);
        let mut rows = Vec::with_capacity(bodies.len());
        for (index, body) in bodies.into_iter().enumerate() {
            let id = index as i64 + 1;
            let row = body.with(C::Id, id);
            db.execute(
                "INSERT INTO posts (id, author_id, team_id, status, score) VALUES (?, ?, ?, ?, ?)",
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

        let mut db = db;
        match check::<Post, _, _>(&pred, &rows, Dialect::Sqlite, &mut db) {
            Ok(()) => {}
            // A predicate with one interpretation cannot disagree with itself,
            // and the generator does not produce them anyway.
            Err(HarnessError::NotEvaluable(_)) => {}
            Err(other) => prop_assert!(false, "{other}\n  predicate: {pred:?}"),
        }
    }
}
