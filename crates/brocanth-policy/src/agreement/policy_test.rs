//! The property test `policy!` emits for `eval { … }`.
//!
//! PLAN.md §9 obligation 1: opting into `eval()` generates a proptest
//! asserting `eval(row) == (row ∈ query(condition))`. The macro emits one
//! `#[test]` that calls [`policy_agrees`]. The logic lives here, in the
//! library, so it is tested once and the generated code stays one call.

use proptest::test_runner::{Config, TestCaseError, TestRunner};

use super::{check, HarnessError, Oracle};
use crate::entity::{Column, Entity};
use crate::eval::MapRow;
use crate::policy::Policy;
use crate::pred::Pred;
use crate::sql::Dialect;
use crate::typed::TypedColumn;
use crate::value::Value;

/// The rule names, in the order the test reports them.
const ACTIONS: [&str; 5] = ["list", "view", "create", "update", "delete"];

/// Assert that every rule of `policy` agrees with its own SQL, for every
/// subject in `subjects`, over generated rows.
///
/// For each case the test generates one to five rows, asks `database` for a
/// fresh database that holds exactly those rows, and runs [`check`] for each
/// subject and each rule. A rule with no in-memory interpretation (an
/// `EXISTS`, or an opaque fragment without an evaluator) has one
/// interpretation only and cannot disagree with itself, so it is skipped.
///
/// The cell values come from two places: a few values of each column's type
/// ([`TypedColumn::samples`]), and every value the rules compare that column
/// against. The second source makes near-misses common, and near-misses are
/// the rows where two interpretations disagree.
///
/// `Err` holds the smallest failing case proptest found.
pub fn policy_agrees<P, O>(
    policy: &P,
    subjects: &[P::Subject],
    dialect: Dialect,
    cases: u32,
    database: impl Fn(&[MapRow<<P::Entity as Entity>::Column>]) -> O,
) -> Result<(), String>
where
    P: Policy,
    <P::Entity as Entity>::Column: TypedColumn,
    O: Oracle<P::Entity>,
{
    type C<P> = <<P as Policy>::Entity as Entity>::Column;

    let rules: Vec<(usize, &str, Pred<C<P>>)> = subjects
        .iter()
        .enumerate()
        .flat_map(|(index, subject)| {
            [
                policy.list(subject),
                policy.view(subject),
                policy.create(subject),
                policy.update(subject),
                policy.delete(subject),
            ]
            .into_iter()
            .zip(ACTIONS)
            .map(move |(pred, action)| (index, action, pred))
        })
        .collect();

    let key = <P::Entity as Entity>::PRIMARY_KEY;
    let columns: Vec<C<P>> = C::<P>::ALL.iter().copied().filter(|c| *c != key).collect();
    let pools: Vec<Vec<Value>> = columns
        .iter()
        .map(|column| pool(*column, rules.iter().map(|(_, _, pred)| pred)))
        .collect();

    let row = pools
        .iter()
        .map(|pool| proptest::sample::select(pool.clone()))
        .collect::<Vec<_>>();
    let rows = proptest::collection::vec(row, 1..6);

    let config = Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner = TestRunner::new(config);
    let result = runner.run(&rows, |bodies| {
        let rows: Vec<MapRow<C<P>>> = bodies
            .into_iter()
            .enumerate()
            .map(|(index, cells)| {
                let mut row = MapRow::new().with(key, key.key(index as i64 + 1));
                for (column, value) in columns.iter().zip(cells) {
                    row = row.with(*column, value);
                }
                row
            })
            .collect();
        let mut oracle = database(&rows);
        for (subject, action, pred) in &rules {
            match check::<P::Entity, _, _>(pred, &rows, dialect, &mut oracle) {
                Ok(()) | Err(HarnessError::NotEvaluable(_)) => {}
                Err(other) => {
                    return Err(TestCaseError::fail(format!(
                        "rule `{action}` for subject {subject}: {other}\n  predicate: {pred:?}"
                    )));
                }
            }
        }
        Ok(())
    });
    result.map_err(|e| e.to_string())
}

/// The values a column takes in generated rows.
fn pool<'a, C: TypedColumn + 'a>(
    column: C,
    rules: impl Iterator<Item = &'a Pred<C>>,
) -> Vec<Value> {
    let mut out = column.samples();
    let nullable = out.iter().any(Value::is_null);
    for pred in rules {
        collect(pred, column, &mut out);
    }
    // A NOT NULL column gets no NULL, even when a rule compares with one:
    // the database would refuse the row.
    if !nullable {
        out.retain(|v| !v.is_null());
    }
    out
}

fn collect<C: Column>(pred: &Pred<C>, column: C, out: &mut Vec<Value>) {
    match pred {
        Pred::Cmp {
            column: c, value, ..
        } if *c == column => push(out, value),
        Pred::In {
            column: c, values, ..
        } if *c == column => values.iter().for_each(|v| push(out, v)),
        Pred::All(parts) | Pred::Any(parts) => {
            parts.iter().for_each(|p| collect(p, column, out));
        }
        Pred::Not(inner) => collect(inner, column, out),
        _ => {}
    }
}

fn push(out: &mut Vec<Value>, value: &Value) {
    if !out.contains(value) {
        out.push(value.clone());
    }
}
