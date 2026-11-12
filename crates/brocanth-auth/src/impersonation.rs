//! Impersonation: an operator acts as a user, and the session says so forever.
//!
//! # The failure this module exists to not have
//!
//! Support opens a ticket, an operator clicks "view as this customer", and the
//! session they get back is byte-for-byte a session that customer could have
//! opened themselves. Six weeks later somebody asks who deleted the invoice.
//! The answer is in nobody's database, because the only identity the session
//! ever carried was the customer's.
//!
//! So an impersonated session here records **both** identities, in the session
//! row, for as long as the session exists. [`SessionRecord::account`] stays the
//! person being acted *for* — every authorization check in the application
//! keeps working, unchanged, which is the whole point of impersonation — and
//! [`SessionRecord::impersonation`] names the operator, their own session, when
//! it started, and the reason they typed. An audit trail joins the two.
//!
//! [`SessionRecord::account`]: crate::storage::SessionRecord::account
//! [`SessionRecord::impersonation`]: crate::storage::SessionRecord::impersonation
//!
//! # What an impersonated session may not do, and where that is enforced
//!
//! "Operators should not change customers' passwords" is a sentence in a
//! runbook. [`Restricted`] is the same sentence with a column saying which type
//! makes it impossible, and every row of it is enforced:
//!
//! | Refused | Enforced by |
//! |---|---|
//! | Change the password | [`Accounts::change_password`] takes [`AccountHolder`], which [`AccountHolder::of`] refuses to build from an impersonated session |
//! | Enrol, confirm or remove a second factor | the same, on [`Mfa`] |
//! | See the recovery codes | the same: codes are returned only by the calls that take an [`AccountHolder`], and so is the list of enrolled factors ([`Mfa::enrolled`]) |
//! | Reach [`Privilege::Elevated`] | [`Sessions::elevate`] refuses an impersonated session, and no other method takes a [`Privilege`] at all |
//! | Impersonate somebody else | [`Sessions::impersonate`] refuses an operator session that is itself impersonated |
//! | Connect or remove a provider sign-in | [`Linking::link`] and [`Linking::unlink`] take [`AccountHolder`] |
//!
//! The list is deliberately short. Everything an operator genuinely needs
//! impersonation *for* — seeing the broken page, reproducing the failing
//! order — still works, because it goes through the application's own
//! authorization and that reads `account`.
//!
//! What is *not* enforced here, and cannot be: an impersonated session can do
//! anything the application itself exposes. If your product lets a signed-in
//! user wire money, an operator impersonating them can wire money. This crate
//! guards the credentials; the rest is [`brocanth-policy`]'s half of the
//! problem.
//!
//! [`Accounts::change_password`]: crate::accounts::Accounts::change_password
//! [`Mfa`]: crate::mfa::Mfa
//! [`FactorPolicy`]: crate::mfa::FactorPolicy
//! [`Privilege`]: crate::session::Privilege
//! [`Privilege::Elevated`]: crate::session::Privilege::Elevated
//! [`Sessions::elevate`]: crate::session::Sessions::elevate
//! [`Mfa::enrolled`]: crate::mfa::Mfa::enrolled
//! [`Sessions::impersonate`]: crate::session::Sessions::impersonate
//! [`brocanth-policy`]: https://docs.rs/brocanth-policy
//! [`Linking::link`]: crate::linking::Linking::link
//! [`Linking::unlink`]: crate::linking::Linking::unlink

use serde::{Deserialize, Serialize};

use crate::clock::UnixTime;
use crate::error::AuthError;
use crate::identity::AccountId;
use crate::session::{Privilege, SessionId};
use crate::storage::SessionRecord;

/// Who is really behind an impersonated session.
///
/// Stored on the session row and never removed from it: a session that was
/// opened by impersonation stays marked for its whole life, and there is no
/// method here that clears the field. Ending impersonation deletes the session
/// rather than laundering it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Impersonation {
    /// The operator doing the acting.
    pub operator: AccountId,
    /// The operator's own session, which is still alive and is what they
    /// return to. Recording it is what lets an audit log join "the operator
    /// signed in at 09:02" to "somebody deleted the invoice at 09:14".
    pub operator_session: SessionId,
    /// When it started.
    pub started_at: UnixTime,
    /// Why, as the operator typed it — a ticket number, usually.
    ///
    /// It is stored verbatim and this crate never parses it. Requiring it is
    /// not bureaucracy: an operator who must name a reason before they can see
    /// somebody's account is an operator who knows the click is recorded.
    pub reason: String,
}

/// Proof that the acting party is the account holder, not an operator wearing
/// their face.
///
/// This type is the enforcement. The operations in [`Restricted`] take one, and
/// the only ways to get one are [`AccountHolder::of`] — which refuses an
/// impersonated session — and [`AccountHolder::without_a_session`], whose name
/// is the review prompt.
///
/// It also remembers when the session last re-proved itself, so a call that
/// changes a credential ([`Mfa::begin_totp_enrolment`] and its siblings) can
/// ask for a fresh elevation: a stolen cookie alone must not enrol the
/// thief's authenticator.
///
/// [`Mfa::begin_totp_enrolment`]: crate::mfa::Mfa::begin_totp_enrolment
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountHolder {
    account: AccountId,
    proof: HolderProof,
}

/// How an [`AccountHolder`] was built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HolderProof {
    /// From a session; `Some` with the moment it was elevated.
    Session(Option<UnixTime>),
    /// From [`AccountHolder::without_a_session`]: no request, no thief.
    NoSession,
}

impl AccountHolder {
    /// Reads the holder out of a live session.
    ///
    /// # Errors
    /// Returns [`AuthError::Impersonated`] when the session was opened by
    /// impersonation. That is the whole mechanism: a handler that wants to
    /// change a password must hold one of these, and an operator's session
    /// cannot produce one.
    pub fn of(session: &SessionRecord) -> Result<Self, AuthError> {
        if session.impersonation.is_some() {
            return Err(AuthError::Impersonated);
        }
        let elevated_at = (session.privilege == Privilege::Elevated).then_some(session.created_at);
        Ok(Self {
            account: session.account.clone(),
            proof: HolderProof::Session(elevated_at),
        })
    }

    /// Asserts the holder with no session to check.
    ///
    /// For a command-line tool, a migration or a background job — code with no
    /// request and therefore no operator. **Calling this from a request handler
    /// defeats every guarantee this module makes**, which is why it is named
    /// the way it is rather than `new`.
    pub fn without_a_session(account: AccountId) -> Self {
        Self {
            account,
            proof: HolderProof::NoSession,
        }
    }

    /// Whose account it is.
    pub fn account(&self) -> &AccountId {
        &self.account
    }

    /// Whether the holder re-proved itself less than `max_age` seconds before
    /// `now`. A holder built without a session counts as fresh: it has no
    /// cookie to steal.
    pub fn elevation_is_fresh(&self, now: UnixTime, max_age: UnixTime) -> bool {
        match self.proof {
            HolderProof::NoSession => true,
            HolderProof::Session(Some(at)) => now - at < max_age,
            HolderProof::Session(None) => false,
        }
    }
}

/// What an impersonated session is refused, as a value rather than a paragraph.
///
/// Useful for an admin screen that wants to grey out the buttons it is about to
/// be refused, and for a test that asserts the list has not quietly shrunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Restricted {
    /// Setting a new password, with or without the current one.
    ChangePassword,
    /// Enrolling, confirming or removing a second factor.
    ManageSecondFactors,
    /// Reading or regenerating the recovery codes.
    ReadRecoveryCodes,
    /// Reaching [`Privilege::Elevated`].
    ///
    /// [`Privilege::Elevated`]: crate::session::Privilege::Elevated
    Elevate,
    /// Starting a further impersonation from inside one.
    Nest,
    /// Attaching or removing a provider sign-in.
    ///
    /// A link is a credential: an operator who connects their own provider
    /// account to the subject keeps a way in after the impersonation ends.
    LinkProvider,
}

impl Restricted {
    /// Everything on the list.
    pub fn all() -> [Restricted; 6] {
        [
            Restricted::ChangePassword,
            Restricted::ManageSecondFactors,
            Restricted::ReadRecoveryCodes,
            Restricted::Elevate,
            Restricted::Nest,
            Restricted::LinkProvider,
        ]
    }

    /// Which type refuses it, for a reviewer checking the claim.
    pub fn enforced_by(self) -> &'static str {
        match self {
            Restricted::ChangePassword
            | Restricted::ManageSecondFactors
            | Restricted::ReadRecoveryCodes
            | Restricted::LinkProvider => "AccountHolder::of",
            Restricted::Elevate => "Sessions::elevate",
            Restricted::Nest => "Sessions::impersonate",
        }
    }
}

impl std::fmt::Display for Restricted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Restricted::ChangePassword => "change the password",
            Restricted::ManageSecondFactors => "manage second factors",
            Restricted::ReadRecoveryCodes => "read the recovery codes",
            Restricted::Elevate => "elevate the session",
            Restricted::Nest => "impersonate from inside an impersonation",
            Restricted::LinkProvider => "connect or remove a provider sign-in",
        })
    }
}
