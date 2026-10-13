//! The status table, and the reason beside every entry.
//!
//! `docs/pocketbase-surface.md` §3 records PocketBase's choices:
//!
//! > **A rule is also a filter.** An unsatisfied `listRule` yields `200` with
//! > an empty `items`, not `403`. `createRule` unsatisfied → `400`.
//! > `view`/`update`/`delete` unsatisfied → `404`. A `null` rule for a
//! > non-superuser → `403`.
//!
//! Those are not arbitrary. They are information hiding, and the rule behind
//! all four is one sentence: **a status code must not tell you about a record
//! you are not allowed to see.** A `403` where PocketBase returns `404` tells
//! an attacker the record exists.
//!
//! So the ones that encode a fact about a record are copied exactly, and the
//! ones that encode a fact about the collection are decided here on their
//! merits. The table, with the reason for each line:
//!
//! | Situation | Status | Copied? | Reason |
//! |---|---|---|---|
//! | list, condition matches nothing | `200` `{"items":[]}` | yes | A list rule is a filter. An empty page is indistinguishable from a collection with no matching rows, which is exactly the point. A `403` here would turn the list route into an oracle for "does anything match this filter". |
//! | view / update / delete, condition matches nothing | `404` | yes | The existence of the row is the secret. `403` and `404` must be the same answer, so they are: a row you may not see and a row that is not there are one case. |
//! | create, condition refuses the proposed row | `400` | yes | Nothing exists yet, so there is nothing to conceal. The body is what failed the predicate, which makes this a statement about the request rather than about a resource. |
//! | any operation, the policy is [`Pred::Never`](brocanth_policy::Pred::Never) | `403` | yes | The typed equivalent of PocketBase's `null` rule. It is a fact about the collection and the subject, decided without reading a row, so it discloses nothing about any record. It is also the one refusal a client can act on: no retry, no different id, go and get different credentials. |
//! | update, a field the mask forbids | `403` | — | PocketBase folds this into `updateRule` and answers `404`. Here the mask is checked **before the row is touched**, which is what makes `403` safe: the answer is the same whether or not the row exists. It is also the more useful answer, because the client's mistake is in the body it sent. |
//! | update / delete, zero rows affected | `409` | — | The admission test passed, so the caller has already been told the row exists and matched. Nothing further leaks. `404` would be wrong twice over: the row may well still be there, and the client's correct next move is to re-read and retry, which is `409`'s meaning and not `404`'s. This is [`Denied::RaceLost`](brocanth_policy::Denied::RaceLost), and most applications report it as "nothing to do". |
//! | update, the row is no longer visible to its author | `204` | — | A legal write can move a row out of the subject's own view predicate — unpublish your own post under a policy that lists only published ones. There is no body to return and refusing the write would be wrong, so the write stands and the reply is empty. |
//! | create, success | `201` + `Location` | — | The surface note records PocketBase's failure codes, which are the ones that carry information. A success code carries none, so this one is decided on HTTP's own terms. |
//! | delete, success | `204` | — | Same. |
//! | no subject the extractor could read | `401` | — | Not anonymity. Anonymity is a subject the application builds with its own `None` inside, and the policy decides what it means. This is the extractor failing, which is a `401` because the request cannot be authorised at all. |
//! | a column that is not on the entity | `500` | — | [`MaskViolation::UnknownField`](brocanth_policy::MaskViolation::UnknownField): the mask and the record disagree about which table they describe. A programming error, and reporting it as a client error would send somebody to debug the wrong machine. |
//! | the storage layer failed | `500` | — | Handed to the application's error hooks rather than rendered, so a logger sees it. |
//!
//! # The one place the order of checks is load-bearing
//!
//! The field mask is checked **before** the admission query. If it ran after,
//! a `403` for a forbidden field would mean "this row exists and you may write
//! it, but not that column", and the `404` above would have been undone by the
//! next line of code. Checking first makes the `403` a statement about the
//! subject and the column only. `tests/writes.rs` asserts that no condition reached the data source when a
//! mask refused a write, and `tests/status.rs` pins every line of the table.

use crate::error::Failure;

/// Which route is answering.
///
/// The same [`Failure`] means different things on different routes — that is
/// the whole content of the table above — so the mapping takes both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    /// `GET /`
    List,
    /// `GET /{id}`
    View,
    /// `POST /`
    Create,
    /// `PATCH /{id}`
    Update,
    /// `DELETE /{id}`
    Delete,
}

impl Action {
    /// The word used in a message.
    pub fn as_str(self) -> &'static str {
        match self {
            Action::List => "list",
            Action::View => "view",
            Action::Create => "create",
            Action::Update => "update",
            Action::Delete => "delete",
        }
    }
}

/// What to answer with.
///
/// A plain `u16` rather than `http::StatusCode`, so the table is readable and
/// testable with the `plugin` feature off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The status code.
    pub status: u16,
    /// Whether the body says anything about why.
    ///
    /// `false` for the refusals that must not be distinguishable from each
    /// other; the message is then the generic one for the status.
    pub explain: bool,
    /// Whether this is the application's problem, and so belongs to its error
    /// hooks rather than to a rendered body.
    pub internal: bool,
}

impl Outcome {
    const fn answer(status: u16) -> Self {
        Outcome {
            status,
            explain: false,
            internal: false,
        }
    }

    const fn explained(status: u16) -> Self {
        Outcome {
            status,
            explain: true,
            internal: false,
        }
    }

    const fn internal(status: u16) -> Self {
        Outcome {
            status,
            explain: false,
            internal: true,
        }
    }
}

/// The table, as a function. Every line of it is argued for in the module
/// documentation.
pub fn outcome(action: Action, failure: &Failure) -> Outcome {
    match failure {
        // A fact about the collection and the subject. No row was read.
        Failure::NoRule => Outcome::answer(403),

        // A fact about a row, so the answer must be the same as "there is no
        // such row". List is the exception, because a list rule is a filter
        // and an empty page is the honest answer; `flow::list` never returns
        // `NoMatch`, and this line is here so the table is total.
        Failure::NoMatch => match action {
            Action::List => Outcome::answer(200),
            Action::Create => Outcome::answer(400),
            Action::View | Action::Update | Action::Delete => Outcome::answer(404),
        },

        // The caller already knows the row existed and matched.
        Failure::RaceLost => Outcome::answer(409),

        // Checked before the row is touched, which is what makes 403 safe.
        // An unknown column is a programming error on this side of the wire.
        Failure::NotWritable(violation) => {
            if matches!(
                violation,
                brocanth_policy::MaskViolation::UnknownField { .. }
            ) {
                Outcome::internal(500)
            } else {
                Outcome::explained(403)
            }
        }

        Failure::BadRequest(_) => Outcome::explained(400),
        Failure::NoSubject => Outcome::answer(401),

        Failure::Data(error) => match error {
            // A validation failure is the model layer's answer about the body.
            crate::data::DataError::Invalid(_) => Outcome::explained(400),
            _ => Outcome::internal(500),
        },

        // A policy that does not compile, or an in-memory check that cannot
        // decide. Loud, not quiet.
        Failure::Compile(_) | Failure::Eval(_) => Outcome::internal(500),
    }
}

/// The message for a status, when the body must not explain itself.
pub fn generic_message(status: u16) -> &'static str {
    match status {
        400 => "bad request",
        401 => "unauthenticated",
        403 => "forbidden",
        404 => "not found",
        409 => "the record changed; read it again and retry",
        _ => "error",
    }
}
