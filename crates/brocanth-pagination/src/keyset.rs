//! A keyset page as a `Pred`, so the cursor goes into the `WHERE` beside the
//! policy's condition. Behind the `policy` feature.
//!
//! ```text
//! ascending:  "created_at" > $1 OR ("created_at" = $1 AND "id" > $2)
//! descending: "created_at" < $1 OR ("created_at" = $1 AND "id" < $2)
//! ```
//!
//! The tie-breaker is the primary key, because a sort column with two equal
//! values would otherwise skip or repeat a row at the edge of a page.
//!
//! # A sort column must not be null
//!
//! `NULL > $1` is unknown, so a row with a null sort key is never "after"
//! any cursor and drops out of every page but the first. Sort on a
//! `NOT NULL` column, or on the key alone.

use brocanth_policy::{CompileError, Entity, Pred, Value};

use crate::cursor::{Cursor, CursorError, Key};
use crate::Direction;

/// A keyset order: a sort column, a direction, and the key that breaks ties.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keyset<C: brocanth_policy::Column> {
    column: C,
    key: C,
    direction: Direction,
}

impl<C: brocanth_policy::Column> Keyset<C> {
    /// Sort on `column`, ties broken by `key`.
    pub fn new(column: C, key: C, direction: Direction) -> Self {
        Keyset {
            column,
            key,
            direction,
        }
    }

    /// Sort on the key alone.
    pub fn by_key(key: C, direction: Direction) -> Self {
        Keyset::new(key, key, direction)
    }

    /// How many keys a cursor for this order holds: one or two.
    pub fn width(&self) -> usize {
        if self.column == self.key {
            1
        } else {
            2
        }
    }

    /// The rows after `cursor`, as a predicate. [`Pred::Always`] with no
    /// cursor: the first page.
    ///
    /// `AND` it with the policy's rule before you compile: the cursor narrows
    /// the window, and the rule still decides what is in it.
    ///
    /// # Errors
    ///
    /// [`CursorError`] when the cursor holds the wrong number of keys.
    pub fn after(&self, cursor: Option<&Cursor>) -> Result<Pred<C>, CursorError> {
        let Some(cursor) = cursor else {
            return Ok(Pred::Always);
        };
        let cursor = cursor.clone().expect(self.width())?;
        let values: Vec<Value> = cursor.keys().iter().map(value).collect();
        let beyond = |column: C, value: Value| match self.direction {
            Direction::Ascending => Pred::gt(column, value),
            Direction::Descending => Pred::lt(column, value),
        };
        Ok(match values.as_slice() {
            [only] => beyond(self.key, only.clone()),
            [first, second] => Pred::any([
                beyond(self.column, first.clone()),
                Pred::all([
                    Pred::eq(self.column, first.clone()),
                    beyond(self.key, second.clone()),
                ]),
            ]),
            _ => unreachable!("`expect` checked the width"),
        })
    }

    /// The `ORDER BY` list, with the table name, as the condition writes its
    /// columns: `"posts"."created_at" DESC, "posts"."id" DESC`.
    ///
    /// # Errors
    ///
    /// [`CompileError::BadIdentifier`] for a column name that is not a bare
    /// SQL identifier.
    pub fn order_by<E: Entity<Column = C>>(&self) -> Result<String, CompileError> {
        let way = match self.direction {
            Direction::Ascending => "ASC",
            Direction::Descending => "DESC",
        };
        let table = E::TABLE.as_str();
        let column = |c: C| -> Result<String, CompileError> {
            Ok(format!("\"{table}\".\"{}\" {way}", c.ident()?.as_str()))
        };
        if self.width() == 1 {
            column(self.key)
        } else {
            Ok(format!("{}, {}", column(self.column)?, column(self.key)?))
        }
    }

    /// The `LIMIT` to read: one more row than the page shows, so
    /// [`KeysetPage::from_rows`](crate::KeysetPage::from_rows) knows whether
    /// there is a next page.
    pub fn fetch(limit: u64) -> u64 {
        limit.max(1) + 1
    }
}

fn value(key: &Key) -> Value {
    match key {
        Key::Int(n) => Value::Int(*n),
        Key::Text(text) => Value::Text(text.clone()),
    }
}
