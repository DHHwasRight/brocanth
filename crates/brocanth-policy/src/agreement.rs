//! The harness for PLAN.md §9 obligation 1, and an honest account of how far
//! it got.
//!
//! > 1. **The two interpretations can disagree**, and PocketBase cannot have
//! >    this bug because it has only one. Default to `condition()` only,
//! >    implemented as a one-row select … `eval()` is opt-in, and opting in
//! >    generates a proptest asserting `eval(row) == (row ∈ query(condition))`.
//! >    **The obligation moves from review to CI.**
//!
//! # Who runs it
//!
//! Two callers. An application with a hand-written policy calls [`check`]
//! with its own predicates, rows and database. A policy written with the
//! `policy!` macro (PLAN.md §12 item 10) and an `eval { … }` clause gets a
//! generated `#[test]` that calls [`policy_agrees`]: every rule, for every
//! subject the author lists, over generated rows. So for a macro policy the
//! obligation is in CI the moment the author opts into `eval()`.
//!
//! This crate's own `tests/agreement.rs` drives [`check`] against a real
//! SQLite database over a table of hand-written predicates, and
//! `tests/agreement_proptest.rs` drives it over generated ones, so the
//! compiler and the evaluator are held together here even though an
//! application's own policies are its own responsibility.
//!
//! # The dialect this crate's own agreement tests prove it on
//!
//! **SQLite, which is not the base dialect.** PostgreSQL is where brocanth
//! deploys; SQLite is the smaller setup. The oracle in `tests/common/sqlite.rs`
//! runs an in-memory SQLite, so `tests/agreement.rs` and
//! `tests/agreement_proptest.rs` prove the two interpretations agree *on
//! SQLite*.
//!
//! That is a stated limit rather than a hidden one. A test that runs on every
//! machine with no server beats a better test that needs one, and the gap is
//! narrow: the predicate shapes with dialect-specific SQL are the null-safe
//! operators, where `IS DISTINCT FROM` and `IS NOT` are defined to mean the
//! same thing, and the constants, where `FALSE` and `1 = 0` are. Closing it
//! properly needs a live PostgreSQL in CI — a service container or a
//! testcontainer — plus a `sqlx` `postgres` dev-feature and a second
//! `Oracle`. [`Oracle`] is the seam for exactly that, and nothing else would
//! change.
//!
//! The dialect-sensitive behaviour that *can* be checked without a server is
//! checked: `tests/snapshots.rs` asserts the generated text for both dialects,
//! including PostgreSQL's placeholder renumbering across a spliced condition,
//! which is the one dialect difference that is a bug rather than a spelling.
//!
//! # Why an [`Oracle`] rather than a database
//!
//! This crate has no driver and will not grow one. The question
//! `row ∈ query(condition)` can only be answered by the engine that will
//! answer it in production, so the harness asks the caller to run the query.
//! A caller that answers from a reimplementation of SQL proves only that two
//! of its own guesses agree.

use crate::entity::Entity;
use crate::errors::EvalError;
use crate::eval::{RowView, Truth};
use crate::sql::{Dialect, Query};
use crate::value::Value;

/// Whatever runs the query. The application's database, in its own words.
pub trait Oracle<E: Entity> {
    /// Run `query` and return the primary key of every row it matched.
    ///
    /// The query is [`crate::Pred::matching_ids`], so it selects exactly that
    /// column and nothing else.
    fn matching(&mut self, query: &Query) -> Result<Vec<Value>, String>;
}

impl<E: Entity, F> Oracle<E> for F
where
    F: FnMut(&Query) -> Result<Vec<Value>, String>,
{
    fn matching(&mut self, query: &Query) -> Result<Vec<Value>, String> {
        self(query)
    }
}

/// The two interpretations gave different answers for one row.
#[derive(Clone, Debug, PartialEq)]
pub struct Disagreement {
    /// The row's primary key.
    pub id: Value,
    /// What [`crate::Pred::eval`] said.
    pub evaluated: Truth,
    /// Whether the database returned the row.
    pub selected: bool,
    /// The SQL that was run, so the report is actionable without a rerun.
    pub sql: String,
    /// Its binds.
    pub binds: Vec<Value>,
}

/// The harness refused to run.
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum HarnessError {
    /// The predicate would not compile.
    #[error("the predicate did not compile: {0}")]
    Compile(#[from] crate::errors::CompileError),

    /// The predicate cannot be evaluated in memory at all — an `EXISTS`, or an
    /// opaque fragment with no evaluator.
    ///
    /// Not a disagreement. A predicate with only one interpretation is the
    /// §9 default and cannot disagree with itself.
    #[error("the predicate has no in-memory interpretation: {0}")]
    NotEvaluable(#[from] EvalError),

    /// The oracle failed.
    #[error("the oracle failed: {0}")]
    Oracle(String),

    /// The two interpretations disagree. This is the finding.
    #[error(
        "eval() said {:?} and the query said {} for row {} of `{}`",
        .0.evaluated, if .0.selected { "yes" } else { "no" }, .0.id, .0.sql
    )]
    Disagreed(Disagreement),
}

/// Assert `eval(row) == (row ∈ query(condition))` over a set of rows.
///
/// The rows must be the rows the oracle's table holds, and each must carry the
/// entity's primary key, because that is what joins the two answers together.
pub fn check<E, R, O>(
    pred: &crate::Pred<E::Column>,
    rows: &[R],
    dialect: Dialect,
    oracle: &mut O,
) -> Result<(), HarnessError>
where
    E: Entity,
    R: RowView<E::Column>,
    O: Oracle<E> + ?Sized,
{
    let query = pred.matching_ids::<E>(dialect)?;
    let selected = oracle.matching(&query).map_err(HarnessError::Oracle)?;

    for row in rows {
        let id = row.get(E::PRIMARY_KEY);
        let evaluated = pred.eval(row)?;
        let in_set = selected.contains(&id);
        if evaluated.admits() != in_set {
            return Err(HarnessError::Disagreed(Disagreement {
                id,
                evaluated,
                selected: in_set,
                sql: query.sql().to_owned(),
                binds: query.binds().to_vec(),
            }));
        }
    }

    // A row the query returned that the caller did not hand us is a row the
    // caller forgot, and silently passing would make the harness useless
    // exactly where it matters: a predicate that admits too much.
    for id in &selected {
        if !rows.iter().any(|row| row.get(E::PRIMARY_KEY) == *id) {
            return Err(HarnessError::Disagreed(Disagreement {
                id: id.clone(),
                evaluated: Truth::False,
                selected: true,
                sql: query.sql().to_owned(),
                binds: query.binds().to_vec(),
            }));
        }
    }

    Ok(())
}

#[cfg(feature = "agreement-proptest")]
pub use policy_test::policy_agrees;

#[cfg(feature = "agreement-proptest")]
mod policy_test;

/// Strategies for generating predicates and rows, for an application that
/// wants the property test rather than the table.
///
/// Behind the `agreement-proptest` feature, because an application that never
/// calls [`crate::Pred::eval`] should not link a property-testing framework to
/// avoid a bug it cannot have.
#[cfg(feature = "agreement-proptest")]
pub mod strategies {
    use proptest::prelude::*;

    use crate::entity::Column;
    use crate::eval::MapRow;
    use crate::pred::{Op, Pred};
    use crate::value::Value;

    /// One of `values`, as a [`Value`].
    pub fn value(values: Vec<Value>) -> impl Strategy<Value = Value> {
        proptest::sample::select(values)
    }

    /// A predicate over `columns`, drawing its values from `values`.
    ///
    /// Covers the shapes the compiler has arms for, excluding `EXISTS` and
    /// opaque fragments, which have no in-memory interpretation and therefore
    /// no agreement to check.
    pub fn pred<C: Column>(
        columns: &'static [C],
        values: Vec<Value>,
    ) -> impl Strategy<Value = Pred<C>> {
        let ops = prop_oneof![
            Just(Op::Eq),
            Just(Op::Ne),
            Just(Op::Lt),
            Just(Op::Le),
            Just(Op::Gt),
            Just(Op::Ge),
            Just(Op::Is),
            Just(Op::IsNot),
        ];
        let leaf_values = values.clone();
        let leaf = prop_oneof![
            1 => Just(Pred::Always),
            1 => Just(Pred::Never),
            8 => (
                proptest::sample::select(columns),
                ops,
                proptest::sample::select(leaf_values),
            )
                .prop_map(|(column, op, value)| Pred::Cmp { column, op, value }),
            3 => (
                proptest::sample::select(columns),
                proptest::collection::vec(proptest::sample::select(values), 1..4),
                any::<bool>(),
            )
                .prop_map(|(column, values, negated)| Pred::In {
                    column,
                    values,
                    negated,
                }),
        ];
        leaf.prop_recursive(3, 16, 3, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 1..4).prop_map(Pred::All),
                proptest::collection::vec(inner.clone(), 1..4).prop_map(Pred::Any),
                inner.prop_map(|p| Pred::Not(Box::new(p))),
            ]
        })
    }

    /// A row over `columns`, drawing its cells from `values`.
    pub fn row<C: Column>(
        columns: &'static [C],
        values: Vec<Value>,
    ) -> impl Strategy<Value = MapRow<C>> {
        proptest::collection::vec(proptest::sample::select(values), columns.len()).prop_map(
            move |cells| {
                let mut out = MapRow::new();
                for (column, value) in columns.iter().zip(cells) {
                    out = out.with(*column, value);
                }
                out
            },
        )
    }
}
