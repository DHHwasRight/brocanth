//! Every way a request does not succeed, kept separate so the status table can
//! be written against it.
//!
//! The important split is between an **answer** and a **failure**. A request
//! the policy refuses is an answer: the system worked, and the reply is a
//! `200` with no items, a `403`, a `404` or a `400` depending on what would
//! leak. A database that is down is a failure, and it reaches the
//! application's error hooks rather than being rendered here. [`Failure`]
//! carries both because a handler has one return type; [`crate::status`] is
//! where they part company.

use brocanth_policy::{CompileError, Denied, EvalError, MaskViolation};
use thiserror::Error;

use crate::data::DataError;

/// Why a resource operation did not produce what was asked for.
#[derive(Debug, Error)]
pub enum Failure {
    /// The policy for this operation is [`Pred::Never`](brocanth_policy::Pred::Never).
    ///
    /// The typed equivalent of PocketBase's `null` rule: the door is shut for
    /// this subject, and no row was looked at. A collection-level fact, so
    /// saying it discloses nothing about any record.
    #[error("no rule admits this subject for this operation")]
    NoRule,

    /// The condition admitted no row.
    ///
    /// Either the row is not there or the subject may not have it.
    /// `brocanth-policy` refuses to distinguish the two and so does this
    /// crate: the distinction is itself a disclosure.
    #[error("the condition admitted no row")]
    NoMatch,

    /// The row stopped matching the condition between the check and the write.
    ///
    /// PLAN.md §9 obligation 2, reported rather than swallowed.
    #[error("the row changed between the authorisation check and the write")]
    RaceLost,

    /// The write touched a column the subject may not write.
    #[error(transparent)]
    NotWritable(#[from] MaskViolation),

    /// The request itself is unusable: an unparseable id, a body this resource
    /// does not understand, a page number that is not a number.
    #[error("{0}")]
    BadRequest(String),

    /// The application's subject extractor could not say who is asking.
    ///
    /// Not the same as "anonymous". An anonymous visitor is a subject the
    /// application builds with its own `None` inside, and the policy decides
    /// what that means. This is the extractor failing.
    #[error("the request carries no subject this resource can read")]
    NoSubject,

    /// The storage layer failed.
    #[error(transparent)]
    Data(#[from] DataError),

    /// A policy did not compile. A programming error, never a request error.
    #[error(transparent)]
    Compile(#[from] CompileError),

    /// A policy could not be evaluated in memory under
    /// [`CreateCheck::InMemory`](crate::CreateCheck).
    ///
    /// Also a programming error: the resource opted into the in-memory
    /// interpretation with a predicate that needs a database. The request is
    /// refused, and the refusal is loud rather than a quiet denial, because a
    /// quiet denial is a bug that survives to production.
    #[error(transparent)]
    Eval(#[from] EvalError),
}

impl From<Denied> for Failure {
    fn from(denied: Denied) -> Self {
        match denied {
            Denied::NotAuthorised => Failure::NoMatch,
            Denied::RaceLost => Failure::RaceLost,
        }
    }
}

impl Failure {
    /// A bad request, from anything that can say why.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Failure::BadRequest(message.into())
    }

    /// Whether this is the application's problem rather than the request's.
    ///
    /// These are the ones [`crate::routes`] hands to the core as a
    /// `HandlerError`, so that an application's error hooks see them. The rest
    /// are answers, and answers are rendered.
    pub fn is_internal(&self) -> bool {
        match self {
            Failure::Data(error) => !matches!(error, DataError::Invalid(_)),
            Failure::Compile(_) | Failure::Eval(_) => true,
            Failure::NotWritable(violation) => {
                matches!(violation, MaskViolation::UnknownField { .. })
            }
            _ => false,
        }
    }
}
