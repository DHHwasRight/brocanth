//! `brocanth_policy::Audience`, with the column type erased, plus the plan for
//! indexing it.
//!
//! # Why a mirror type
//!
//! `Audience<C>` is generic over one entity's column enum. A broker serves
//! many entities through one index, so the type parameter cannot survive into
//! the `HashMap`. [`Reach`] is the same six shapes with `C` replaced by
//! `C::name()`, built by [`Reach::of`] at subscribe time — the one place the
//! typed value is still in hand.
//!
//! Nothing is lost by the erasure. A column name is `&'static str` because
//! `Column::name` returns one, so the erased form allocates nothing and
//! compares by pointer-or-bytes rather than by vtable.
//!
//! # The plan
//!
//! [`Reach::plan`] answers the only question the index asks: *which key do I
//! file this subscription under, and is a hit on that key conclusive?*
//!
//! | shape | plan | cost per matching write |
//! |---|---|---|
//! | `Everyone` | [`Plan::Everyone`] | nothing; no row is read |
//! | `NoOne` | [`Plan::Never`] | nothing; never delivered |
//! | `Keyed` | [`Plan::Keys`], conclusive | one map lookup |
//! | `AnyOf` of `Keyed` | [`Plan::Keys`], conclusive | one map lookup per column |
//! | `AllOf` with a `Keyed` part | [`Plan::Keys`], confirmed | one lookup, then an in-memory check |
//! | anything else | [`Plan::Unindexed`] | an in-memory check per subscriber |
//!
//! [`Plan::Unindexed`] is still far from PocketBase: the check is
//! `Reach::admits` against a row that was read **once for the whole publish**,
//! not a `SELECT` per subscriber. The one thing it is not is a map lookup, and
//! [`Delivery::scanned`](crate::Delivery) counts exactly how many subscribers
//! are paying for that.
//!
//! # `AnyOf` is conclusive only when every part is
//!
//! A union indexed on the keys of some of its parts would miss a row that
//! matches only an unindexable part. So `AnyOf` is keyed only when every part
//! reduces to plain keys, and otherwise falls to [`Plan::Unindexed`] whole.
//! `AllOf` is the opposite: every part is a **necessary** condition, so one
//! single-key part is a sound prefilter and the rest are confirmed in memory.

use brocanth_policy::{Audience, Column, Value};

/// A key an audience can be filed under.
///
/// [`Value`] has no `Eq` and no `Hash`, by `brocanth-policy`'s decision, and
/// the reason it gives is this exact use: *"a policy that keys an audience on
/// a float is a policy with a bug."* So [`ValueKey`] is the subset that is a
/// sound hash key, a float is not in it, and an audience keyed on one degrades
/// to [`Plan::Unindexed`] rather than being silently filed under a bit
/// pattern.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ValueKey {
    /// SQL `NULL`. Reachable only through the NULL-safe `IS`, because `= NULL`
    /// inverts to `NoOne`.
    Null,
    /// A boolean.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// Text.
    Text(String),
    /// A blob.
    Bytes(Vec<u8>),
}

impl ValueKey {
    /// The key for a value, or `None` for a float.
    pub fn of(value: &Value) -> Option<ValueKey> {
        Some(match value {
            Value::Null => ValueKey::Null,
            Value::Bool(b) => ValueKey::Bool(*b),
            Value::Int(n) => ValueKey::Int(*n),
            Value::Text(s) => ValueKey::Text(s.clone()),
            Value::Bytes(b) => ValueKey::Bytes(b.clone()),
            Value::Float(_) => return None,
        })
    }
}

/// A link table an audience names, with the column type erased.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// The column on the changed row the link table points at.
    pub local: &'static str,
    /// The link table.
    pub table: &'static str,
    /// The column on the link table matching `local`.
    pub foreign: &'static str,
    /// What identifies the subject in the link table.
    pub filter: Vec<(&'static str, Value)>,
}

/// Whoever can answer "is this row's group one of mine?".
///
/// `brocanth_policy::Audience::Membership` *names* a group without resolving
/// it, and its own documentation says why: *"not a map lookup on its own — the
/// realtime layer still has to know who is in the group — but it names the
/// group, which is what lets a layer that caches memberships answer without
/// touching the row's table."* This is that layer's interface.
///
/// `None` means "I do not know", and the broker denies, counts it in
/// [`Delivery::undecidable`](crate::Delivery) and moves on. It never guesses
/// and it never blocks the broadcaster to go and find out.
pub trait GroupIndex: Send + Sync {
    /// Is `member` — the changed row's value for `group.local` — inside the
    /// group this filter names?
    fn contains(&self, group: &Group, member: &Value) -> Option<bool>;
}

/// The group index for an application that installed none.
///
/// Every membership audience is undecidable, so a subscription that needs one
/// is denied and counted. Visible, rather than silently wrong in either
/// direction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UnknownGroups;

impl GroupIndex for UnknownGroups {
    fn contains(&self, _group: &Group, _member: &Value) -> Option<bool> {
        None
    }
}

impl<F> GroupIndex for F
where
    F: Fn(&Group, &Value) -> Option<bool> + Send + Sync,
{
    fn contains(&self, group: &Group, member: &Value) -> Option<bool> {
        self(group, member)
    }
}

/// `brocanth_policy::Audience` with the column type erased to its name.
#[derive(Clone, Debug, PartialEq)]
pub enum Reach {
    /// Every row.
    Everyone,
    /// No row.
    NoOne,
    /// Rows whose column holds this value.
    Keyed {
        /// The column to key on.
        column: &'static str,
        /// The value to key on.
        value: Value,
    },
    /// Rows whose group the subject belongs to.
    Group(Group),
    /// Every part must admit.
    AllOf(Vec<Reach>),
    /// Any part admits.
    AnyOf(Vec<Reach>),
}

/// Whether a row reaches a subscriber, with the third answer SQL insists on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Deliver.
    Admit,
    /// Do not deliver.
    Deny,
    /// Nobody could say. The broker denies and counts it.
    Undecidable {
        /// What was missing, for whoever reads the counter and wants to know.
        reason: &'static str,
    },
}

impl Verdict {
    /// Collapse towards denial, in the one direction `brocanth-policy`
    /// collapses.
    pub fn admits(self) -> bool {
        matches!(self, Verdict::Admit)
    }

    fn and(self, other: Verdict) -> Verdict {
        match (self, other) {
            (Verdict::Deny, _) | (_, Verdict::Deny) => Verdict::Deny,
            (Verdict::Undecidable { reason }, _) | (_, Verdict::Undecidable { reason }) => {
                Verdict::Undecidable { reason }
            }
            (Verdict::Admit, Verdict::Admit) => Verdict::Admit,
        }
    }

    fn or(self, other: Verdict) -> Verdict {
        match (self, other) {
            (Verdict::Admit, _) | (_, Verdict::Admit) => Verdict::Admit,
            (Verdict::Undecidable { reason }, _) | (_, Verdict::Undecidable { reason }) => {
                Verdict::Undecidable { reason }
            }
            (Verdict::Deny, Verdict::Deny) => Verdict::Deny,
        }
    }
}

/// How the index files a subscription, and what a hit means.
#[derive(Clone, Debug, PartialEq)]
pub enum Plan {
    /// No key, no row, no check. Delivered to on every change of the topic.
    Everyone,
    /// Never delivered to. Kept in the registry so an operator can see that a
    /// subscriber is registered and receiving nothing.
    Never,
    /// Filed under these `(column, key)` pairs.
    Keys {
        /// Where to file it. A hit on any one of them is a candidate.
        keys: Vec<(&'static str, ValueKey)>,
        /// `true` when a hit is only a prefilter and the whole [`Reach`] still
        /// has to be checked against the row.
        confirm: bool,
    },
    /// No sound key. Checked against the row, per subscriber, in memory.
    Unindexed {
        /// Why there is no key, for the operator who wants to fix it.
        reason: &'static str,
    },
}

impl Reach {
    /// Erase a typed audience.
    pub fn of<C: Column>(audience: &Audience<C>) -> Reach {
        match audience {
            Audience::Everyone => Reach::Everyone,
            Audience::NoOne => Reach::NoOne,
            Audience::Keyed { column, value } => Reach::Keyed {
                column: column.name(),
                value: value.clone(),
            },
            Audience::Membership {
                local,
                table,
                foreign,
                filter,
            } => Reach::Group(Group {
                local: local.name(),
                table: table.as_str(),
                foreign: foreign.as_str(),
                filter: filter
                    .iter()
                    .map(|(column, value)| (column.as_str(), value.clone()))
                    .collect(),
            }),
            Audience::AllOf(parts) => Reach::AllOf(parts.iter().map(Reach::of).collect()),
            Audience::AnyOf(parts) => Reach::AnyOf(parts.iter().map(Reach::of).collect()),
        }
    }

    /// Every column a changed row must be read for before this reach can be
    /// decided. The erased twin of `Audience::columns`.
    pub fn columns(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        self.collect_columns(&mut out);
        out
    }

    fn collect_columns(&self, out: &mut Vec<&'static str>) {
        match self {
            Reach::Keyed { column, .. } => push_once(out, column),
            Reach::Group(group) => push_once(out, group.local),
            Reach::AllOf(parts) | Reach::AnyOf(parts) => {
                for part in parts {
                    part.collect_columns(out);
                }
            }
            Reach::Everyone | Reach::NoOne => {}
        }
    }

    /// Where the index files this reach, and whether a hit is conclusive.
    pub fn plan(&self) -> Plan {
        match self {
            Reach::Everyone => Plan::Everyone,
            Reach::NoOne => Plan::Never,
            Reach::Keyed { column, value } => match ValueKey::of(value) {
                Some(key) => Plan::Keys {
                    keys: vec![(*column, key)],
                    confirm: false,
                },
                None => Plan::Unindexed {
                    reason: "an audience keyed on a float is not a sound index key",
                },
            },
            Reach::Group(_) => Plan::Unindexed {
                reason: "a membership audience names a group, which is not a column of the row",
            },

            // A union is keyed only when every part is, because a key set that
            // covers some of the parts would miss a row matching only the rest.
            Reach::AnyOf(parts) => {
                let mut keys = Vec::new();
                for part in parts {
                    match part.plan() {
                        Plan::Everyone => return Plan::Everyone,
                        Plan::Never => {}
                        Plan::Keys {
                            keys: mut part_keys,
                            confirm: false,
                        } => keys.append(&mut part_keys),
                        Plan::Keys { .. } => {
                            return Plan::Unindexed {
                                reason: "a union containing a conjunction has no conclusive key",
                            }
                        }
                        Plan::Unindexed { reason } => return Plan::Unindexed { reason },
                    }
                }
                if keys.is_empty() {
                    Plan::Never
                } else {
                    Plan::Keys {
                        keys,
                        confirm: false,
                    }
                }
            }

            // Every part is necessary, so one single-key part is a sound
            // prefilter and the rest are confirmed against the same row.
            Reach::AllOf(parts) => {
                let mut prefilter = None;
                let mut all_everyone = true;
                for part in parts {
                    match part.plan() {
                        Plan::Never => return Plan::Never,
                        Plan::Everyone => {}
                        Plan::Keys { keys, confirm } => {
                            all_everyone = false;
                            if prefilter.is_none() && !confirm && keys.len() == 1 {
                                prefilter = Some(keys);
                            }
                        }
                        Plan::Unindexed { .. } => all_everyone = false,
                    }
                }
                match (prefilter, all_everyone) {
                    (Some(keys), _) => Plan::Keys {
                        keys,
                        confirm: true,
                    },
                    (None, true) => Plan::Everyone,
                    (None, false) => Plan::Unindexed {
                        reason: "a conjunction with no single-column equality has no prefilter",
                    },
                }
            }
        }
    }

    /// Does this row reach the subscriber?
    ///
    /// In memory, against a row that was read once for the whole publish.
    /// Nothing here opens a connection: a group it cannot resolve is
    /// [`Verdict::Undecidable`] rather than a query.
    pub fn admits(&self, row: &crate::Row, groups: &dyn GroupIndex) -> Verdict {
        match self {
            Reach::Everyone => Verdict::Admit,
            Reach::NoOne => Verdict::Deny,
            Reach::Keyed { column, value } => match row.get(column) {
                // Not `Value::Null == Value::Null`: this is the NULL-safe
                // comparison `Audience::Keyed` was inverted from, since a plain
                // `= NULL` inverts to `NoOne` before it ever gets here.
                Some(found) => {
                    if found == value {
                        Verdict::Admit
                    } else {
                        Verdict::Deny
                    }
                }
                None => Verdict::Undecidable {
                    reason: "the changed row was read without a column the audience keys on",
                },
            },
            Reach::Group(group) => match row.get(group.local) {
                Some(member) => match groups.contains(group, member) {
                    Some(true) => Verdict::Admit,
                    Some(false) => Verdict::Deny,
                    None => Verdict::Undecidable {
                        reason: "no group index could say whether the row is in the group",
                    },
                },
                None => Verdict::Undecidable {
                    reason: "the changed row was read without the column the group links on",
                },
            },
            Reach::AllOf(parts) => parts.iter().fold(Verdict::Admit, |acc, part| {
                acc.and(part.admits(row, groups))
            }),
            Reach::AnyOf(parts) => parts
                .iter()
                .fold(Verdict::Deny, |acc, part| acc.or(part.admits(row, groups))),
        }
    }
}

fn push_once(out: &mut Vec<&'static str>, column: &'static str) {
    if !out.contains(&column) {
        out.push(column);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Row;
    use brocanth_policy::{Ident, Pred};

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum Post {
        AuthorId,
        TeamId,
        Status,
        Score,
    }

    impl Column for Post {
        const ALL: &'static [Self] = &[Post::AuthorId, Post::TeamId, Post::Status, Post::Score];
        fn name(&self) -> &'static str {
            match self {
                Post::AuthorId => "author_id",
                Post::TeamId => "team_id",
                Post::Status => "status",
                Post::Score => "score",
            }
        }
    }

    fn reach(pred: Pred<Post>) -> Reach {
        Reach::of(&pred.audience().expect("invertible"))
    }

    #[test]
    fn one_equality_is_one_key() {
        let plan = reach(Pred::eq(Post::AuthorId, 7i64)).plan();
        assert_eq!(
            plan,
            Plan::Keys {
                keys: vec![("author_id", ValueKey::Int(7))],
                confirm: false
            }
        );
    }

    #[test]
    fn a_union_of_equalities_is_several_keys() {
        let pred = Pred::any([
            Pred::eq(Post::Status, "published"),
            Pred::eq(Post::AuthorId, 7i64),
        ]);
        let Plan::Keys { keys, confirm } = reach(pred).plan() else {
            panic!("expected keys")
        };
        assert!(!confirm);
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn a_conjunction_keys_on_one_part_and_confirms_the_rest() {
        let pred = Pred::all([
            Pred::eq(Post::TeamId, 3i64),
            Pred::eq(Post::Status, "published"),
        ]);
        let plan = reach(pred).plan();
        let Plan::Keys { keys, confirm } = plan else {
            panic!("expected keys")
        };
        assert!(confirm, "a conjunction's key is a prefilter, not an answer");
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn a_union_that_is_not_all_keys_has_no_key() {
        // `IN (…)` over a team, OR membership of a group: the group part has no
        // column key, so keying on the team alone would miss rows.
        let group = brocanth_policy::Membership::new(
            Post::TeamId,
            Ident::must("team_members"),
            Ident::must("team_id"),
        )
        .filter_by(Ident::must("user_id"), brocanth_policy::Op::Eq, 7i64)
        .expect("a definite subject");
        let pred = Pred::any([Pred::eq(Post::Status, "published"), group.into_pred()]);
        assert!(matches!(reach(pred).plan(), Plan::Unindexed { .. }));
    }

    #[test]
    fn a_float_key_is_refused_rather_than_hashed() {
        let plan = reach(Pred::eq(Post::Score, 1.5f64)).plan();
        assert!(matches!(plan, Plan::Unindexed { .. }));
        assert_eq!(ValueKey::of(&Value::Float(1.5)), None);
    }

    #[test]
    fn admits_reads_the_row_and_denies_what_it_cannot_read() {
        let reach = reach(Pred::eq(Post::AuthorId, 7i64));
        assert_eq!(
            reach.admits(&Row::new().with("author_id", 7i64), &UnknownGroups),
            Verdict::Admit
        );
        assert_eq!(
            reach.admits(&Row::new().with("author_id", 8i64), &UnknownGroups),
            Verdict::Deny
        );
        let verdict = reach.admits(&Row::new(), &UnknownGroups);
        assert!(matches!(verdict, Verdict::Undecidable { .. }));
        assert!(!verdict.admits());
    }

    #[test]
    fn a_group_is_undecidable_without_an_index_and_decidable_with_one() {
        let group = brocanth_policy::Membership::new(
            Post::TeamId,
            Ident::must("team_members"),
            Ident::must("team_id"),
        )
        .filter_by(Ident::must("user_id"), brocanth_policy::Op::Eq, 7i64)
        .expect("a definite subject");
        let reach = reach(group.into_pred());
        let row = Row::new().with("team_id", 3i64);
        assert!(matches!(
            reach.admits(&row, &UnknownGroups),
            Verdict::Undecidable { .. }
        ));
        let mine = |g: &Group, member: &Value| {
            assert_eq!(g.table, "team_members");
            Some(*member == Value::Int(3))
        };
        assert_eq!(reach.admits(&row, &mine), Verdict::Admit);
    }
}
