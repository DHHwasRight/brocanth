//! A policy's rules as data, before a subject is known.
//!
//! docs/policy-rls-bridge.md, decisions 1 and 2. A [`Pred`] is a rule after
//! Rust ran it: each guard took a branch, and `who.user_id` became the
//! literal `7`. The database must read `brocanth.user_id` instead, and must
//! follow the guards itself. So `policy!` with `#[db]` keeps the rule as a
//! tree, [`Rules`], with two readers:
//!
//! * [`Rule::resolve`] takes a subject's [`Attributes`], takes each guard's
//!   branch and folds each attribute into a [`Bound`]. The result is the
//!   [`Pred`] that `policy!` expands to today for the same subject, so
//!   `condition()` and `eval()` do not change.
//! * `brocanth-rls` walks the same tree and writes the database policy:
//!   each [`Operand::Attr`] becomes a setting read, each [`Guard`] a boolean
//!   over settings.
//!
//! One tree, two readers: the readers can differ, and the agreement test of
//! decision 10 checks where. Every type here is public, with public fields,
//! for the second reader.
//!
//! # The shape of one rule
//!
//! A [`Rule`] is a chain of guarded arms, then a fallback:
//!
//! ```text
//! list {
//!     who.banned => never,                      // arm: Guard::Attr("banned"), Term::Never
//!     who.admin => always,                      // arm: Guard::Attr("admin"), Term::Always
//!     status == "published"
//!         || author_id == who.user_id else deny // fallback: Term::Any([...])
//! }
//! ```
//!
//! # Absent guard attributes
//!
//! A guard reads a `bool` (or compares an attribute with a literal). A Rust
//! `bool` has no third value, but a setting does. So when any attribute that
//! a guard of the chain reads is absent, the whole rule is [`Pred::Never`],
//! in [`Rule::resolve`] and in the database (`s(g) IS NOT NULL AND …`). A
//! subject from `#[derive(Subject)]` always has its non-`Option` fields, so
//! this happens only to [`Attributes`] built by hand without the key.

use crate::entity::{Column, Entity, Ident};
use crate::mask::FieldMask;
use crate::pred::{Membership, Op, Pred};
use crate::subject::Attributes;
use crate::value::{Bound, Missing, Value};

/// The rules of one policy, one per command.
///
/// `view` and `update_check` are `None` when the policy does not write them.
/// [`Rules::view`] then reads `list`, and [`Rules::update_check`] reads
/// `update`. A reader that writes `list OR view` can test `view.is_none()`.
pub struct Rules<E: Entity> {
    /// Which rows a list shows.
    pub list: Rule<E::Column>,
    /// Which rows a read of one row shows. `None`: the `list` rule.
    pub view: Option<Rule<E::Column>>,
    /// Which proposed rows a create may write.
    pub create: Rule<E::Column>,
    /// Which stored rows an update may change.
    pub update: Rule<E::Column>,
    /// Which rows an update may leave: the database's `WITH CHECK`
    /// (decision 8). `None`: the `update` rule. The application does not
    /// read it; the `writable` mask does that job there.
    pub update_check: Option<Rule<E::Column>>,
    /// Which rows a delete may remove.
    pub delete: Rule<E::Column>,
    /// The `writable` mask as data (decision 9). `None`: the policy does
    /// not write one.
    pub writable: Option<MaskRule<E::Column>>,
    /// The `writable_on_create` mask as data. `None`: the policy does not
    /// write one, and [`Rules::writable_on_create`] reads `writable`.
    pub writable_on_create: Option<MaskRule<E::Column>>,
}

/// A write mask as data: the columns a write may set.
///
/// docs/policy-rls-bridge.md, decision 9. A mask is per subject, and a
/// database grant is per role. So the database can hold a mask only when it
/// is the same for every subject: [`MaskRule::Fixed`]. `brocanth-rls` then
/// grants the role these columns and no others.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MaskRule<C: Column> {
    /// No guard: the same mask for every subject.
    Fixed(FieldMask<C>),
    /// A guard or a `{ … }` block chooses the mask per subject. It stays in
    /// the application.
    PerSubject,
}

impl<C: Column> MaskRule<C> {
    /// The columns the mask permits, in declaration order. `None` when the
    /// database cannot hold the mask as a column list: [`MaskRule::PerSubject`],
    /// and [`FieldMask::All`], which is the whole table.
    pub fn columns(&self) -> Option<Vec<C>> {
        match self {
            MaskRule::Fixed(FieldMask::All) | MaskRule::PerSubject => None,
            MaskRule::Fixed(mask) => Some(
                C::ALL
                    .iter()
                    .copied()
                    .filter(|c| mask.permits(*c))
                    .collect(),
            ),
        }
    }
}

/// A policy whose rules are data, so the database can enforce them too.
///
/// `policy!` with `#[db]` implements it, and implements [`crate::Policy`]
/// by resolving these rules with the subject's [`crate::Attributes`].
/// `brocanth-rls` reads [`DbPolicy::rules`] to write the database policies.
/// A hand-written `impl Policy` does not implement it (bridge decision 1).
pub trait DbPolicy: crate::Policy<Subject: crate::Subject> {
    /// The rules, before any subject is known.
    fn rules() -> Rules<Self::Entity>;
}

impl<E: Entity> Rules<E> {
    /// Every command denies, as the [`crate::Policy`] defaults do. Set the
    /// fields to open one.
    pub fn deny_all() -> Self {
        Rules {
            list: Rule::never(),
            view: None,
            create: Rule::never(),
            update: Rule::never(),
            update_check: None,
            delete: Rule::never(),
            writable: None,
            writable_on_create: None,
        }
    }

    /// The `view` rule, or `list` when the policy has none.
    pub fn view(&self) -> &Rule<E::Column> {
        self.view.as_ref().unwrap_or(&self.list)
    }

    /// The `update_check` rule, or `update` when the policy has none.
    pub fn update_check(&self) -> &Rule<E::Column> {
        self.update_check.as_ref().unwrap_or(&self.update)
    }

    /// The `writable_on_create` mask, or `writable` when the policy has
    /// none, as [`crate::Policy::writable_on_create`] defaults.
    pub fn writable_on_create(&self) -> Option<&MaskRule<E::Column>> {
        self.writable_on_create.as_ref().or(self.writable.as_ref())
    }
}

impl<E: Entity> Clone for Rules<E> {
    fn clone(&self) -> Self {
        Rules {
            list: self.list.clone(),
            view: self.view.clone(),
            create: self.create.clone(),
            update: self.update.clone(),
            update_check: self.update_check.clone(),
            delete: self.delete.clone(),
            writable: self.writable.clone(),
            writable_on_create: self.writable_on_create.clone(),
        }
    }
}

impl<E: Entity> std::fmt::Debug for Rules<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rules")
            .field("table", &E::TABLE)
            .field("list", &self.list)
            .field("view", &self.view)
            .field("create", &self.create)
            .field("update", &self.update)
            .field("update_check", &self.update_check)
            .field("delete", &self.delete)
            .field("writable", &self.writable)
            .field("writable_on_create", &self.writable_on_create)
            .finish()
    }
}

impl<E: Entity> PartialEq for Rules<E> {
    fn eq(&self, other: &Self) -> bool {
        self.list == other.list
            && self.view == other.view
            && self.create == other.create
            && self.update == other.update
            && self.update_check == other.update_check
            && self.delete == other.delete
            && self.writable == other.writable
            && self.writable_on_create == other.writable_on_create
    }
}

/// One command's rule: guarded arms, tried in order, then a fallback.
///
/// `g1 => t1, g2 => t2, f` means: `t1` when `g1`, else `t2` when `g2`, else
/// `f`. This is `if g1 { return t1; } if g2 { return t2; } f`, which is
/// what `policy!` writes in Rust.
///
/// # App-only rules
///
/// `#[app_only("reason")]` on a rule of a `#[db]` policy keeps that rule in
/// Rust (bridge decision 4). Its `Rule` is [`Rule::app_only`]: no arms,
/// [`Term::Never`], and [`Rule::app_only`] set to the reason. A reader that
/// writes a database policy must test [`Rule::app_only`] first and refuse
/// the command until a hand-written expression replaces it. It must not
/// read the arms or the fallback of such a rule: they are not the rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule<C: Column> {
    /// The guarded arms, in source order.
    pub arms: Vec<(Guard, Term<C>)>,
    /// The term when no guard holds. [`Term::Never`] when the source has no
    /// fallback.
    pub fallback: Term<C>,
    /// `Some(reason)` when the rule is Rust code that only the application
    /// runs: `#[app_only("reason")]`. The arms and the fallback are then
    /// empty and mean nothing.
    pub app_only: Option<&'static str>,
}

impl<C: Column> Rule<C> {
    /// A rule with no arms.
    pub fn new(fallback: Term<C>) -> Self {
        Rule {
            arms: Vec::new(),
            fallback,
            app_only: None,
        }
    }

    /// A rule that only the application can run, with the author's reason.
    ///
    /// [`Rule::resolve`] gives [`Pred::Never`] for it: the data does not
    /// hold the rule, so it fails closed. `policy!` does not call it for
    /// such a rule; the `Policy` method runs the Rust code instead.
    pub fn app_only(reason: &'static str) -> Self {
        Rule {
            arms: Vec::new(),
            fallback: Term::Never,
            app_only: Some(reason),
        }
    }

    /// A rule that matches no row: no arms, [`Term::Never`].
    pub fn never() -> Self {
        Self::new(Term::Never)
    }

    /// The same rule, with one more arm after the others.
    pub fn arm(mut self, guard: Guard, term: Term<C>) -> Self {
        self.arms.push((guard, term));
        self
    }

    /// The keys every guard of the chain reads, each once, in source order.
    ///
    /// The database requires each of them to be set (the `IS NOT NULL`
    /// prefix), and so does [`Rule::resolve`].
    pub fn guard_keys(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for (guard, _) in &self.arms {
            guard.collect_keys(&mut out);
        }
        out
    }

    /// The predicate for one subject.
    ///
    /// Takes the first arm whose guard holds, or the fallback, and resolves
    /// its term with [`Term::resolve`]. When an attribute any guard of the
    /// chain reads is absent, the rule is [`Pred::Never`]: see the module
    /// documentation. So is a guard with no answer ([`Guard::holds`] is
    /// `None`).
    ///
    /// An [`Rule::app_only`] rule is [`Pred::Never`].
    pub fn resolve(&self, attrs: &Attributes) -> Pred<C> {
        if self.app_only.is_some() {
            return Pred::Never;
        }
        if self.guard_keys().iter().any(|k| attrs.get(k).is_none()) {
            return Pred::Never;
        }
        for (guard, term) in &self.arms {
            match guard.holds(attrs) {
                Some(true) => return term.resolve(attrs),
                Some(false) => {}
                None => return Pred::Never,
            }
        }
        self.fallback.resolve(attrs)
    }
}

/// A guard: a boolean over the subject's attributes, and nothing else.
///
/// Decision 3 refuses any other guard with `#[db]`: the database can test a
/// setting, not run Rust.
#[derive(Clone, Debug, PartialEq)]
pub enum Guard {
    /// `who.f`: the `boolean` attribute `f` is true.
    Attr(&'static str),
    /// `who.f == lit`.
    Eq(&'static str, Value),
    /// `!g`.
    Not(Box<Guard>),
    /// `g1 && g2 && …`. Empty is true.
    All(Vec<Guard>),
    /// `g1 || g2 || …`. Empty is false.
    Any(Vec<Guard>),
}

impl Guard {
    /// Whether the guard holds for these attributes. `None` when an
    /// attribute it reads is absent, or when [`Guard::Attr`] reads a value
    /// that is not a boolean.
    ///
    /// Every attribute is read, even after the answer is known, so a guard
    /// with an absent attribute is `None` whatever the others say.
    pub fn holds(&self, attrs: &Attributes) -> Option<bool> {
        match self {
            Guard::Attr(key) => match attrs.get(key)? {
                Value::Bool(b) => Some(*b),
                // Not a boolean: no answer, so the rule fails closed.
                // `policy!` refuses this at compile time.
                _ => None,
            },
            Guard::Eq(key, lit) => Some(attrs.get(key)? == lit),
            Guard::Not(inner) => inner.holds(attrs).map(|b| !b),
            Guard::All(parts) => parts
                .iter()
                .map(|g| g.holds(attrs))
                .try_fold(true, |acc, b| Some(acc & b?)),
            Guard::Any(parts) => parts
                .iter()
                .map(|g| g.holds(attrs))
                .try_fold(false, |acc, b| Some(acc | b?)),
        }
    }

    /// The keys this guard reads, each once, in source order.
    pub fn keys(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        self.collect_keys(&mut out);
        out
    }

    fn collect_keys(&self, out: &mut Vec<&'static str>) {
        match self {
            Guard::Attr(key) | Guard::Eq(key, _) => {
                if !out.contains(key) {
                    out.push(key);
                }
            }
            Guard::Not(inner) => inner.collect_keys(out),
            Guard::All(parts) | Guard::Any(parts) => {
                parts.iter().for_each(|g| g.collect_keys(out));
            }
        }
    }
}

/// The right side of a comparison: a literal, or a subject attribute.
#[derive(Clone, Debug, PartialEq)]
pub enum Operand {
    /// A literal. [`Value::Null`] with [`Op::Is`] is `col IS NULL`.
    Value(Value),
    /// `who.<key>`, with what its absence means.
    ///
    /// `policy!` writes [`Missing::Deny`] for a non-`Option` field: the
    /// field is always there, and the database reads a missing setting as
    /// a deny (decision 3's table). [`Missing::Allow`] exists in the
    /// application; `#[db]` refuses it.
    Attr {
        /// The attribute's key: the field name.
        key: &'static str,
        /// `else deny`, `else null` (or `else allow`).
        missing: Missing,
    },
}

impl Operand {
    /// The bound for these attributes: the literal, or the attribute's value
    /// with its absence mode. The same [`Bound`] that `typed::bind` or
    /// `typed::absent` gives in the `policy!` expansion.
    pub fn bound(&self, attrs: &Attributes) -> Bound {
        match self {
            Operand::Value(v) => Bound::of(v.clone()),
            Operand::Attr { key, missing } => Bound::or(attrs.get(key).cloned(), *missing),
        }
    }
}

/// A predicate whose values can be subject attributes.
///
/// The variants follow [`Pred`]'s, minus `Opaque` (decision 3 refuses a
/// `{ }` block with `#[db]`). Build it with the variants: [`Term::resolve`]
/// does the folding that [`Pred`]'s constructors do.
#[derive(Clone, Debug, PartialEq)]
pub enum Term<C: Column> {
    /// Every row.
    Always,
    /// No row.
    Never,
    /// A column against an operand: `col == v`, `!=`, `<`, `<=`, `>`, `>=`,
    /// and with a literal also `is`, `is not`, `is null`, `is not null`.
    Cmp {
        /// The column.
        column: C,
        /// The operator.
        op: Op,
        /// The literal or the attribute.
        operand: Operand,
    },
    /// `col in [lits]`, `col not in [lits]`. Literals only: a setting holds
    /// one value.
    In {
        /// The column.
        column: C,
        /// The set.
        values: Vec<Value>,
        /// `NOT IN` when true.
        negated: bool,
    },
    /// `a && b && …`.
    All(Vec<Term<C>>),
    /// `a || b || …`.
    Any(Vec<Term<C>>),
    /// `!a`.
    Not(Box<Term<C>>),
    /// `exists T(…)`: a row of a link table. Resolves to [`Pred::Exists`].
    Exists(Box<Exists<C>>),
}

impl<C: Column> Term<C> {
    /// The predicate for these attributes.
    ///
    /// Each node goes through the [`Pred`] constructor that `policy!` calls
    /// for it, so the result folds the same way: an absent `else deny`
    /// attribute makes its comparison [`Pred::Never`], an empty `in` is
    /// [`Pred::Never`], `!never` is [`Pred::Always`].
    pub fn resolve(&self, attrs: &Attributes) -> Pred<C> {
        match self {
            Term::Always => Pred::Always,
            Term::Never => Pred::Never,
            Term::Cmp {
                column,
                op,
                operand,
            } => Pred::cmp(*column, *op, operand.bound(attrs)),
            Term::In {
                column,
                values,
                negated: false,
            } => Pred::one_of(*column, values.iter().cloned()),
            Term::In {
                column,
                values,
                negated: true,
            } => Pred::none_of(*column, values.iter().cloned()),
            Term::All(parts) => Pred::all(parts.iter().map(|p| p.resolve(attrs))),
            Term::Any(parts) => Pred::any(parts.iter().map(|p| p.resolve(attrs))),
            Term::Not(inner) => Pred::negate(inner.resolve(attrs)),
            Term::Exists(exists) => exists.resolve(attrs),
        }
    }
}

/// `exists T(foreign == local, filter…)`: a row in a link table that points
/// at this row, and matches each filter.
///
/// The symbolic twin of [`Membership`]: a filter value can be a subject
/// attribute.
#[derive(Clone, Debug, PartialEq)]
pub struct Exists<C: Column> {
    /// The column of this entity that the link table points at.
    pub local: C,
    /// The link table.
    pub table: Ident,
    /// The link table's column that matches [`Exists::local`].
    pub foreign: Ident,
    /// Further constraints on the link table, each a column of it, an
    /// operator and an operand.
    pub filter: Vec<(Ident, Op, Operand)>,
    /// `NOT EXISTS` when true.
    pub negated: bool,
}

impl<C: Column> Exists<C> {
    /// The predicate for these attributes: the [`Membership`] the hand-written
    /// form builds, with [`Membership::filter_by`] for each filter. An absent
    /// `else deny` filter value makes it [`Pred::Never`], as there.
    ///
    /// Also when [`Exists::negated`]: `!exists T(…, col == who.x else deny)`
    /// with no `x` is `never`, not `always`. `else deny` makes the term that
    /// holds it `never`, and `NOT EXISTS` is one term. A database reader
    /// writes `s(x) IS NOT NULL AND NOT EXISTS (…)` for each such `x`, so it
    /// agrees. A [`Term::Not`] around a non-negated `Exists` is different:
    /// [`Pred::negate`] turns `never` into `always`. `policy!` never builds
    /// that shape: it folds `!exists` into [`Exists::negated`], and refuses
    /// `!` over a larger predicate that holds an `else deny` filter.
    pub fn resolve(&self, attrs: &Attributes) -> Pred<C> {
        let mut membership = Membership::new(self.local, self.table, self.foreign);
        for (column, op, operand) in &self.filter {
            match membership.filter_by(*column, *op, operand.bound(attrs)) {
                Some(next) => membership = next,
                None => return Pred::Never,
            }
        }
        if self.negated {
            membership = membership.negate();
        }
        membership.into_pred()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs() -> Attributes {
        Attributes::new()
            .with("admin", Some(Value::Bool(true)))
            .with("banned", Some(Value::Bool(false)))
            .with("role", Some(Value::from("editor")))
            .with("user_id", None)
    }

    #[test]
    fn a_guard_reads_booleans_and_literals() {
        let a = attrs();
        assert_eq!(Guard::Attr("admin").holds(&a), Some(true));
        assert_eq!(
            Guard::Not(Box::new(Guard::Attr("banned"))).holds(&a),
            Some(true)
        );
        assert_eq!(
            Guard::Eq("role", Value::from("editor")).holds(&a),
            Some(true)
        );
        assert_eq!(
            Guard::Eq("role", Value::from("admin")).holds(&a),
            Some(false)
        );
        assert_eq!(Guard::All(vec![]).holds(&a), Some(true));
        assert_eq!(Guard::Any(vec![]).holds(&a), Some(false));
    }

    #[test]
    fn a_guard_over_an_absent_attribute_has_no_answer() {
        let a = attrs();
        assert_eq!(Guard::Attr("user_id").holds(&a), None);
        assert_eq!(Guard::Attr("unknown").holds(&a), None);
        // Even when the other part already decides it.
        let any = Guard::Any(vec![Guard::Attr("admin"), Guard::Attr("unknown")]);
        assert_eq!(any.holds(&a), None);
        assert_eq!(Guard::Not(Box::new(Guard::Attr("unknown"))).holds(&a), None);
        assert_eq!(Guard::Attr("role").holds(&a), None);
    }

    #[test]
    fn guard_keys_are_each_key_once_in_source_order() {
        let g = Guard::All(vec![
            Guard::Attr("b"),
            Guard::Any(vec![Guard::Eq("a", Value::Int(1)), Guard::Attr("b")]),
        ]);
        assert_eq!(g.keys(), ["b", "a"]);
    }

    #[test]
    fn an_app_only_rule_resolves_to_never() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        enum C {
            Id,
        }
        impl Column for C {
            const ALL: &'static [Self] = &[Self::Id];
            fn name(&self) -> &'static str {
                "id"
            }
        }
        let rule: Rule<C> = Rule::app_only("reads the clock");
        assert_eq!(rule.app_only, Some("reads the clock"));
        assert!(rule.arms.is_empty());
        assert!(rule.resolve(&attrs()).is_never());
        assert!(Rule::<C>::new(Term::Always).app_only.is_none());
        let _ = C::Id;
    }

    #[test]
    fn a_fixed_mask_lists_its_columns_and_the_others_do_not() {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        enum C {
            Id,
            Title,
            Body,
        }
        impl Column for C {
            const ALL: &'static [Self] = &[Self::Id, Self::Title, Self::Body];
            fn name(&self) -> &'static str {
                match self {
                    C::Id => "id",
                    C::Title => "title",
                    C::Body => "body",
                }
            }
        }
        let only = MaskRule::Fixed(FieldMask::only([C::Body, C::Title]));
        assert_eq!(only.columns(), Some(vec![C::Title, C::Body]));
        let except = MaskRule::Fixed(FieldMask::except([C::Id]));
        assert_eq!(except.columns(), Some(vec![C::Title, C::Body]));
        assert_eq!(
            MaskRule::Fixed(FieldMask::<C>::None).columns(),
            Some(vec![])
        );
        assert_eq!(MaskRule::Fixed(FieldMask::<C>::All).columns(), None);
        assert_eq!(MaskRule::<C>::PerSubject.columns(), None);
    }

    #[test]
    fn attributes_replace_a_key_set_twice() {
        let a = Attributes::new()
            .with("k", Some(Value::Int(1)))
            .with("k", Some(Value::Int(2)));
        assert_eq!(a.get("k"), Some(&Value::Int(2)));
        assert_eq!(a.iter().count(), 1);
    }
}
