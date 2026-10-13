//! The seam the honest limit in `brocanth-events` forces.
//!
//! > A `RowChange` carries changed field *names*, never column *values*.
//!
//! That is correct and it is why this module exists. §10 broadcasts by reading
//! the changed row's value for each column some subscriber keys on, and a
//! change does not carry values. So the values come from somewhere, and
//! "somewhere" is a trait with one method rather than an ORM dependency.
//!
//! # Three ways to fill it, in order of preference
//!
//! 1. **The publisher already holds the row.** The model layer just wrote it.
//!    [`Broker::publish_with_row`](crate::Broker::publish_with_row) takes the
//!    row directly and no [`RowReader`] runs at all. This is the path with
//!    literally zero queries, and it is the one the model layer should take.
//!    [`brocanth_events::ChangeSink::publish`] cannot express it — the trait
//!    carries identity, not values — so a publisher that wants it calls the
//!    broker directly instead of through the sink.
//! 2. **A reader that caches.** A `RowReader` over a small recent-rows cache
//!    answers most changes without touching the database.
//! 3. **A reader that queries.** One `SELECT` per changed row, shared by every
//!    subscriber on it. Still two to three orders of magnitude better than
//!    PocketBase, which runs one query *per subscriber*.
//!
//! # What the broker does when the row cannot be read
//!
//! It denies, and says so in [`Delivery::row_unavailable`](crate::Delivery).
//! Denial is the same direction `brocanth_policy::Truth::admits` collapses in,
//! and for the same reason: a `WHERE` clause keeps a row only when the
//! predicate is true. Subscribers whose audience is `Everyone` are unaffected,
//! because a policy that admits every row needs no column to prove it.
//!
//! A **delete** is the case to think about. The row is gone, so a reader that
//! queries will find nothing. The model layer holds the row it is about to
//! delete, so the answer is `publish_with_row` with the snapshot taken before
//! the delete. A reader that cannot do that returns `None`, and a delete then
//! reaches only the `Everyone` audience — which is a real loss, stated here
//! rather than discovered in production.

use std::collections::HashMap;

use brocanth_events::RowChange;
use brocanth_policy::{Column, RowView, Value};

/// One row's columns, with the column type erased to its name.
///
/// Erased because a broker serves many collections at once and each has its
/// own `Column` enum. The type parameter survives where it matters — a
/// subscription is built from a typed `Pred<C>` — and is dropped at the index,
/// where a single `HashMap` has to hold every collection.
///
/// A column that is absent is not the same as a column that is `NULL`.
/// [`Row::get`] reports the difference, and the broker treats an absent column
/// a subscriber keys on as an unreadable row rather than as a `NULL` that
/// happens to match nothing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    cells: HashMap<String, Value>,
}

impl Row {
    /// An empty row.
    pub fn new() -> Row {
        Row::default()
    }

    /// Set one column.
    #[must_use]
    pub fn with(mut self, column: impl Into<String>, value: impl Into<Value>) -> Row {
        self.cells.insert(column.into(), value.into());
        self
    }

    /// Set one column, typed. The name comes from the column enum, so it is
    /// the name the policy and the database both use.
    #[must_use]
    pub fn with_column<C: Column>(self, column: C, value: impl Into<Value>) -> Row {
        self.with(column.name(), value)
    }

    /// Set one column, in place.
    pub fn set(&mut self, column: impl Into<String>, value: impl Into<Value>) {
        self.cells.insert(column.into(), value.into());
    }

    /// The column's value, or `None` when the row does not carry it.
    pub fn get(&self, column: &str) -> Option<&Value> {
        self.cells.get(column)
    }

    /// `true` when every one of these columns is present.
    ///
    /// What the broker checks before it trusts a row: a reader that returned
    /// four of the five columns an index keys on would otherwise look like a
    /// row whose fifth column is `NULL`, and silently stop delivering to
    /// everyone keyed on it.
    pub fn has_all(&self, columns: &[&str]) -> bool {
        columns.iter().all(|c| self.cells.contains_key(*c))
    }

    /// How many columns this row carries.
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// `true` when this row carries no column at all.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Read this row as `brocanth-policy` reads one, for a given column enum.
    ///
    /// The adapter that lets an erased row be evaluated against a typed
    /// predicate. An absent column reads as [`Value::Null`], which is what
    /// [`RowView`] documents and the safe direction: the predicate refuses
    /// rather than guesses.
    pub fn view<C: Column>(&self) -> impl RowView<C> + '_ {
        Viewed {
            row: self,
            column: std::marker::PhantomData,
        }
    }
}

struct Viewed<'a, C> {
    row: &'a Row,
    column: std::marker::PhantomData<fn() -> C>,
}

impl<C: Column> RowView<C> for Viewed<'_, C> {
    fn get(&self, column: C) -> Value {
        self.row.get(column.name()).cloned().unwrap_or(Value::Null)
    }
}

/// Where the broker gets the values a change does not carry.
///
/// One method, because one method is the whole requirement, and because an
/// application that stores rows in something this workspace has never heard of
/// should be able to implement it in ten lines.
///
/// `columns` is exactly the set some subscriber on this change's topics keys
/// on, de-duplicated, which is usually one or two names and never the whole
/// table unless a policy is opaque. A reader is free to return more columns
/// than were asked for.
pub trait RowReader: Send + Sync {
    /// Read these columns of the row this change names.
    ///
    /// `None` means the row cannot be read — it is deleted, or the backing
    /// store is unavailable. The broker denies rather than guesses. Returning
    /// a row that is missing one of `columns` is treated the same way, so a
    /// reader does not have to decide what a half-answer means.
    ///
    /// Must not panic, and should not block for long: it runs once per
    /// published change, under the broker's read lock.
    fn read(&self, change: &RowChange<'_>, columns: &[&str]) -> Option<Row>;
}

/// A reader that reads nothing.
///
/// The default, and the right one for an application whose model layer calls
/// [`Broker::publish_with_row`](crate::Broker::publish_with_row): the row is
/// handed over, so nothing has to go and find it. Also what a deployment uses
/// when every policy inverts to `Everyone`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoRows;

impl RowReader for NoRows {
    fn read(&self, _change: &RowChange<'_>, _columns: &[&str]) -> Option<Row> {
        None
    }
}

impl<F> RowReader for F
where
    F: Fn(&RowChange<'_>, &[&str]) -> Option<Row> + Send + Sync,
{
    fn read(&self, change: &RowChange<'_>, columns: &[&str]) -> Option<Row> {
        self(change, columns)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum Post {
        AuthorId,
    }

    impl Column for Post {
        const ALL: &'static [Self] = &[Post::AuthorId];
        fn name(&self) -> &'static str {
            "author_id"
        }
    }

    #[test]
    fn an_absent_column_is_not_a_null_column() {
        let row = Row::new().with("author_id", 7i64);
        assert_eq!(row.get("author_id"), Some(&Value::Int(7)));
        assert_eq!(row.get("team_id"), None);
        assert!(row.has_all(&["author_id"]));
        assert!(!row.has_all(&["author_id", "team_id"]));
    }

    #[test]
    fn a_row_reads_as_a_policy_row() {
        let row = Row::new().with_column(Post::AuthorId, 7i64);
        let view = row.view::<Post>();
        assert_eq!(view.get(Post::AuthorId), Value::Int(7));
    }

    #[test]
    fn a_closure_is_a_reader() {
        let reader = |_change: &RowChange<'_>, _columns: &[&str]| Some(Row::new().with("a", 1i64));
        let change = RowChange::created("posts", 1i64);
        assert_eq!(reader.read(&change, &["a"]).unwrap().len(), 1);
        assert_eq!(NoRows.read(&change, &["a"]), None);
    }
}
