//! The predicate: a value, not a string, and the whole of the algebra.
//!
//! # Shape, because §10 depends on it
//!
//! Every variant here is public and every field is readable. That is not
//! laziness about encapsulation, it is the requirement PLAN.md §10 states:
//!
//! > A predicate you can inspect can be inverted into an audience. A string
//! > you can only execute.
//!
//! `brocanth-realtime` will pattern-match this enum to turn a policy into a
//! broadcast key, which is the difference between six hundred point queries
//! per write and one map lookup. [`crate::Audience`] is the tier-one version
//! of that inversion, and [`Pred::shape`] is the matchable summary it is built
//! on. A predicate that cannot be inverted is [`Pred::Opaque`], which says so
//! in a `reason` string and falls back to per-subscriber evaluation.
//!
//! # Constant folding, and why an absent value is visible afterwards
//!
//! [`Bound`] resolves at construction. `Bound::or_deny(None)` does not survive
//! as a comparison against a ghost; the comparison *becomes* [`Pred::Never`],
//! and `Pred::not` folds `Never` into [`Pred::Always`]. So the predicate an
//! author holds shows what their absent value did to it:
//!
//! ```
//! # use brocanth_policy::{Bound, Pred};
//! # #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] enum C { AuthorId }
//! # impl brocanth_policy::Column for C {
//! #     const ALL: &'static [Self] = &[Self::AuthorId];
//! #     fn name(&self) -> &'static str { "author_id" }
//! # }
//! let anonymous: Option<i64> = None;
//!
//! // "not the author" — the PocketBase shape, with an unresolvable left side.
//! let pred = Pred::negate(Pred::eq(C::AuthorId, Bound::or_deny(anonymous)));
//!
//! // PocketBase grants here, silently. We grant here too — and say so.
//! assert!(pred.is_always());
//! ```
//!
//! PocketBase reaches the same answer and cannot tell you. The difference is
//! that `is_always()` exists, a test can assert it, and the author had to
//! write the word `or_deny` to get there.

use crate::entity::Column;
use crate::value::{Bound, BoundInner, Missing, Value};

/// A comparison operator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// `=`. Three-valued: `NULL = anything` is unknown.
    Eq,
    /// `<>`. Three-valued, and this is the one PocketBase gets wrong.
    Ne,
    /// `<`.
    Lt,
    /// `<=`.
    Le,
    /// `>`.
    Gt,
    /// `>=`.
    Ge,
    /// NULL-safe equality. Two-valued: `NULL` equals `NULL` and nothing else.
    ///
    /// PostgreSQL spells it `IS NOT DISTINCT FROM`, SQLite `IS`. Use it when
    /// the author's answer to "what does absence mean" is "absence is a value
    /// like any other".
    Is,
    /// NULL-safe inequality. Two-valued, the negation of [`Op::Is`].
    ///
    /// PostgreSQL's `IS DISTINCT FROM`, SQLite's `IS NOT`. This is the honest
    /// version of the operator PLAN.md §9's worked example turns into an
    /// access grant: PocketBase reaches for `COALESCE` because its dialect
    /// gave it nothing better, and ours did.
    IsNot,
}

impl Op {
    /// `true` when this operator never answers [`crate::Truth::Unknown`].
    pub fn is_null_safe(&self) -> bool {
        matches!(self, Op::Is | Op::IsNot)
    }
}

/// A predicate over one entity's columns.
///
/// Build it with the constructors rather than the variants, because the
/// constructors fold constants and the variants do not. Match on the variants
/// freely; that is what they are for.
#[derive(Clone, Debug, PartialEq)]
pub enum Pred<C: Column> {
    /// Matches every row. Public.
    Always,
    /// Matches no row. The default a policy falls back to.
    Never,
    /// A column against a value.
    Cmp {
        /// The column.
        column: C,
        /// The operator.
        op: Op,
        /// The value, which reaches the database as a bind parameter and
        /// never as SQL text.
        value: Value,
    },
    /// A column against a set of values.
    In {
        /// The column.
        column: C,
        /// The set. Every element binds.
        values: Vec<Value>,
        /// `NOT IN` when true.
        negated: bool,
    },
    /// Conjunction. Empty means [`Pred::Always`], as the empty conjunction
    /// does in logic and as `WHERE` with no terms does in SQL.
    All(Vec<Pred<C>>),
    /// Disjunction. Empty means [`Pred::Never`].
    Any(Vec<Pred<C>>),
    /// Negation. Three-valued: `NOT Unknown` is `Unknown`.
    Not(Box<Pred<C>>),
    /// A row in another table that links to this one — "members of a team".
    ///
    /// Matchable by §10's inversion, which is why it is a variant rather than
    /// an [`Pred::Opaque`] fragment.
    Exists(Box<Membership<C>>),
    /// SQL this crate does not understand, carried verbatim.
    ///
    /// The escape hatch, and the thing [`crate::Audience`] cannot invert.
    Opaque(Opaque<C>),
}

/// A correlated `EXISTS` over a link table.
///
/// This is the fourth of §10's coarse audience partitions — public, any
/// authenticated subject, the owner, *members of a team* — and the only one
/// that needs a second table. Holding it as a named shape rather than as a
/// string is what lets realtime key a broadcast on the team.
#[derive(Clone, Debug, PartialEq)]
pub struct Membership<C: Column> {
    /// The column on the entity that the link table points at.
    pub local: C,
    /// The link table.
    pub table: crate::Ident,
    /// The column on the link table that matches [`Membership::local`].
    pub foreign: crate::Ident,
    /// Further constraints on the link table — in practice "and the member is
    /// this subject". Each value binds.
    pub filter: Vec<(crate::Ident, Op, Value)>,
    /// `NOT EXISTS` when true.
    pub negated: bool,
}

/// A predicate this crate carries but does not understand.
///
/// # What it costs
///
/// * §10 cannot invert it, so realtime falls back to one point query per
///   subscriber for any policy containing one. [`crate::Audience`] returns
///   `None` and says why.
/// * [`crate::Pred::eval`] cannot run it unless the author supplies an
///   evaluator, so a policy with an opaque fragment and no evaluator is
///   `condition()`-only — which is the default §9 obligation 1 asks for
///   anyway.
///
/// # Why the SQL is `&'static str`
///
/// Because that is the lock. A request body is not `&'static str`. Producing
/// one from user input means calling `String::leak` or `Box::leak`, which is a
/// deliberate act, written in the open, that a reviewer will see. The values
/// go through the same bind list as everything else.
#[derive(Clone, Debug)]
pub struct Opaque<C: Column> {
    /// Why this could not be expressed in the algebra. Shows up in
    /// [`crate::errors::EvalError::Opaque`] and in audience diagnostics, so
    /// write it for the person reading the fallback.
    pub reason: &'static str,
    /// The SQL, with `?` for each value. Dialect placeholder renumbering is
    /// [`crate::sql`]'s job, so a fragment writes `?` and only `?`.
    pub sql: &'static str,
    /// The values, in the order the placeholders appear.
    pub binds: Vec<Value>,
    /// The in-memory interpretation, when the author chose to supply one.
    pub eval: Option<fn(&dyn crate::RowView<C>) -> crate::Truth>,
}

/// Compares the fragment, not the evaluator.
///
/// Two function pointers are not meaningfully comparable — the same function
/// can have two addresses and two functions can share one — so what is
/// compared is whether an evaluator is present at all. That is the property
/// anything here cares about: a fragment with one has two interpretations and
/// a fragment without has one.
impl<C: Column> PartialEq for Opaque<C> {
    fn eq(&self, other: &Self) -> bool {
        self.reason == other.reason
            && self.sql == other.sql
            && self.binds == other.binds
            && self.eval.is_some() == other.eval.is_some()
    }
}

impl<C: Column> Opaque<C> {
    /// A fragment that only [`crate::Pred::condition`] can interpret.
    pub fn new(reason: &'static str, sql: &'static str, binds: Vec<Value>) -> Self {
        Opaque {
            reason,
            sql,
            binds,
            eval: None,
        }
    }

    /// The same fragment, with the second interpretation supplied by hand.
    ///
    /// PLAN.md §9 obligation 1 is the warning that goes with this: the two
    /// interpretations can disagree, and here nothing but the author keeps
    /// them together. Run [`crate::agreement`] over it.
    pub fn with_eval(mut self, eval: fn(&dyn crate::RowView<C>) -> crate::Truth) -> Self {
        self.eval = Some(eval);
        self
    }
}

impl<C: Column> Pred<C> {
    /// A comparison. The usual entry point.
    ///
    /// `bound` resolves immediately: an absent value that meant
    /// [`Missing::Deny`] produces [`Pred::Never`] and one that meant
    /// [`Missing::Allow`] produces [`Pred::Always`], so the predicate shows
    /// what the absence did.
    pub fn cmp(column: C, op: Op, bound: impl Into<Bound>) -> Self {
        match bound.into().0 {
            BoundInner::Value(value) => Pred::Cmp { column, op, value },
            BoundInner::Missing(Missing::Deny) => Pred::Never,
            BoundInner::Missing(Missing::Allow) => Pred::Always,
            BoundInner::Missing(Missing::Null) => Pred::Cmp {
                column,
                op,
                value: Value::Null,
            },
        }
    }

    /// `column = value`, three-valued.
    pub fn eq(column: C, bound: impl Into<Bound>) -> Self {
        Self::cmp(column, Op::Eq, bound)
    }

    /// `column <> value`, three-valued.
    ///
    /// This is the operator PLAN.md §9's worked example turns into an access
    /// grant. Here, against a `NULL` on either side, it is
    /// [`crate::Truth::Unknown`] and a `WHERE` clause does not match the row.
    /// An author who wanted "different, treating absence as a value" writes
    /// [`Pred::is_not`], which is PostgreSQL's `IS DISTINCT FROM`.
    pub fn ne(column: C, bound: impl Into<Bound>) -> Self {
        Self::cmp(column, Op::Ne, bound)
    }

    /// `column < value`.
    pub fn lt(column: C, bound: impl Into<Bound>) -> Self {
        Self::cmp(column, Op::Lt, bound)
    }

    /// `column <= value`.
    pub fn le(column: C, bound: impl Into<Bound>) -> Self {
        Self::cmp(column, Op::Le, bound)
    }

    /// `column > value`.
    pub fn gt(column: C, bound: impl Into<Bound>) -> Self {
        Self::cmp(column, Op::Gt, bound)
    }

    /// `column >= value`.
    pub fn ge(column: C, bound: impl Into<Bound>) -> Self {
        Self::cmp(column, Op::Ge, bound)
    }

    /// NULL-safe equality: `NULL` equals `NULL`, and the answer is never
    /// unknown.
    pub fn is(column: C, bound: impl Into<Bound>) -> Self {
        Self::cmp(column, Op::Is, bound)
    }

    /// NULL-safe inequality.
    pub fn is_not(column: C, bound: impl Into<Bound>) -> Self {
        Self::cmp(column, Op::IsNot, bound)
    }

    /// `column IS NULL`.
    pub fn is_null(column: C) -> Self {
        Pred::Cmp {
            column,
            op: Op::Is,
            value: Value::Null,
        }
    }

    /// `column IS NOT NULL`.
    pub fn is_not_null(column: C) -> Self {
        Pred::Cmp {
            column,
            op: Op::IsNot,
            value: Value::Null,
        }
    }

    /// `column IN (…)`. An empty set is [`Pred::Never`], which is what the
    /// empty disjunction means and what `IN ()` would mean if dialects let you
    /// write it.
    pub fn one_of(column: C, values: impl IntoIterator<Item = impl Into<Value>>) -> Self {
        let values: Vec<Value> = values.into_iter().map(Into::into).collect();
        if values.is_empty() {
            return Pred::Never;
        }
        Pred::In {
            column,
            values,
            negated: false,
        }
    }

    /// `column NOT IN (…)`. An empty set is [`Pred::Always`].
    ///
    /// A `NULL` *in the set* makes the whole thing unknown, which is SQL's
    /// rule and a common production surprise. [`crate::Pred::eval`] reproduces
    /// it rather than smoothing it over.
    pub fn none_of(column: C, values: impl IntoIterator<Item = impl Into<Value>>) -> Self {
        let values: Vec<Value> = values.into_iter().map(Into::into).collect();
        if values.is_empty() {
            return Pred::Always;
        }
        Pred::In {
            column,
            values,
            negated: true,
        }
    }

    /// Conjunction, with [`Pred::Always`] dropped and [`Pred::Never`]
    /// absorbing.
    pub fn all(parts: impl IntoIterator<Item = Pred<C>>) -> Self {
        let mut kept = Vec::new();
        for part in parts {
            match part {
                Pred::Always => continue,
                Pred::Never => return Pred::Never,
                Pred::All(inner) => kept.extend(inner),
                other => kept.push(other),
            }
        }
        match kept.len() {
            0 => Pred::Always,
            1 => kept.pop().unwrap_or(Pred::Always),
            _ => Pred::All(kept),
        }
    }

    /// Disjunction, with [`Pred::Never`] dropped and [`Pred::Always`]
    /// absorbing.
    pub fn any(parts: impl IntoIterator<Item = Pred<C>>) -> Self {
        let mut kept = Vec::new();
        for part in parts {
            match part {
                Pred::Never => continue,
                Pred::Always => return Pred::Always,
                Pred::Any(inner) => kept.extend(inner),
                other => kept.push(other),
            }
        }
        match kept.len() {
            0 => Pred::Never,
            1 => kept.pop().unwrap_or(Pred::Never),
            _ => Pred::Any(kept),
        }
    }

    /// Negation, folding the constants so an absent value stays visible.
    ///
    /// `negate(Never)` is `Always`. That is the PocketBase grant, reproduced
    /// on purpose and reported by [`Pred::is_always`], because hiding it would
    /// only mean the author could not test for it.
    ///
    /// Also spelled `!pred`.
    pub fn negate(inner: Pred<C>) -> Self {
        match inner {
            Pred::Always => Pred::Never,
            Pred::Never => Pred::Always,
            Pred::Not(inner) => *inner,
            other => Pred::Not(Box::new(other)),
        }
    }

    /// This predicate and another.
    pub fn and(self, other: Pred<C>) -> Self {
        Self::all([self, other])
    }

    /// This predicate or another.
    pub fn or(self, other: Pred<C>) -> Self {
        Self::any([self, other])
    }

    /// Matches every row.
    pub fn is_always(&self) -> bool {
        matches!(self, Pred::Always)
    }

    /// Matches no row.
    pub fn is_never(&self) -> bool {
        matches!(self, Pred::Never)
    }

    /// `true` when any part of this predicate is an [`Pred::Opaque`]
    /// fragment, and therefore when §10 must fall back to per-subscriber
    /// evaluation.
    pub fn has_opaque(&self) -> bool {
        match self {
            Pred::Opaque(_) => true,
            Pred::All(parts) | Pred::Any(parts) => parts.iter().any(Pred::has_opaque),
            Pred::Not(inner) => inner.has_opaque(),
            _ => false,
        }
    }

    /// A matchable summary of the outermost shape, for a crate that wants to
    /// walk the tree without matching every variant.
    pub fn shape(&self) -> Shape<'_, C> {
        match self {
            Pred::Always => Shape::Everyone,
            Pred::Never => Shape::NoOne,
            Pred::Cmp { column, op, value } => Shape::Column {
                column: *column,
                op: *op,
                value,
            },
            Pred::In {
                column,
                values,
                negated,
            } => Shape::Set {
                column: *column,
                values,
                negated: *negated,
            },
            Pred::All(parts) => Shape::All(parts),
            Pred::Any(parts) => Shape::Any(parts),
            Pred::Not(inner) => Shape::Negated(inner),
            Pred::Exists(membership) => Shape::Membership(membership),
            Pred::Opaque(fragment) => Shape::Opaque {
                reason: fragment.reason,
            },
        }
    }
}

/// The outermost shape of a predicate, borrowed.
///
/// A convenience over matching [`Pred`] itself, and the vocabulary PLAN.md §10
/// is written in: public, the owner, a set, members of a team, or opaque.
#[derive(Clone, Copy, Debug)]
pub enum Shape<'a, C: Column> {
    /// [`Pred::Always`].
    Everyone,
    /// [`Pred::Never`].
    NoOne,
    /// One column against one value — "the owner", "the team", "published".
    Column {
        /// The column.
        column: C,
        /// The operator.
        op: Op,
        /// The value.
        value: &'a Value,
    },
    /// One column against a set.
    Set {
        /// The column.
        column: C,
        /// The set.
        values: &'a [Value],
        /// `NOT IN` when true.
        negated: bool,
    },
    /// Every part.
    All(&'a [Pred<C>]),
    /// Any part.
    Any(&'a [Pred<C>]),
    /// The negation of the inner predicate.
    Negated(&'a Pred<C>),
    /// Membership in a link table.
    Membership(&'a Membership<C>),
    /// Not invertible, with the author's reason.
    Opaque {
        /// Why.
        reason: &'static str,
    },
}

impl<C: Column> std::ops::Not for Pred<C> {
    type Output = Pred<C>;

    /// [`Pred::negate`], as an operator.
    fn not(self) -> Pred<C> {
        Pred::negate(self)
    }
}

/// The predicate type for an entity, spelled the way a policy reads.
pub type EntityPred<E> = Pred<<E as crate::Entity>::Column>;

impl<C: Column> Membership<C> {
    /// "A row in `table` whose `foreign` column matches this entity's `local`
    /// column."
    ///
    /// The subject constraint goes on with [`Membership::filter_by`], which is
    /// where "and the member is this user" lives.
    pub fn new(local: C, table: crate::Ident, foreign: crate::Ident) -> Self {
        Membership {
            local,
            table,
            foreign,
            filter: Vec::new(),
            negated: false,
        }
    }

    /// Constrain the link table further.
    ///
    /// Returns the membership unchanged when `bound` was absent and meant
    /// [`Missing::Allow`], and `None` when it meant [`Missing::Deny`] — an
    /// anonymous subject is a member of nothing, and the caller turns that
    /// into [`Pred::Never`].
    pub fn filter_by(
        mut self,
        column: crate::Ident,
        op: Op,
        bound: impl Into<Bound>,
    ) -> Option<Self> {
        match bound.into().0 {
            BoundInner::Value(value) => {
                self.filter.push((column, op, value));
                Some(self)
            }
            BoundInner::Missing(Missing::Allow) => Some(self),
            BoundInner::Missing(Missing::Deny) => None,
            BoundInner::Missing(Missing::Null) => {
                self.filter.push((column, op, Value::Null));
                Some(self)
            }
        }
    }

    /// `NOT EXISTS` instead of `EXISTS`.
    pub fn negate(mut self) -> Self {
        self.negated = !self.negated;
        self
    }

    /// As a predicate.
    pub fn into_pred(self) -> Pred<C> {
        Pred::Exists(Box::new(self))
    }
}
