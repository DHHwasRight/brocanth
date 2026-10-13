//! The second interpretation: a row you already hold.
//!
//! # Three-valued logic, stated rather than discovered
//!
//! SQL has three truth values and this crate has the same three, because a
//! predicate with two interpretations that disagree about `NULL` is a
//! predicate with a security bug. [`Truth::Unknown`] is what a comparison
//! against `NULL` produces, and the tables are SQL's:
//!
//! ```text
//! AND   | T U F        OR    | T U F        NOT  | T U F
//! ------+------        ------+------        -----+------
//!   T   | T U F          T   | T T T             | F U T
//!   U   | U U F          U   | T U U
//!   F   | F F F          F   | T U F
//! ```
//!
//! `NOT Unknown` is `Unknown`, and that is the whole repair of PLAN.md §9's
//! vulnerability. PocketBase's `!=` over an unresolvable path becomes
//! `COALESCE(NULL,'') IS NOT COALESCE('banned','')`, which is **true**, so a
//! typo grants access. Here `Pred::ne(col, …)` against a `NULL` is `Unknown`,
//! negating it leaves `Unknown`, and [`Truth::admits`] — the one place the
//! third value collapses to a decision — answers `false`.
//!
//! # Where the collapse happens, and why there
//!
//! Exactly once, at the top, in [`Truth::admits`], and it collapses towards
//! denial. That is not a policy preference, it is what makes the two
//! interpretations agree: a SQL `WHERE` clause keeps a row when the predicate
//! is true and drops it when the predicate is false *or* unknown. Collapsing
//! anywhere else, or in the other direction, would make `eval()` and
//! `condition()` disagree about exactly the rows an attacker cares about.
//!
//! An author who wants a different meaning says so in the predicate, with
//! [`crate::Pred::is_null`], [`crate::Pred::is_not_null`] or the NULL-safe
//! [`crate::Op::Is`] and [`crate::Op::IsNot`]. The crate never decides for
//! them.
//!
//! # What `eval()` cannot do
//!
//! [`crate::Pred::Exists`] asks about another table, and an opaque fragment
//! with no evaluator is SQL this crate cannot read. Both return
//! [`EvalError`], which is the honest form of PLAN.md §9 obligation 1:
//! `condition()` always exists, `eval()` is opt-in and sometimes impossible.

use crate::entity::Column;
use crate::errors::EvalError;
use crate::pred::{Op, Pred};
use crate::value::Value;

/// SQL's three truth values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Truth {
    /// The predicate holds.
    True,
    /// The predicate does not hold.
    False,
    /// A `NULL` took part and the answer is not knowable.
    Unknown,
}

impl Truth {
    /// The decision a `WHERE` clause makes: true matches, everything else does
    /// not.
    ///
    /// The only place [`Truth::Unknown`] becomes a yes or a no, and it becomes
    /// a no.
    pub fn admits(self) -> bool {
        matches!(self, Truth::True)
    }

    /// Three-valued conjunction.
    pub fn and(self, other: Truth) -> Truth {
        match (self, other) {
            (Truth::False, _) | (_, Truth::False) => Truth::False,
            (Truth::True, Truth::True) => Truth::True,
            _ => Truth::Unknown,
        }
    }

    /// Three-valued disjunction.
    pub fn or(self, other: Truth) -> Truth {
        match (self, other) {
            (Truth::True, _) | (_, Truth::True) => Truth::True,
            (Truth::False, Truth::False) => Truth::False,
            _ => Truth::Unknown,
        }
    }
}

/// Three-valued negation, written `!truth`. `NOT Unknown` is `Unknown`, and
/// that one line is the repair of PLAN.md §9's vulnerability class.
///
/// The operator rather than a method, so it cannot be confused with the
/// two-valued negation of a `bool`.
impl std::ops::Not for Truth {
    type Output = Truth;

    fn not(self) -> Truth {
        match self {
            Truth::True => Truth::False,
            Truth::False => Truth::True,
            Truth::Unknown => Truth::Unknown,
        }
    }
}

impl From<bool> for Truth {
    fn from(value: bool) -> Self {
        if value {
            Truth::True
        } else {
            Truth::False
        }
    }
}

/// One row, as [`crate::Pred::eval`] reads it.
///
/// Object-safe, because an opaque fragment's evaluator is a plain function
/// pointer over `&dyn RowView<C>` and a function pointer keeps [`crate::Pred`]
/// `Clone` and `PartialEq` without a dependency on anything.
pub trait RowView<C: Column> {
    /// The column's value in this row. [`Value::Null`] for SQL `NULL`.
    ///
    /// A column the row does not carry is a caller error; return
    /// [`Value::Null`] and the predicate will refuse rather than guess, which
    /// is the safe direction.
    fn get(&self, column: C) -> Value;
}

/// A row built by hand, for tests and for a caller that already has a map.
#[derive(Clone, Debug, Default)]
pub struct MapRow<C: Column> {
    cells: Vec<(C, Value)>,
}

impl<C: Column> MapRow<C> {
    /// An empty row. Every column reads as `NULL`.
    pub fn new() -> Self {
        MapRow { cells: Vec::new() }
    }

    /// Set one cell.
    #[must_use]
    pub fn with(mut self, column: C, value: impl Into<Value>) -> Self {
        self.cells.retain(|(c, _)| *c != column);
        self.cells.push((column, value.into()));
        self
    }

    /// Set one cell to `NULL`.
    #[must_use]
    pub fn with_null(self, column: C) -> Self {
        self.with(column, Value::Null)
    }
}

impl<C: Column> RowView<C> for MapRow<C> {
    fn get(&self, column: C) -> Value {
        self.cells
            .iter()
            .find(|(c, _)| *c == column)
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Null)
    }
}

impl<C: Column> Pred<C> {
    /// Evaluate against a row, in three-valued logic.
    ///
    /// Opt-in, per PLAN.md §9 obligation 1. The default path is
    /// [`Pred::condition`] compiled into a one-row select, which is what
    /// PocketBase's `CanAccessRecord` does and what cannot disagree with
    /// itself. Calling this takes on the obligation to prove the two agree;
    /// [`crate::agreement`] is the harness for it.
    pub fn eval(&self, row: &dyn RowView<C>) -> Result<Truth, EvalError> {
        match self {
            Pred::Always => Ok(Truth::True),
            Pred::Never => Ok(Truth::False),
            Pred::Cmp { column, op, value } => Ok(compare(&row.get(*column), *op, value)),
            Pred::In {
                column,
                values,
                negated,
            } => {
                let lhs = row.get(*column);
                let truth = in_set(&lhs, values);
                Ok(if *negated { !truth } else { truth })
            }
            Pred::All(parts) => {
                let mut acc = Truth::True;
                for part in parts {
                    acc = acc.and(part.eval(row)?);
                }
                Ok(acc)
            }
            Pred::Any(parts) => {
                let mut acc = Truth::False;
                for part in parts {
                    acc = acc.or(part.eval(row)?);
                }
                Ok(acc)
            }
            Pred::Not(inner) => Ok(!inner.eval(row)?),
            Pred::Exists(membership) => Err(EvalError::NeedsDatabase {
                table: membership.table.as_str(),
            }),
            Pred::Opaque(fragment) => match fragment.eval {
                Some(f) => Ok(f(row)),
                None => Err(EvalError::Opaque {
                    reason: fragment.reason,
                }),
            },
        }
    }

    /// Evaluate and collapse, in the one direction a `WHERE` clause collapses.
    ///
    /// Shorthand for `eval(row)?.admits()`.
    pub fn admits(&self, row: &dyn RowView<C>) -> Result<bool, EvalError> {
        Ok(self.eval(row)?.admits())
    }
}

/// One comparison, in SQL's semantics.
fn compare(lhs: &Value, op: Op, rhs: &Value) -> Truth {
    if op.is_null_safe() {
        let same = null_safe_eq(lhs, rhs);
        return Truth::from(if op == Op::Is { same } else { !same });
    }
    if lhs.is_null() || rhs.is_null() {
        return Truth::Unknown;
    }
    let ordering = order(lhs, rhs);
    Truth::from(match op {
        Op::Eq => ordering == std::cmp::Ordering::Equal,
        Op::Ne => ordering != std::cmp::Ordering::Equal,
        Op::Lt => ordering == std::cmp::Ordering::Less,
        Op::Le => ordering != std::cmp::Ordering::Greater,
        Op::Gt => ordering == std::cmp::Ordering::Greater,
        Op::Ge => ordering != std::cmp::Ordering::Less,
        Op::Is | Op::IsNot => unreachable!("handled above"),
    })
}

fn null_safe_eq(lhs: &Value, rhs: &Value) -> bool {
    match (lhs.is_null(), rhs.is_null()) {
        (true, true) => true,
        (true, false) | (false, true) => false,
        (false, false) => order(lhs, rhs) == std::cmp::Ordering::Equal,
    }
}

/// `x IN (…)`, three-valued.
///
/// `NULL IN (…)` is unknown. A value not present in a set that contains a
/// `NULL` is unknown rather than false, because the `NULL` might have been it.
/// That is SQL, and `NOT IN` over a set containing `NULL` matching nothing is
/// the production surprise it produces.
fn in_set(lhs: &Value, values: &[Value]) -> Truth {
    if lhs.is_null() {
        return Truth::Unknown;
    }
    let mut saw_null = false;
    for value in values {
        if value.is_null() {
            saw_null = true;
        } else if order(lhs, value) == std::cmp::Ordering::Equal {
            return Truth::True;
        }
    }
    if saw_null {
        Truth::Unknown
    } else {
        Truth::False
    }
}

/// Ordering between two non-`NULL` values, in SQLite's storage-class order:
/// integers and reals compare numerically with each other, then text, then
/// blobs.
///
/// A policy that compares a text column to an integer is relying on dialect
/// behaviour — SQLite coerces by column affinity, PostgreSQL raises a type
/// error — and [`crate::agreement`] is how you find that out rather than this
/// function. Comparing like with like has no such question.
fn order(lhs: &Value, rhs: &Value) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => a.partial_cmp(b).unwrap_or(Ordering::Equal),
        (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b).unwrap_or(Ordering::Equal),
        (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)).unwrap_or(Ordering::Equal),
        (Value::Text(a), Value::Text(b)) => a.cmp(b),
        (Value::Bytes(a), Value::Bytes(b)) => a.cmp(b),
        // SQLite stores a boolean as an integer, so this is the same question.
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Bool(a), other) => order(&Value::Int(i64::from(*a)), other),
        (other, Value::Bool(b)) => order(other, &Value::Int(i64::from(*b))),
        _ => class(lhs).cmp(&class(rhs)),
    }
}

/// SQLite's storage-class ordering: NULL < numeric < text < blob.
fn class(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) | Value::Int(_) | Value::Float(_) => 1,
        Value::Text(_) => 2,
        Value::Bytes(_) => 3,
    }
}
