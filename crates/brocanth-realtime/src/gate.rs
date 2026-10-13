//! What stands between a change and a subscriber, and the two ways it can.
//!
//! A [`Subscription`] is a [`Topic`] and a [`Gate`]. The gate is where §10's
//! whole argument lands:
//!
//! > **A predicate you can inspect can be inverted into an audience. A string
//! > you can only execute.**
//!
//! [`Gate::Audience`] is the inverted form: a [`Reach`] the broker files under
//! a map key. [`Gate::Evaluated`] is the fallback: the predicate itself,
//! boxed, run against the changed row once per subscriber. Both are correct.
//! Only one is cheap, and which one you got is in the return value of
//! [`Client::subscribe`](crate::Client::subscribe).
//!
//! # The fallback is not PocketBase's fallback
//!
//! PocketBase runs `SELECT (1) FROM t WHERE id = ? AND <rule>` per subscriber,
//! which on Postgres is §10's 180 ms. [`Gate::Evaluated`] runs
//! `brocanth_policy::Pred::eval` per subscriber — in memory, against a row the
//! broker read **once for the whole publish**. So even the bad path here is a
//! row read plus N comparisons rather than N round trips. It is still the bad
//! path, and [`Delivery::scanned`](crate::Delivery) counts the subscribers
//! paying for it.
//!
//! The one case with no in-memory answer is a predicate `eval` refuses: an
//! `EXISTS` over another table, or an opaque fragment with no evaluator. That
//! is [`Verdict::Undecidable`], the broker denies, and
//! [`Delivery::undecidable`](crate::Delivery) counts it. It does not silently
//! open a connection on the broadcast path, because the broadcaster must not
//! block.
//!
//! # Noticing that you landed here
//!
//! Three ways, deliberately redundant, because §10's cost model only works if
//! an accidentally opaque policy is a thing somebody finds:
//!
//! 1. [`Client::subscribe`](crate::Client::subscribe) returns [`Gating`], and
//!    [`Gating::Scanned`] carries the reason string
//!    `brocanth_policy::NotInvertible` recorded.
//! 2. [`Broker::audit`](crate::Broker::audit) lists every topic with an
//!    unindexed subscriber, the reason, and how many. One call, suitable for a
//!    `/up` page, an admin screen, or an assertion in a test.
//! 3. [`Delivery::scanned`](crate::Delivery) and
//!    [`Delivery::undecidable`](crate::Delivery) on every publish.

use std::sync::Arc;

use brocanth_policy::{Column, Entity, EvalError, Policy, Pred};

use crate::reach::{Plan, Reach, Verdict};
use crate::row::Row;
use crate::topic::Topic;

type EvalFn = dyn Fn(&Row) -> Verdict + Send + Sync;

/// How a subscriber's policy decides whether a change reaches them.
#[derive(Clone)]
pub enum Gate {
    /// The policy inverted. The fast path.
    Audience(Reach),
    /// The policy did not invert. Evaluated per subscriber, in memory.
    Evaluated {
        /// What stopped the inversion, verbatim from
        /// `brocanth_policy::NotInvertible`.
        reason: &'static str,
        /// Every column the predicate reads, so the broker knows what to ask
        /// the [`RowReader`](crate::RowReader) for.
        columns: Vec<&'static str>,
        /// The predicate, with its column type erased behind a closure.
        eval: Arc<EvalFn>,
    },
}

impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Gate::Audience(reach) => f.debug_tuple("Audience").field(reach).finish(),
            Gate::Evaluated {
                reason, columns, ..
            } => f
                .debug_struct("Evaluated")
                .field("reason", reason)
                .field("columns", columns)
                .finish_non_exhaustive(),
        }
    }
}

impl Gate {
    /// Admit everybody, with no row read and no check. What a public
    /// collection gets, and what a test uses when the policy is not the
    /// subject.
    pub fn everyone() -> Gate {
        Gate::Audience(Reach::Everyone)
    }

    /// Invert a predicate, or fall back to evaluating it.
    ///
    /// The one function that decides which path a subscriber is on, and the
    /// only place `brocanth_policy::Pred::audience` is called.
    ///
    /// `Send + Sync` on the column enum is asked for here and nowhere in
    /// `brocanth-policy`, because this is the first place a predicate crosses
    /// a thread: the fallback closure is held in the broker and run by
    /// whichever thread wrote the row. A `Column` is a fieldless enum, so both
    /// are automatic and the bound costs nothing to satisfy.
    pub fn of<C: Column + Send + Sync>(pred: Pred<C>) -> Gate {
        match pred.audience() {
            Ok(audience) => Gate::Audience(Reach::of(&audience)),
            Err(not_invertible) => Gate::Evaluated {
                reason: not_invertible.reason,
                columns: pred_columns(&pred),
                eval: Arc::new(move |row: &Row| match pred.eval(&row.view::<C>()) {
                    Ok(truth) => {
                        if truth.admits() {
                            Verdict::Admit
                        } else {
                            Verdict::Deny
                        }
                    }
                    Err(EvalError::NeedsDatabase { .. }) => Verdict::Undecidable {
                        reason: "the policy asks about another table, which the broadcaster \
                                 will not open a connection to answer",
                    },
                    Err(EvalError::Opaque { reason }) => Verdict::Undecidable { reason },
                }),
            },
        }
    }

    /// Every column a changed row must be read for.
    pub fn columns(&self) -> Vec<&'static str> {
        match self {
            Gate::Audience(reach) => reach.columns(),
            Gate::Evaluated { columns, .. } => columns.clone(),
        }
    }

    /// How the index will file this gate.
    pub fn plan(&self) -> Plan {
        match self {
            Gate::Audience(reach) => reach.plan(),
            Gate::Evaluated { reason, .. } => Plan::Unindexed { reason },
        }
    }

    /// Decide one subscriber against one row.
    pub fn admits(&self, row: &Row, groups: &dyn crate::GroupIndex) -> Verdict {
        match self {
            Gate::Audience(reach) => reach.admits(row, groups),
            Gate::Evaluated { eval, .. } => eval(row),
        }
    }
}

/// Every column a predicate reads.
///
/// An [`brocanth_policy::Opaque`] fragment is SQL this crate cannot read, so
/// the columns its evaluator touches are unknowable — and the answer is
/// `C::ALL` rather than a guess. Over-reading the row is a cost; under-reading
/// it would make the evaluator see `NULL` where there is a value and deny
/// somebody silently.
fn pred_columns<C: Column>(pred: &Pred<C>) -> Vec<&'static str> {
    fn walk<C: Column>(pred: &Pred<C>, out: &mut Vec<&'static str>) -> bool {
        let mut opaque = false;
        match pred {
            Pred::Always | Pred::Never => {}
            Pred::Cmp { column, .. } | Pred::In { column, .. } => push_once(out, column.name()),
            Pred::All(parts) | Pred::Any(parts) => {
                for part in parts {
                    opaque |= walk(part, out);
                }
            }
            Pred::Not(inner) => opaque |= walk(inner, out),
            Pred::Exists(membership) => push_once(out, membership.local.name()),
            Pred::Opaque(_) => opaque = true,
        }
        opaque
    }

    let mut out = Vec::new();
    if walk(pred, &mut out) {
        return C::ALL.iter().map(Column::name).collect();
    }
    out
}

fn push_once(out: &mut Vec<&'static str>, column: &'static str) {
    if !out.contains(&column) {
        out.push(column);
    }
}

/// One topic, gated one way.
#[derive(Clone, Debug)]
pub struct Subscription {
    topic: Topic,
    gate: Gate,
}

impl Subscription {
    /// A topic and a gate, chosen by the caller.
    pub fn new(topic: Topic, gate: Gate) -> Subscription {
        Subscription { topic, gate }
    }

    /// A topic gated by a predicate, inverted if it inverts.
    pub fn gated_by<C: Column + Send + Sync>(topic: Topic, pred: Pred<C>) -> Subscription {
        Subscription::new(topic, Gate::of(pred))
    }

    /// A topic gated by the policy rule that matches its kind.
    ///
    /// This is the method to use, because it is the one that cannot get the
    /// distinction wrong. PocketBase gates `collection/*` on the list rule and
    /// `collection/recordId` on the view rule; here the topic *is* the choice,
    /// so a caller cannot accidentally gate a wildcard on the narrower rule
    /// and leak a row the list hides.
    ///
    /// ```
    /// # use brocanth_policy::{Column, Entity, EntityPred, Ident, Policy, Pred};
    /// # use brocanth_realtime::{Subscription, Topic};
    /// # #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] enum PostColumn { Id, AuthorId }
    /// # impl Column for PostColumn {
    /// #     const ALL: &'static [Self] = &[Self::Id, Self::AuthorId];
    /// #     fn name(&self) -> &'static str { match self { Self::Id => "id", Self::AuthorId => "author_id" } }
    /// # }
    /// # struct Post;
    /// # impl Entity for Post {
    /// #     type Column = PostColumn;
    /// #     const TABLE: Ident = Ident::must("posts");
    /// #     const PRIMARY_KEY: Self::Column = PostColumn::Id;
    /// # }
    /// # struct PostPolicy;
    /// # impl Policy for PostPolicy {
    /// #     type Entity = Post;
    /// #     type Subject = i64;
    /// #     fn list(&self, who: &i64) -> EntityPred<Post> { Pred::eq(PostColumn::AuthorId, *who) }
    /// # }
    /// let topic = Topic::collection("posts").unwrap();
    /// let subscription = Subscription::from_policy(topic, &PostPolicy, &7);
    /// ```
    pub fn from_policy<P: Policy>(topic: Topic, policy: &P, subject: &P::Subject) -> Subscription
    where
        <P::Entity as Entity>::Column: Send + Sync,
    {
        let pred = if topic.is_wildcard() {
            policy.list(subject)
        } else {
            policy.view(subject)
        };
        Subscription::gated_by(topic, pred)
    }

    /// The topic.
    pub fn topic(&self) -> &Topic {
        &self.topic
    }

    /// The gate.
    pub fn gate(&self) -> &Gate {
        &self.gate
    }

    pub(crate) fn into_parts(self) -> (Topic, Gate) {
        (self.topic, self.gate)
    }
}

/// How the broker filed a subscription. Returned by
/// [`Client::subscribe`](crate::Client::subscribe), so the cost is visible at
/// the call site rather than in a metric nobody reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gating {
    /// Delivered to on every change of the topic. No row read, no check.
    Everyone,
    /// Never delivered to. The policy denied this subject outright.
    Never,
    /// Filed under this many keys; a hit is the answer.
    Indexed {
        /// How many `(column, value)` keys the subscription sits under.
        keys: usize,
    },
    /// Filed under one key as a prefilter; a hit is checked against the row.
    Prefiltered {
        /// How many keys the prefilter uses.
        keys: usize,
    },
    /// Not filed. Checked against the row on every change of the topic.
    ///
    /// **This is the slow path.** §10's whole argument is about not being
    /// here, and the reason says what to change.
    Scanned {
        /// Why there is no key.
        reason: &'static str,
    },
}

impl Gating {
    /// `true` when this subscription is checked rather than looked up.
    pub fn is_scanned(&self) -> bool {
        matches!(self, Gating::Scanned { .. })
    }

    /// Why this subscription is on the slow path, when it is.
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            Gating::Scanned { reason } => Some(reason),
            _ => None,
        }
    }

    pub(crate) fn of(plan: &Plan) -> Gating {
        match plan {
            Plan::Everyone => Gating::Everyone,
            Plan::Never => Gating::Never,
            Plan::Keys {
                keys,
                confirm: false,
            } => Gating::Indexed { keys: keys.len() },
            Plan::Keys {
                keys,
                confirm: true,
            } => Gating::Prefiltered { keys: keys.len() },
            Plan::Unindexed { reason } => Gating::Scanned { reason },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth_policy::{Ident, Opaque, Value};

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    enum Post {
        Id,
        AuthorId,
        Status,
    }

    impl Column for Post {
        const ALL: &'static [Self] = &[Post::Id, Post::AuthorId, Post::Status];
        fn name(&self) -> &'static str {
            match self {
                Post::Id => "id",
                Post::AuthorId => "author_id",
                Post::Status => "status",
            }
        }
    }

    #[test]
    fn an_equality_inverts_and_a_negation_does_not() {
        let indexed = Gate::of(Pred::eq(Post::AuthorId, 7i64));
        assert!(matches!(indexed, Gate::Audience(_)));
        assert_eq!(Gating::of(&indexed.plan()), Gating::Indexed { keys: 1 });

        let scanned = Gate::of(Pred::negate(Pred::eq(Post::Status, "draft")));
        let Gate::Evaluated { reason, .. } = &scanned else {
            panic!("a negation does not invert")
        };
        assert!(reason.contains("complement"), "{reason}");
        assert!(Gating::of(&scanned.plan()).is_scanned());
    }

    #[test]
    fn a_fallback_gate_reads_only_the_columns_its_predicate_names() {
        let gate = Gate::of(Pred::negate(Pred::eq(Post::Status, "draft")));
        assert_eq!(gate.columns(), vec!["status"]);
    }

    #[test]
    fn an_opaque_fragment_makes_the_gate_read_every_column() {
        let opaque = Pred::Opaque(Opaque::new("hand written", "1 = 1", Vec::new()));
        let gate = Gate::of(Pred::all([Pred::eq(Post::AuthorId, 1i64), opaque]));
        assert_eq!(gate.columns(), vec!["id", "author_id", "status"]);
    }

    #[test]
    fn a_fallback_gate_evaluates_the_predicate_it_could_not_invert() {
        let gate = Gate::of(Pred::negate(Pred::eq(Post::Status, "draft")));
        let groups = crate::UnknownGroups;
        assert_eq!(
            gate.admits(&Row::new().with("status", "published"), &groups),
            Verdict::Admit
        );
        assert_eq!(
            gate.admits(&Row::new().with("status", "draft"), &groups),
            Verdict::Deny
        );
        // `NOT Unknown` is `Unknown`, which collapses to a denial. The repair
        // of PLAN.md §9's vulnerability survives the trip through realtime.
        assert_eq!(
            gate.admits(&Row::new().with("status", Value::Null), &groups),
            Verdict::Deny
        );
    }

    #[test]
    fn a_predicate_about_another_table_is_undecidable_rather_than_a_query() {
        let membership = brocanth_policy::Membership::new(
            Post::AuthorId,
            Ident::must("team_members"),
            Ident::must("user_id"),
        )
        .negate();
        let gate = Gate::of(membership.into_pred());
        let verdict = gate.admits(&Row::new().with("author_id", 1i64), &crate::UnknownGroups);
        assert!(matches!(verdict, Verdict::Undecidable { .. }));
    }
}
