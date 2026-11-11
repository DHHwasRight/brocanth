//! Attaching a policy to an entity, so it is unforgettable.
//!
//! PLAN.md §9's scoreboard is the design note for this file:
//!
//! > PocketBase's rules attach to the **data**, so they are unforgettable on
//! > the generated CRUD path and **entirely absent everywhere else**. … A
//! > witness attaches to the **function signature**, so it is unforgettable
//! > everywhere and must be written everywhere. Attaching the policy to the
//! > entity gives us the first; expressing the result as a witness gives us
//! > the second.
//!
//! [`Policy`] is the first half. [`crate::Verified`] is the second.
//!
//! # Every default denies
//!
//! Only [`Policy::list`] is required. The rest default to [`Pred::Never`] and
//! [`FieldMask::deny_all`] ([`Policy::writable_on_create`] defaults to
//! [`Policy::writable`], so it denies too), which matches `brocanth-admin`'s `DenyAll` and for
//! the same reason it gives: shipping open and asking people to remember to
//! lock it is a security incident waiting for someone's first deploy.
//!
//! # What this crate does not know
//!
//! What a subject is. `Policy::Subject` is the application's type — a session,
//! a user, an API key, `()` — and this crate never looks inside it. An
//! anonymous request is the application's own `None`, turned into a decision
//! by [`crate::Bound::or_deny`] at the point the policy reads it.

use crate::entity::Entity;
use crate::mask::FieldMask;
use crate::pred::{EntityPred, Pred};

/// The rules for one entity.
///
/// ```
/// use brocanth_policy::{Bound, Column, Entity, EntityPred, FieldMask, Ident, Policy, Pred};
///
/// # #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// # pub enum PostColumn { Id, AuthorId, Status }
/// # impl Column for PostColumn {
/// #     const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status];
/// #     fn name(&self) -> &'static str {
/// #         match self { Self::Id => "id", Self::AuthorId => "author_id", Self::Status => "status" }
/// #     }
/// # }
/// # pub struct Post;
/// # impl Entity for Post {
/// #     type Column = PostColumn;
/// #     const TABLE: Ident = Ident::must("posts");
/// #     const PRIMARY_KEY: Self::Column = PostColumn::Id;
/// # }
/// /// What the application knows about whoever is asking. Anonymous is `None`.
/// pub struct Visitor { user_id: Option<i64>, admin: bool }
///
/// pub struct PostPolicy;
///
/// impl Policy for PostPolicy {
///     type Entity = Post;
///     type Subject = Visitor;
///
///     fn list(&self, who: &Visitor) -> EntityPred<Post> {
///         if who.admin {
///             return Pred::Always;
///         }
///         // Published posts, or your own. `or_deny` is the word that says
///         // what an anonymous visitor's missing id means: it owns nothing.
///         Pred::any([
///             Pred::eq(PostColumn::Status, "published"),
///             Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id)),
///         ])
///     }
///
///     fn create(&self, who: &Visitor) -> EntityPred<Post> {
///         // The proposed row must name the author as its owner.
///         Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id))
///     }
///
///     fn update(&self, who: &Visitor) -> EntityPred<Post> {
///         Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id))
///     }
///
///     fn writable(&self, who: &Visitor) -> FieldMask<PostColumn> {
///         if who.admin {
///             FieldMask::except([PostColumn::Id])
///         } else {
///             // Never the status (PocketBase's `status:changed = false ||
///             // role = "admin"`, as a value), and never the owner. This
///             // entity has no other column, so that is nothing.
///             FieldMask::deny_all()
///         }
///     }
///
///     fn writable_on_create(&self, who: &Visitor) -> FieldMask<PostColumn> {
///         if who.admin {
///             return self.writable(who);
///         }
///         // A create names its owner, and `create` above admits only the
///         // author's own id. `update` reads the stored row and not the new
///         // one, so the owner column stays out of `writable`.
///         FieldMask::only([PostColumn::AuthorId])
///     }
/// }
///
/// let author = Visitor { user_id: Some(7), admin: false };
/// assert!(PostPolicy.writable_on_create(&author).permits(PostColumn::AuthorId));
/// assert!(!PostPolicy.writable(&author).permits(PostColumn::AuthorId));
/// ```
pub trait Policy {
    /// The table these rules are about.
    type Entity: Entity;

    /// Whoever is asking. This crate never looks inside it.
    type Subject;

    /// Which rows this subject may see in a list.
    ///
    /// The only required method, because it is the one PLAN.md §9 says cannot
    /// be done per object: list authorisation is a set comprehension, and a
    /// paginator that filters after reading lies about its totals.
    fn list(&self, subject: &Self::Subject) -> EntityPred<Self::Entity>;

    /// Which rows this subject may read one at a time. Defaults to
    /// [`Policy::list`], because a row you may see in a list is a row you may
    /// see.
    fn view(&self, subject: &Self::Subject) -> EntityPred<Self::Entity> {
        self.list(subject)
    }

    /// Which rows this subject may bring into existence, as a predicate over
    /// the row about to be written.
    fn create(&self, _subject: &Self::Subject) -> EntityPred<Self::Entity> {
        Pred::Never
    }

    /// Which rows this subject may change.
    fn update(&self, _subject: &Self::Subject) -> EntityPred<Self::Entity> {
        Pred::Never
    }

    /// Which rows this subject may remove.
    fn delete(&self, _subject: &Self::Subject) -> EntityPred<Self::Entity> {
        Pred::Never
    }

    /// Which columns this subject may change on a row that exists: an update.
    ///
    /// [`Policy::update`] decides on the row **as stored**, not on the row
    /// the update leaves. So a column in this mask is a column the subject
    /// may set to any value, and a column that [`Policy::update`] reads (the
    /// owner column, say) lets the subject move a row out of its own rule:
    /// an author who may write `author_id` can give a post to somebody else.
    /// Keep such a column out of this mask, and put it in
    /// [`Policy::writable_on_create`] when a create must set it.
    fn writable(&self, _subject: &Self::Subject) -> FieldMask<<Self::Entity as Entity>::Column> {
        FieldMask::deny_all()
    }

    /// Which columns this subject may set on a row it creates.
    ///
    /// Defaults to [`Policy::writable`]. Override it when a create must set a
    /// column that an update must not change. The usual case is the owner
    /// column: [`Policy::create`] checks the **proposed** row, so a rule like
    /// `author_id = <subject>` lets a create set only the subject's own id,
    /// while [`Policy::update`] checks only the stored row and cannot stop a
    /// change of owner.
    ///
    /// The other direction (a column an update may change but a create may
    /// not set) is rare. Write both methods for it.
    fn writable_on_create(
        &self,
        subject: &Self::Subject,
    ) -> FieldMask<<Self::Entity as Entity>::Column> {
        self.writable(subject)
    }
}
