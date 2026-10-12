//! The storage contract, which is the whole of what this crate assumes about
//! persistence.
//!
//! §8 of the plan forbids assuming a database, and the ORM is a separate
//! plugin. So there is no SQL here, no pool, no migration and no entity. There
//! is one trait with the smallest set of operations the rest of the crate
//! actually performs, and [`MemoryStore`] implements it so the tests never
//! need a server.
//!
//! An adapter over `brocanth-model` is a later, separate crate. It implements
//! this trait and changes nothing above it.
//!
//! # Two requirements an implementation must meet
//!
//! 1. **[`AuthStore::consume_token`] must be atomic.** It has to delete the
//!    row and return what it deleted, in one step. If two requests can both
//!    read the same reset token before either deletes it, the token is not
//!    single-use, and "single-use" is the entire value of a reset token. In
//!    SQL that is `DELETE ... RETURNING`, or a transaction.
//! 2. **[`AuthStore::create_account`] must enforce uniqueness on the email.**
//!    The check-then-insert in [`Accounts`] narrows the window; only a unique
//!    index closes it.
//!
//! [`Accounts`]: crate::accounts::Accounts
//! [`MemoryStore`]: crate::memory::MemoryStore

use async_trait::async_trait;

use crate::clock::UnixTime;
use crate::identity::{Account, AccountId, EmailAddress, ProviderLink};
use crate::session::{Privilege, SessionId};
use crate::tokens::TokenPurpose;

/// What a store failed with.
///
/// It is a string because this crate cannot usefully distinguish a deadlock
/// from a dropped connection, and pretending otherwise would put a `match` in
/// callers that can only log either way. One case *is* distinguished, because
/// one caller genuinely branches on it: [`StoreError::EmailTaken`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// The unique index on the email rejected the insert.
    ///
    /// Distinguished because registration must turn it back into the same
    /// response a successful registration produces — see [`Accounts::register`].
    ///
    /// [`Accounts::register`]: crate::accounts::Accounts::register
    #[error("that email address is already registered")]
    EmailTaken,

    /// Anything else.
    #[error("{0}")]
    Backend(String),
}

impl StoreError {
    /// Builds a [`StoreError::Backend`].
    pub fn backend(what: impl Into<String>) -> Self {
        Self::Backend(what.into())
    }
}

/// What to create an account from.
#[derive(Clone, Debug, Default)]
pub struct NewAccount {
    /// The address, when there is one.
    pub email: Option<EmailAddress>,
    /// Whether the address is already proven. Only ever `true` when a provider
    /// we trust asserted it, or the person opened a verification link.
    pub email_verified: bool,
    /// The PHC string, when the account has a password.
    pub password_hash: Option<String>,
}

/// What to link a provider account from.
#[derive(Clone, Debug)]
pub struct NewProviderLink {
    /// Provider id.
    pub provider: String,
    /// The provider's subject.
    pub subject: String,
    /// The address the provider reported.
    pub email: Option<EmailAddress>,
}

/// A stored session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionRecord {
    /// The session identifier. The cookie carries this, signed.
    pub id: SessionId,
    /// Whose session it is.
    pub account: AccountId,
    /// What it is allowed to do without re-authenticating.
    pub privilege: Privilege,
    /// When it was created — reset by every rotation, because a rotation
    /// starts a new session.
    pub created_at: UnixTime,
    /// Last time it was used, for the idle timeout.
    pub last_seen_at: UnixTime,
    /// The hard deadline, which a rotation carries over rather than extends.
    pub absolute_expires_at: UnixTime,
}

/// A stored single-use token: a password reset or an email verification.
///
/// The token the user receives is `selector.verifier`. The store keeps the
/// selector in the clear, because it is a lookup key and not a secret, and
/// keeps only the SHA-256 of the verifier. A database leak therefore hands the
/// attacker nothing they can present: the verifier is not in the row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenRecord {
    /// What the token is for. Part of the lookup key, so a verification token
    /// cannot be spent as a reset token.
    pub purpose: TokenPurpose,
    /// The lookup half. Not a secret.
    pub selector: String,
    /// SHA-256 of the secret half.
    pub verifier_hash: [u8; 32],
    /// Whose token it is.
    pub account: AccountId,
    /// When it stops being valid.
    pub expires_at: UnixTime,
}

/// Everything this crate needs from storage, and nothing else.
///
/// Implementations must be safe to call concurrently; all methods take `&self`
/// for the same reason a connection pool does.
#[async_trait]
pub trait AuthStore: Send + Sync + 'static {
    // -- accounts ----------------------------------------------------------

    /// Finds an account by its identifier.
    async fn account(&self, id: &AccountId) -> Result<Option<Account>, StoreError>;

    /// Finds an account by its normalised email address.
    async fn account_by_email(&self, email: &EmailAddress)
        -> Result<Option<Account>, StoreError>;

    /// Creates an account.
    ///
    /// Must answer [`StoreError::EmailTaken`] when a unique index on the email
    /// rejects it, rather than panicking or inventing a second row.
    async fn create_account(&self, new: NewAccount) -> Result<Account, StoreError>;

    /// Replaces the password hash. `None` removes the password, leaving an
    /// account that can only sign in through a provider.
    async fn set_password_hash(
        &self,
        id: &AccountId,
        hash: Option<&str>,
    ) -> Result<(), StoreError>;

    /// Records that the address has, or has not, been proven.
    async fn set_email_verified(&self, id: &AccountId, verified: bool)
        -> Result<(), StoreError>;

    // -- provider links ----------------------------------------------------

    /// Finds the link for a provider's subject, across all accounts.
    async fn link(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<ProviderLink>, StoreError>;

    /// Every provider linked to an account.
    async fn links_for_account(
        &self,
        account: &AccountId,
    ) -> Result<Vec<ProviderLink>, StoreError>;

    /// Attaches a provider account to a local account.
    async fn link_provider(
        &self,
        account: &AccountId,
        new: NewProviderLink,
    ) -> Result<ProviderLink, StoreError>;

    /// Detaches a provider.
    ///
    /// The caller is responsible for refusing to remove the last way in; this
    /// crate does that in [`Linking::unlink`].
    ///
    /// [`Linking::unlink`]: crate::linking::Linking::unlink
    async fn unlink_provider(
        &self,
        account: &AccountId,
        provider: &str,
    ) -> Result<(), StoreError>;

    // -- sessions ----------------------------------------------------------

    /// Stores a session.
    async fn create_session(&self, session: SessionRecord) -> Result<(), StoreError>;

    /// Loads a session by identifier.
    async fn session(&self, id: &SessionId) -> Result<Option<SessionRecord>, StoreError>;

    /// Records that a session was used, for the idle timeout.
    async fn touch_session(&self, id: &SessionId, at: UnixTime) -> Result<(), StoreError>;

    /// Deletes one session.
    async fn delete_session(&self, id: &SessionId) -> Result<(), StoreError>;

    /// Deletes every session belonging to an account, and answers how many.
    ///
    /// This is "sign me out everywhere", and it is also what a password change
    /// and a password reset must call: a stolen password that has already been
    /// used to open a session is not revoked by changing the password.
    async fn delete_sessions_for_account(
        &self,
        account: &AccountId,
    ) -> Result<u64, StoreError>;

    // -- single-use tokens -------------------------------------------------

    /// Stores a single-use token.
    async fn store_token(&self, token: TokenRecord) -> Result<(), StoreError>;

    /// Deletes the token with this purpose and selector, and returns what was
    /// deleted.
    ///
    /// **Must be atomic.** See the module documentation.
    async fn consume_token(
        &self,
        purpose: TokenPurpose,
        selector: &str,
    ) -> Result<Option<TokenRecord>, StoreError>;

    /// Deletes every outstanding token of one purpose for an account.
    ///
    /// Issuing a new reset token invalidates the previous one, so a reset link
    /// forwarded by mistake stops working as soon as a new one is asked for.
    async fn delete_tokens_for_account(
        &self,
        account: &AccountId,
        purpose: TokenPurpose,
    ) -> Result<(), StoreError>;
}

impl From<StoreError> for crate::error::AuthError {
    fn from(e: StoreError) -> Self {
        crate::error::AuthError::Store(e.to_string())
    }
}
