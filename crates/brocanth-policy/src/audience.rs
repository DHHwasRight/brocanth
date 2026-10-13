//! Inverting a predicate into an audience, which is why the predicate is a
//! value.
//!
//! PLAN.md §10 is the whole reason this module exists:
//!
//! > PocketBase broadcasts to **all** connected clients in chunks … and for
//! > each client runs `SELECT (1) FROM t WHERE id = ? AND <rule>`. At a
//! > thousand connections with six hundred subscribers, one write is six
//! > hundred point queries: 1.2–6 ms on in-process SQLite, and **about 180 ms
//! > on Postgres**, which does not work at all.
//! >
//! > **A predicate you can inspect can be inverted into an audience. A string
//! > you can only execute.**
//!
//! §10 names the four coarse partitions most policies use — public, any
//! authenticated subject, the owner, members of a team — and all four are
//! shapes here.
//!
//! # How `brocanth-realtime` is meant to use this
//!
//! A subscriber's policy, applied to that subscriber, yields a [`Pred`].
//! Inverting it yields an [`Audience`], which is a set of
//! `(column, value)` keys. Index subscribers by those keys. When a row
//! changes, read the row's value for each column any subscriber keyed on, and
//! look the key up. One write becomes a map lookup rather than six hundred
//! point queries.
//!
//! ```text
//! subscriber A  Pred::eq(AuthorId, 7)            -> Keyed(author_id, 7)
//! subscriber B  Pred::eq(TeamId, 3)              -> Keyed(team_id, 3)
//! subscriber C  Pred::Always                     -> Everyone
//! subscriber D  Pred::negate(Pred::eq(Status, "x")) -> NotInvertible: a negation
//!
//! a post changes, author_id = 7, team_id = 3
//!   -> A (key hit), B (key hit), C (everyone), D (one point query)
//! ```
//!
//! # What is not invertible, and what that costs
//!
//! An ordering comparison partitions a range rather than an audience. A
//! negation describes a complement, which is every key but one and therefore
//! no cheaper to index than to evaluate. An [`crate::Opaque`] fragment is SQL
//! this crate cannot read at all. Each returns [`NotInvertible`] with the
//! reason, and §10's own answer applies: *"Only an opaque policy falls back to
//! per-subscriber evaluation."* The fallback is correct, just slow, and the
//! reason string is what tells an operator which policy is costing them.
//!
//! This is tier one, so the inversion is structural and conservative: it says
//! yes only where the answer is certain. A later version can invert more
//! shapes without any caller changing, because "cannot invert" already has a
//! defined meaning.

use crate::entity::{Column, Ident};
use crate::pred::{Op, Pred};
use crate::value::Value;

/// The set of rows a predicate admits, expressed as keys rather than as SQL.
#[derive(Clone, Debug, PartialEq)]
pub enum Audience<C: Column> {
    /// Every row. §10's "public".
    Everyone,
    /// No row. An anonymous subject under a policy that denied them.
    NoOne,
    /// Rows whose `column` holds `value`. §10's "the owner", and also "the
    /// team" and "published".
    Keyed {
        /// The column to key on.
        column: C,
        /// The value to key on.
        value: Value,
    },
    /// Rows in a link table. §10's "members of a team", for the case where
    /// membership is not a column on the row itself.
    ///
    /// Not a map lookup on its own — the realtime layer still has to know who
    /// is in the group — but it names the group, which is what lets a layer
    /// that caches memberships answer without touching the row's table.
    Membership {
        /// The column on the entity the link table points at.
        local: C,
        /// The link table.
        table: Ident,
        /// The column on the link table matching `local`.
        foreign: Ident,
        /// The constraints that identify the subject in the link table.
        filter: Vec<(Ident, Value)>,
    },
    /// Every part must admit the row.
    AllOf(Vec<Audience<C>>),
    /// Any part admits the row.
    AnyOf(Vec<Audience<C>>),
}

impl<C: Column> Audience<C> {
    /// Every row, no key needed.
    pub fn is_everyone(&self) -> bool {
        matches!(self, Audience::Everyone)
    }

    /// No row; the subscriber can be skipped entirely.
    pub fn is_no_one(&self) -> bool {
        matches!(self, Audience::NoOne)
    }

    /// The single key, when this audience is exactly one.
    ///
    /// The common case, and the one worth a fast path: "the owner".
    pub fn key(&self) -> Option<(C, &Value)> {
        match self {
            Audience::Keyed { column, value } => Some((*column, value)),
            _ => None,
        }
    }

    /// Every column this audience keys on, including inside `AllOf` and
    /// `AnyOf`.
    ///
    /// What a realtime index reads off a changed row.
    pub fn columns(&self) -> Vec<C> {
        let mut out = Vec::new();
        self.collect_columns(&mut out);
        out
    }

    fn collect_columns(&self, out: &mut Vec<C>) {
        match self {
            Audience::Keyed { column, .. } | Audience::Membership { local: column, .. } => {
                if !out.contains(column) {
                    out.push(*column);
                }
            }
            Audience::AllOf(parts) | Audience::AnyOf(parts) => {
                for part in parts {
                    part.collect_columns(out);
                }
            }
            Audience::Everyone | Audience::NoOne => {}
        }
    }
}

/// Why a predicate could not be turned into an audience.
///
/// Not an error: the caller's answer is to evaluate per subscriber, which is
/// correct. The `reason` is for the operator who wants to know which policy is
/// costing them a query per subscriber.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotInvertible {
    /// What stopped the inversion, in a phrase.
    pub reason: &'static str,
}

impl<C: Column> Pred<C> {
    /// Invert this predicate into the audience it admits.
    ///
    /// `Err` means per-subscriber evaluation, with a reason. See the module
    /// documentation.
    pub fn audience(&self) -> Result<Audience<C>, NotInvertible> {
        match self {
            Pred::Always => Ok(Audience::Everyone),
            Pred::Never => Ok(Audience::NoOne),

            // `col = NULL` is three-valued and admits nothing, so the audience
            // is empty rather than un-invertible.
            Pred::Cmp {
                op: Op::Eq | Op::Ne | Op::Lt | Op::Le | Op::Gt | Op::Ge,
                value: Value::Null,
                ..
            } => Ok(Audience::NoOne),

            Pred::Cmp {
                column,
                op: Op::Eq | Op::Is,
                value,
            } => Ok(Audience::Keyed {
                column: *column,
                value: value.clone(),
            }),

            Pred::Cmp { op: Op::IsNot, .. } | Pred::Cmp { op: Op::Ne, .. } => Err(NotInvertible {
                reason: "an inequality describes a complement, which is every key but one",
            }),

            Pred::Cmp { .. } => Err(NotInvertible {
                reason: "an ordering comparison partitions a range, not an audience",
            }),

            Pred::In {
                column,
                values,
                negated: false,
            } => Ok(Audience::AnyOf(
                values
                    .iter()
                    .map(|value| Audience::Keyed {
                        column: *column,
                        value: value.clone(),
                    })
                    .collect(),
            )),

            Pred::In { negated: true, .. } => Err(NotInvertible {
                reason: "`NOT IN` describes a complement, which is every key but a few",
            }),

            Pred::All(parts) => Ok(Audience::AllOf(
                parts
                    .iter()
                    .map(Pred::audience)
                    .collect::<Result<Vec<_>, _>>()?,
            )),

            Pred::Any(parts) => Ok(Audience::AnyOf(
                parts
                    .iter()
                    .map(Pred::audience)
                    .collect::<Result<Vec<_>, _>>()?,
            )),

            Pred::Not(_) => Err(NotInvertible {
                reason: "a negation describes a complement, which is no cheaper to index",
            }),

            Pred::Exists(membership) if !membership.negated => {
                // An ordering comparison inside the link-table filter means the
                // group is not a single group, so only equalities invert.
                let mut filter = Vec::with_capacity(membership.filter.len());
                for (column, op, value) in &membership.filter {
                    if !matches!(op, Op::Eq | Op::Is) {
                        return Err(NotInvertible {
                            reason: "the membership filter is not a plain equality",
                        });
                    }
                    filter.push((*column, value.clone()));
                }
                Ok(Audience::Membership {
                    local: membership.local,
                    table: membership.table,
                    foreign: membership.foreign,
                    filter,
                })
            }

            Pred::Exists(_) => Err(NotInvertible {
                reason: "`NOT EXISTS` describes everyone outside a group",
            }),

            Pred::Opaque(fragment) => Err(NotInvertible {
                reason: fragment.reason,
            }),
        }
    }
}
