//! Authorisation as a seam, and a default that denies.
//!
//! PLAN.md §8 forbids assuming an authentication scheme, so this crate ships
//! none. It does not know what a user is, where a session lives or how a
//! password is hashed. It takes a [`Policy`] the application supplies and asks
//! it before every screen.
//!
//! **The default is [`DenyAll`].** An admin mounted without a policy answers
//! 403 to everything, including its own index. The alternative — shipping open
//! and asking people to remember to lock it — is a security incident waiting
//! for someone's first deploy, and a default is what everyone's first deploy
//! runs.
//!
//! The identity is opaque. [`Principal`] carries a `&dyn Any` that the
//! application's own middleware attached, so an admin and an authentication
//! plugin never have to agree on a user type:
//!
//! ```
//! use brocanth_admin::policy::{policy_fn, Action, Principal};
//!
//! struct User { admin: bool }
//!
//! let policy = policy_fn(|principal: &Principal<'_>, request| {
//!     match principal.downcast::<User>() {
//!         Some(user) if user.admin => true,
//!         Some(_) => request.action == Action::ViewList,
//!         None => false,
//!     }
//! });
//! # let _ = policy;
//! ```
//!
//! # Row-level rules, and the paginator that lies
//!
//! The policy is asked per **model and action**, and per record only where a
//! record is already in hand. It is never used to filter a page of rows after
//! they have been read, and that restraint is deliberate: PLAN §9 is explicit
//! that list authorisation is a set comprehension rather than a per-object
//! property, so asking for thirty rows, getting thirty and filtering to eleven
//! gives a page two that overlaps page one and a total that is a lie.
//!
//! A row-level rule therefore belongs in the `WHERE` clause, and that is
//! where [`crate::rows`] puts it: `brocanth-policy`'s `condition()` travels
//! in a [`crate::data::Scope`] on the page **and** on the count, and a write
//! is guarded by the condition it was checked against. This module decides
//! which screens a person may open; [`crate::rows`] decides which rows those
//! screens touch. Both must say yes.

use std::any::Any;
use std::fmt;

use crate::model::PrimaryKey;

/// What is being asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// See the index of models.
    ViewIndex,
    /// See a model's list screen.
    ViewList,
    /// See one record.
    ViewDetail,
    /// Create a record, and see the form that does it.
    Create,
    /// Change a record, and see the form that does it.
    Update,
    /// Remove a record, and see the confirmation that does it.
    Delete,
    /// Read a panel another crate contributed. The subject is its key.
    ViewPanel,
    /// Press a button on one. The subject is its key.
    ///
    /// Separate from [`Action::ViewPanel`], because "may look at the queue"
    /// and "may empty the dead set" are different answers in every
    /// organisation that has ever been asked the question.
    ActOnPanel,
}

impl Action {
    /// The word used in a 403 message and in a template.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::ViewIndex => "view the admin",
            Action::ViewList => "list",
            Action::ViewDetail => "view",
            Action::Create => "create",
            Action::Update => "change",
            Action::Delete => "delete",
            Action::ViewPanel => "view this",
            Action::ActOnPanel => "change this",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One authorisation question.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    /// What is being asked for.
    pub action: Action,
    /// The model key, or `""` for [`Action::ViewIndex`].
    pub model: &'a str,
    /// The record, when the question is about one.
    pub record: Option<&'a PrimaryKey>,
}

impl<'a> Request<'a> {
    /// A question about a model.
    pub fn new(action: Action, model: &'a str) -> Self {
        Request {
            action,
            model,
            record: None,
        }
    }

    /// A question about one record.
    pub fn on(action: Action, model: &'a str, record: &'a PrimaryKey) -> Self {
        Request {
            action,
            model,
            record: Some(record),
        }
    }
}

/// Whoever is asking, as the application spells it.
///
/// The admin never looks inside. It carries the value from the request to the
/// policy and no further, so this crate has no opinion about sessions,
/// cookies, tokens or users.
#[derive(Clone, Copy)]
pub struct Principal<'a> {
    value: Option<&'a (dyn Any + Send + Sync)>,
}

impl<'a> Principal<'a> {
    /// Nobody. A policy that denies anonymous access denies this.
    pub fn anonymous() -> Self {
        Principal { value: None }
    }

    /// Wraps the application's own identity value.
    pub fn new(value: &'a (dyn Any + Send + Sync)) -> Self {
        Principal { value: Some(value) }
    }

    /// The identity as the application's type, when it is that type.
    pub fn downcast<T: 'static>(&self) -> Option<&'a T> {
        self.value.and_then(|v| v.downcast_ref::<T>())
    }

    /// Whether any identity is attached at all.
    pub fn is_anonymous(&self) -> bool {
        self.value.is_none()
    }
}

impl fmt::Debug for Principal<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Principal")
            .field("anonymous", &self.is_anonymous())
            .finish()
    }
}

/// Decides whether a request is allowed.
pub trait Policy: Send + Sync + 'static {
    /// `true` to allow.
    fn allows(&self, principal: &Principal<'_>, request: Request<'_>) -> bool;
}

/// Refuses everything. The default, and the only safe one.
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAll;

impl Policy for DenyAll {
    fn allows(&self, _principal: &Principal<'_>, _request: Request<'_>) -> bool {
        false
    }
}

/// Allows everything. For a development build or a test, and nothing else.
///
/// Named so that it cannot appear in a diff without being noticed.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllowAllUnsafeForDevelopment;

impl Policy for AllowAllUnsafeForDevelopment {
    fn allows(&self, _principal: &Principal<'_>, _request: Request<'_>) -> bool {
        true
    }
}

impl<F> Policy for F
where
    F: Fn(&Principal<'_>, Request<'_>) -> bool + Send + Sync + 'static,
{
    fn allows(&self, principal: &Principal<'_>, request: Request<'_>) -> bool {
        self(principal, request)
    }
}

/// Builds a policy from a closure, with the argument types pinned so inference
/// does not need the annotations spelled out at the call site.
pub fn policy_fn<F>(f: F) -> F
where
    F: Fn(&Principal<'_>, Request<'_>) -> bool + Send + Sync + 'static,
{
    f
}
