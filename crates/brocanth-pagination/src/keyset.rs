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
//!
//! # A cursor value has its column's kind
//!
//! A client can write `after=t61` (text) for a list sorted on an integer.
//! PostgreSQL then refuses `bigint > text` and the request is a 500. So
//! [`Keyset::after`] needs `C: Sortable`, which says what each column holds,
//! and refuses a key of the wrong kind as "not a cursor", a 400. The check
//! is here and not in the database because the database's answer is an
//! error the handler cannot tell from an outage.

use brocanth_policy::{CompileError, Entity, Pred, SortKind, Sortable, Value};

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

    /// The columns a cursor holds a key for, in order.
    fn columns(&self) -> Vec<C> {
        if self.width() == 1 {
            vec![self.key]
        } else {
            vec![self.column, self.key]
        }
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

/// The cursor half: it needs to know what each column holds, so a cursor
/// value of the wrong kind stops here and never reaches the database.
impl<C: Sortable> Keyset<C> {
    /// The rows after `cursor`, as a predicate. [`Pred::Always`] with no
    /// cursor: the first page.
    ///
    /// `AND` it with the policy's rule before you compile: the cursor narrows
    /// the window, and the rule still decides what is in it.
    ///
    /// # Errors
    ///
    /// [`CursorError`] when the cursor holds the wrong number of keys, or a
    /// key that is not its column's kind: text for an integer column, an
    /// integer for a text column. Both are "not a cursor", a 400. Also when
    /// the order sorts on a column a cursor cannot carry (a float, a
    /// boolean), on the first page too, so the mistake shows at once.
    pub fn after(&self, cursor: Option<&Cursor>) -> Result<Pred<C>, CursorError> {
        let columns = self.columns();
        for column in &columns {
            if column.sort_kind().is_none() {
                let token = cursor.map(Cursor::token).unwrap_or_default();
                return Err(CursorError::unsortable(token, column.name()));
            }
        }
        let Some(cursor) = cursor else {
            return Ok(Pred::Always);
        };
        let cursor = cursor.clone().expect(self.width())?;
        let values = columns
            .iter()
            .zip(cursor.keys())
            .map(|(column, key)| value(*column, key))
            .collect::<Option<Vec<Value>>>()
            .ok_or_else(|| CursorError::not_a_cursor(cursor.token()))?;
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
}

/// The bind for `key` in `column`, or `None` when the kinds differ.
fn value<C: Sortable>(column: C, key: &Key) -> Option<Value> {
    match (column.sort_kind()?, key) {
        (SortKind::Int, Key::Int(n)) => Some(Value::Int(*n)),
        (SortKind::Text, Key::Text(text)) => Some(Value::Text(text.clone())),
        _ => None,
    }
}
