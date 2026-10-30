//! Row-level authorisation: every data path goes through the entity's policy.
//!
//! PLAN.md §12 item 9, *"the admin against row-level policies"*.
//! [`crate::policy`] asks *may this person open this screen*. This module asks
//! the question PLAN.md §9 is about: *which rows*. Both must say yes.
//!
//! # The four positions, as the admin uses them
//!
//! | screen | rule | how it is applied |
//! |---|---|---|
//! | list, and its count | [`Policy::list`] | `condition()` goes into the `WHERE` clause of the page **and** of the count, so the total and the pages describe one set |
//! | detail | [`Policy::view`] | one-row guarded select: `WHERE pk = ? AND <condition>` |
//! | edit form, delete confirmation | [`Policy::update`], [`Policy::delete`] | the same guarded select, with that rule |
//! | update, delete | [`Policy::update`], [`Policy::delete`] | an admission, then a [`Verified`](brocanth_policy::Verified) whose condition guards the write; zero rows is `409` |
//! | create | [`Policy::create`] | only [`Pred::Always`](brocanth_policy::Pred::Always) is accepted; see below |
//! | form fields | [`Policy::writable`] | a field the mask refuses is drawn read-only, and a POST that sends one is `403` before any row is read |
//! | foreign-key labels and pickers | the **target** model's view and list rules | a label or a choice the subject may not see is not shown |
//!
//! The status codes are `brocanth-resource`'s, for that crate's reasons:
//! a rule that is [`Pred::Never`](brocanth_policy::Pred::Never) is `403` and reads no row; a row that is
//! absent or refused is `404`, one answer for both; a guarded write that
//! changed nothing is `409`; a field the mask refuses is `403`.
//!
//! # The assertion
//!
//! PLAN.md §12: *"a build of `brocanth-resource` with no policy registered
//! must not compile"*. The admin makes the same claim. [`AdminResource`] has
//! an associated type `Policy` with no default, bound to this entity and this
//! subject. An impl that omits it is `error[E0046]`, an impl that names a
//! type which is not a policy is an unsatisfied bound, and a policy for
//! another entity does not compile either. `tests/ui/` holds the cases.
//!
//! # A model with no resource is denied
//!
//! The admin discovers models from the data source, so a model can exist
//! with no [`AdminResource`] registered. Such a model answers `403` to every
//! screen and is not in the index or the navigation. That is the
//! [`DenyAll`](crate::policy::DenyAll) argument again: a default is what
//! everyone's first deploy runs.
//!
//! [`AdminConfig::unscoped_rows_unsafe_for_development`](crate::AdminConfig::unscoped_rows_unsafe_for_development)
//! turns that off for every model without a resource. Its name is the
//! warning.
//!
//! # Who the subject is
//!
//! The admin's [`Principal`] is a `&dyn Any` the application attached.
//! [`AdminResource::subject`] turns it into the policy's subject, and the
//! default downcasts. A principal that is not the subject type — anonymous,
//! or another type — is a subject nobody can describe, and every rule for it
//! is [`Pred::Never`](brocanth_policy::Pred::Never).
//!
//! # Create
//!
//! A create rule other than [`Pred::Always`](brocanth_policy::Pred::Always) or [`Pred::Never`](brocanth_policy::Pred::Never) is refused
//! with `403`. Deciding a conditional create rule needs the proposed row as
//! a one-row CTE (`brocanth_resource::sql::create_admission`), and that needs
//! typed values the admin's string-keyed form does not have yet. Refusing is
//! the safe half of that gap; the docs list it as open.

use std::sync::Arc;

use brocanth_policy::{
    Column, CompileError, Condition, Denied, Dialect, Entity, EntityPred, MaskViolation, Policy,
    Value as PolicyValue,
};

use crate::data::Scope;
use crate::model::PrimaryKey;
use crate::policy::Principal;

/// One model of the admin, its entity, and the policy over its rows.
///
/// ```
/// use brocanth_admin::rows::AdminResource;
/// use brocanth_admin::policy::Principal;
/// use brocanth_policy::{Bound, Column, Dialect, Entity, EntityPred, FieldMask, Ident, Policy, Pred};
///
/// #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// pub enum PostColumn { Id, AuthorId, Title }
/// impl Column for PostColumn {
///     const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Title];
///     fn name(&self) -> &'static str {
///         match self { Self::Id => "id", Self::AuthorId => "author_id", Self::Title => "title" }
///     }
/// }
/// pub struct Post;
/// impl Entity for Post {
///     type Column = PostColumn;
///     const TABLE: Ident = Ident::must("post");
///     const PRIMARY_KEY: Self::Column = PostColumn::Id;
/// }
///
/// /// Whatever the application's middleware attached to the request.
/// pub struct Staff { id: i64 }
///
/// pub struct OwnPosts;
/// impl Policy for OwnPosts {
///     type Entity = Post;
///     type Subject = Staff;
///     fn list(&self, who: &Staff) -> EntityPred<Post> {
///         Pred::eq(PostColumn::AuthorId, who.id)
///     }
///     fn update(&self, who: &Staff) -> EntityPred<Post> {
///         self.list(who)
///     }
///     fn writable(&self, _: &Staff) -> FieldMask<PostColumn> {
///         FieldMask::only([PostColumn::Title])
///     }
/// }
///
/// pub struct Posts;
/// impl AdminResource for Posts {
///     type Entity = Post;
///     type Subject = Staff;
///     type Policy = OwnPosts;
///     const MODEL: &'static str = "post";
///     const DIALECT: Dialect = Dialect::Sqlite;
///     fn policy(&self) -> &OwnPosts { &OwnPosts }
/// }
///
/// let config = brocanth_admin::AdminConfig::new().resource(Posts);
/// # let _ = config;
/// ```
pub trait AdminResource: Send + Sync + 'static {
    /// The table.
    type Entity: Entity;

    /// Whoever is asking, as the policy knows them.
    type Subject: 'static;

    /// **The rules. No default: an admin resource without one does not
    /// compile.** Bound to this entity and this subject, so a policy for
    /// another table does not compile either.
    type Policy: Policy<Entity = Self::Entity, Subject = Self::Subject>;

    /// The model key this resource governs: the slug in the admin's URLs.
    const MODEL: &'static str;

    /// Which SQL the policy is compiled for. It must match the data source.
    const DIALECT: Dialect = Dialect::Postgres;

    /// The rules.
    fn policy(&self) -> &Self::Policy;

    /// The subject behind a principal, or `None` when there is none.
    ///
    /// `None` makes every rule [`Pred::Never`](brocanth_policy::Pred::Never). The default downcasts, which
    /// is right when the application attaches the subject type itself.
    fn subject<'a>(&self, principal: &Principal<'a>) -> Option<&'a Self::Subject> {
        principal.downcast::<Self::Subject>()
    }

    /// The primary key as the policy's value.
    ///
    /// It goes into the witness the admission produces. The default reads a
    /// single-column key as an integer when it is one, and as text otherwise.
    fn key_value(&self, key: &PrimaryKey) -> PolicyValue {
        let text = key.to_segment();
        match text.parse::<i64>() {
            Ok(n) => PolicyValue::Int(n),
            Err(_) => PolicyValue::Text(text),
        }
    }
}

/// Which rule a screen asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RowAction {
    /// [`Policy::list`].
    List,
    /// [`Policy::view`].
    View,
    /// [`Policy::create`].
    Create,
    /// [`Policy::update`].
    Update,
    /// [`Policy::delete`].
    Delete,
}

/// A rule, compiled for one subject.
#[derive(Debug)]
pub(crate) enum Gate {
    /// [`Pred::Never`](brocanth_policy::Pred::Never), no subject, or no resource. `403`, and no row is read.
    Shut,
    /// No row rule applies: the unsafe development switch.
    Unscoped,
    /// `AND` this into the `WHERE` clause.
    Where {
        condition: Condition,
        /// The predicate was [`Pred::Always`](brocanth_policy::Pred::Always).
        always: bool,
    },
}

impl Gate {
    /// The scope a data source is handed, or `None` when the gate is shut.
    pub(crate) fn scope(&self) -> Option<Scope<'_>> {
        match self {
            Gate::Shut => None,
            Gate::Unscoped => Some(Scope::Unscoped),
            Gate::Where { condition, .. } => Some(Scope::Where(condition)),
        }
    }

    pub(crate) fn is_shut(&self) -> bool {
        matches!(self, Gate::Shut)
    }

    /// Admits every row: unscoped, or [`Pred::Always`](brocanth_policy::Pred::Always).
    pub(crate) fn admits_everything(&self) -> bool {
        matches!(self, Gate::Unscoped | Gate::Where { always: true, .. })
    }
}

/// An admission for one row, before its answer.
///
/// The admin runs the guarded select with [`Pending::scope`] and passes the
/// row count to [`Pending::resolve`], which is the only way to get a
/// [`Guard`].
pub(crate) struct Pending {
    condition: Option<Condition>,
    resolve: Box<dyn FnOnce(usize) -> Result<Guard, Denied> + Send>,
}

impl Pending {
    fn unscoped() -> Pending {
        Pending {
            condition: None,
            resolve: Box::new(|rows| match rows {
                0 => Err(Denied::NotAuthorised),
                _ => Ok(Guard { condition: None }),
            }),
        }
    }

    pub(crate) fn scope(&self) -> Scope<'_> {
        match &self.condition {
            None => Scope::Unscoped,
            Some(condition) => Scope::Where(condition),
        }
    }

    /// Zero rows is [`Denied::NotAuthorised`].
    pub(crate) fn resolve(self, rows: usize) -> Result<Guard, Denied> {
        (self.resolve)(rows)
    }
}

/// The condition a [`brocanth_policy::Verified`] carried, on its way into the
/// write.
///
/// Not `Clone`, no public constructor, and [`Guard::applied`] takes it by
/// value: one admission guards one write.
pub(crate) struct Guard {
    condition: Option<Condition>,
}

impl Guard {
    pub(crate) fn scope(&self) -> Scope<'_> {
        match &self.condition {
            None => Scope::Unscoped,
            Some(condition) => Scope::Where(condition),
        }
    }

    /// Zero rows changed is [`Denied::RaceLost`], never success.
    pub(crate) fn applied(self, rows: u64) -> Result<u64, Denied> {
        match rows {
            0 => Err(Denied::RaceLost),
            n => Ok(n),
        }
    }
}

/// An [`AdminResource`] with its types erased, so the admin can hold one per
/// model.
pub(crate) trait RowRules: Send + Sync {
    fn model(&self) -> &'static str;
    fn table(&self) -> &'static str;
    fn gate(&self, principal: &Principal<'_>, action: RowAction) -> Result<Gate, CompileError>;
    /// `None` when the rule is [`Pred::Never`](brocanth_policy::Pred::Never) or there is no subject.
    fn admission(
        &self,
        principal: &Principal<'_>,
        action: RowAction,
        key: &PrimaryKey,
    ) -> Result<Option<Pending>, CompileError>;
    /// Whether the subject may write this column. An unknown name is not.
    fn writable(&self, principal: &Principal<'_>, column: &str) -> bool;
    /// `Policy::writable`, checked against the names a request sent.
    fn check(&self, principal: &Principal<'_>, sent: &[&str]) -> Result<(), MaskViolation>;
}

struct Typed<R>(R);

impl<R: AdminResource> Typed<R> {
    fn pred(&self, subject: &R::Subject, action: RowAction) -> EntityPred<R::Entity> {
        let policy = self.0.policy();
        match action {
            RowAction::List => policy.list(subject),
            RowAction::View => policy.view(subject),
            RowAction::Create => policy.create(subject),
            RowAction::Update => policy.update(subject),
            RowAction::Delete => policy.delete(subject),
        }
    }
}

impl<R: AdminResource> RowRules for Typed<R> {
    fn model(&self) -> &'static str {
        R::MODEL
    }

    fn table(&self) -> &'static str {
        <R::Entity as Entity>::TABLE.as_str()
    }

    fn gate(&self, principal: &Principal<'_>, action: RowAction) -> Result<Gate, CompileError> {
        let Some(subject) = self.0.subject(principal) else {
            return Ok(Gate::Shut);
        };
        let pred = self.pred(subject, action);
        if pred.is_never() {
            return Ok(Gate::Shut);
        }
        Ok(Gate::Where {
            always: pred.is_always(),
            condition: pred.condition::<R::Entity>(R::DIALECT)?,
        })
    }

    fn admission(
        &self,
        principal: &Principal<'_>,
        action: RowAction,
        key: &PrimaryKey,
    ) -> Result<Option<Pending>, CompileError> {
        let Some(subject) = self.0.subject(principal) else {
            return Ok(None);
        };
        let pred = self.pred(subject, action);
        if pred.is_never() {
            return Ok(None);
        }
        let admission = pred.admission::<R::Entity>(R::DIALECT, self.0.key_value(key))?;
        let condition = admission.condition().clone();
        Ok(Some(Pending {
            condition: Some(condition),
            resolve: Box::new(move |rows| {
                // `resolve` is the one function that makes a `Verified`, and
                // `release` consumes it. The write that follows is not a
                // statement `brocanth-policy` can build — the data source
                // writes through its ORM so the model's hooks run — so the
                // condition leaves the witness here, by name, and guards
                // that write instead.
                let verified = admission.resolve(rows)?;
                let (_id, condition) = verified.release();
                Ok(Guard {
                    condition: Some(condition),
                })
            }),
        }))
    }

    fn writable(&self, principal: &Principal<'_>, column: &str) -> bool {
        let Some(subject) = self.0.subject(principal) else {
            return false;
        };
        let mask = self.0.policy().writable(subject);
        <<R::Entity as Entity>::Column as Column>::ALL
            .iter()
            .find(|c| c.name() == column)
            .is_some_and(|c| mask.permits(*c))
    }

    fn check(&self, principal: &Principal<'_>, sent: &[&str]) -> Result<(), MaskViolation> {
        let Some(subject) = self.0.subject(principal) else {
            return Err(MaskViolation::Forbidden {
                fields: sent.iter().map(|s| (*s).to_owned()).collect(),
            });
        };
        self.0
            .policy()
            .writable(subject)
            .check(sent.iter().copied())
    }
}

/// Erases a resource.
pub(crate) fn erase<R: AdminResource>(resource: R) -> Arc<dyn RowRules> {
    Arc::new(Typed(resource))
}

/// What a model's rows are governed by.
#[derive(Clone, Copy)]
pub(crate) enum Rules<'a> {
    /// A registered [`AdminResource`].
    Resource(&'a dyn RowRules),
    /// No resource, and the unsafe development switch is on.
    Unscoped,
    /// No resource. Every rule is [`Pred::Never`](brocanth_policy::Pred::Never).
    Missing,
}

impl Rules<'_> {
    pub(crate) fn gate(
        self,
        principal: &Principal<'_>,
        action: RowAction,
    ) -> Result<Gate, CompileError> {
        match self {
            Rules::Resource(rules) => rules.gate(principal, action),
            Rules::Unscoped => Ok(Gate::Unscoped),
            Rules::Missing => Ok(Gate::Shut),
        }
    }

    pub(crate) fn admission(
        self,
        principal: &Principal<'_>,
        action: RowAction,
        key: &PrimaryKey,
    ) -> Result<Option<Pending>, CompileError> {
        match self {
            Rules::Resource(rules) => rules.admission(principal, action, key),
            Rules::Unscoped => Ok(Some(Pending::unscoped())),
            Rules::Missing => Ok(None),
        }
    }

    pub(crate) fn writable(self, principal: &Principal<'_>, column: &str) -> bool {
        match self {
            Rules::Resource(rules) => rules.writable(principal, column),
            Rules::Unscoped => true,
            Rules::Missing => false,
        }
    }

    pub(crate) fn check(
        self,
        principal: &Principal<'_>,
        sent: &[&str],
    ) -> Result<(), MaskViolation> {
        match self {
            Rules::Resource(rules) => rules.check(principal, sent),
            Rules::Unscoped => Ok(()),
            Rules::Missing => Err(MaskViolation::Forbidden {
                fields: sent.iter().map(|s| (*s).to_owned()).collect(),
            }),
        }
    }
}
