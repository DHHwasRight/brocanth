//! The values a predicate can bind, and the decision an absent value forces.
//!
//! # Why [`Bound`] exists at all
//!
//! PLAN.md §9 names the vulnerability class this crate deletes. PocketBase
//! resolves `@request.auth.role` against an empty request object, an
//! unresolvable path becomes `NULL`, and its `!=` compiles to `IS NOT` over
//! `COALESCE`, so a *misspelled* field name in a negated rule evaluates to
//! true and grants access. The fix §9 records is that "a typed predicate over
//! `Option<T>` forces the author to say what absence means".
//!
//! That is what [`Bound`] is. A definite value converts into one for free:
//!
//! ```
//! # use brocanth_policy::Bound;
//! let bound: Bound = 7i64.into();
//! let bound: Bound = "draft".into();
//! ```
//!
//! An `Option` does not, and will not compile:
//!
//! ```compile_fail
//! # use brocanth_policy::Bound;
//! let user_id: Option<i64> = None;
//! let bound: Bound = user_id.into();   // no `From<Option<i64>> for Bound`
//! ```
//!
//! The author has to name the meaning of absence, at the call site, in a word:
//!
//! ```
//! # use brocanth_policy::Bound;
//! let user_id: Option<i64> = None;
//! let deny = Bound::or_deny(user_id);    // anonymous sees nothing
//! let allow = Bound::or_allow(user_id);  // anonymous sees everything
//! let null = Bound::or_null(user_id);    // compare against NULL, three-valued
//! ```
//!
//! There is no fourth choice and no default, which is the point.

use std::fmt;

/// A value a predicate compares against, or writes.
///
/// Deliberately small. This crate is an algebra over column names and bind
/// values, so the set is the set SQLite stores plus a boolean, and anything
/// richer — a UUID, a timestamp, a JSON document — is the caller's job to
/// encode the way its schema already stores it. Widening this enum means
/// taking an opinion about column types, which belongs to the model layer.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// SQL `NULL`. Absence, not a value: see [`crate::Truth`].
    Null,
    /// A boolean. PostgreSQL has a real `BOOLEAN` and binds one; SQLite has
    /// no boolean type and binds an integer. Either way the adapter binds what
    /// its driver binds, and this crate never writes one as SQL text.
    Bool(bool),
    /// A signed integer.
    Int(i64),
    /// A float. No `Eq` and no `Hash` on [`Value`] because of this variant,
    /// which is the correct trade: a policy that keys an audience on a float
    /// is a policy with a bug.
    Float(f64),
    /// Text.
    Text(String),
    /// A blob.
    Bytes(Vec<u8>),
}

impl Value {
    /// `true` when this is [`Value::Null`].
    ///
    /// Spelled out because `== Value::Null` reads like a comparison with NULL,
    /// which in SQL is never true, and this one always is.
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The name of the variant, for error messages and snapshots.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Text(_) => "text",
            Value::Bytes(_) => "bytes",
        }
    }
}

impl fmt::Display for Value {
    /// For diagnostics only.
    ///
    /// Nothing in [`crate::sql`] calls this. A value reaches the database as a
    /// bind parameter or it does not reach it at all, and the test
    /// `tests/injection.rs` is what keeps that true.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("NULL"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Text(s) => write!(f, "{s:?}"),
            Value::Bytes(b) => write!(f, "<{} bytes>", b.len()),
        }
    }
}

macro_rules! from_into_value {
    ($($ty:ty => |$v:ident| $body:expr,)*) => {$(
        impl From<$ty> for Value {
            fn from($v: $ty) -> Self { $body }
        }
        impl From<$ty> for Bound {
            fn from(v: $ty) -> Self { Bound::of(Value::from(v)) }
        }
    )*};
}

from_into_value! {
    bool => |v| Value::Bool(v),
    i8 => |v| Value::Int(v.into()),
    i16 => |v| Value::Int(v.into()),
    i32 => |v| Value::Int(v.into()),
    i64 => |v| Value::Int(v),
    u8 => |v| Value::Int(v.into()),
    u16 => |v| Value::Int(v.into()),
    u32 => |v| Value::Int(v.into()),
    f32 => |v| Value::Float(v.into()),
    f64 => |v| Value::Float(v),
    String => |v| Value::Text(v),
    &str => |v| Value::Text(v.to_owned()),
    Vec<u8> => |v| Value::Bytes(v),
}

/// What a predicate means when the value it was going to compare against is
/// not there.
///
/// Three answers, no default. See the module documentation for why.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Missing {
    /// The comparison is false, whatever the row holds. An anonymous visitor
    /// matches nothing.
    Deny,
    /// The comparison is true, whatever the row holds. Say this out loud, in
    /// code review, with a reason.
    Allow,
    /// Compare against `NULL` and let three-valued logic run. The comparison
    /// is [`crate::Truth::Unknown`], which a `WHERE` clause treats as not
    /// matching — but `Unknown` composes differently from `False` under `OR`
    /// and under `NOT`, so this is not a synonym for [`Missing::Deny`].
    Null,
}

/// One side of a comparison: either a value, or a decision about its absence.
///
/// `Bound` is the only way user data enters a predicate, and [`Value`] is the
/// only thing it can hold. Neither ever becomes SQL text.
#[derive(Clone, Debug, PartialEq)]
pub struct Bound(pub(crate) BoundInner);

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BoundInner {
    Value(Value),
    Missing(Missing),
}

impl Bound {
    /// A value that is definitely there.
    pub fn of(value: impl Into<Value>) -> Self {
        Bound(BoundInner::Value(value.into()))
    }

    /// `None` means the comparison is false. The usual choice for an identity:
    /// a visitor with no account does not own anything.
    pub fn or_deny<T: Into<Value>>(value: Option<T>) -> Self {
        Self::or(value, Missing::Deny)
    }

    /// `None` means the comparison is true. Rare, and worth a comment next to
    /// every use.
    pub fn or_allow<T: Into<Value>>(value: Option<T>) -> Self {
        Self::or(value, Missing::Allow)
    }

    /// `None` binds `NULL` and three-valued logic decides. Choose this when
    /// the absence is data rather than an unauthenticated request — a column
    /// that is genuinely nullable on both sides.
    pub fn or_null<T: Into<Value>>(value: Option<T>) -> Self {
        Self::or(value, Missing::Null)
    }

    /// The same three choices, as a value, for a caller that computes which
    /// one applies.
    pub fn or<T: Into<Value>>(value: Option<T>, missing: Missing) -> Self {
        match value {
            Some(v) => Bound(BoundInner::Value(v.into())),
            None => Bound(BoundInner::Missing(missing)),
        }
    }

    /// The value, when there is one.
    pub fn value(&self) -> Option<&Value> {
        match &self.0 {
            BoundInner::Value(v) => Some(v),
            BoundInner::Missing(_) => None,
        }
    }

    /// The decision that stands in for an absent value, when there is one.
    pub fn missing(&self) -> Option<Missing> {
        match &self.0 {
            BoundInner::Value(_) => None,
            BoundInner::Missing(m) => Some(*m),
        }
    }
}

impl From<Value> for Bound {
    fn from(value: Value) -> Self {
        Bound(BoundInner::Value(value))
    }
}
