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
//! # Why there are four traits and not one
//!
//! [`AuthStore`] is what every application needs. The rest are not:
//!
//! * [`MfaStore`] — TOTP secrets and recovery codes.
//! * [`OtpStore`] — one-time code challenges.
//! * [`SignInLocationStore`] — the networks an account has signed in from.
//!
//! §8 of the plan says a capability that was not asked for must be *absent*,
//! not merely switched off, and a trait is where that rule lands for storage: a
//! product with no second factor implements [`AuthStore`] and writes no TOTP
//! table, because [`Mfa`] will not compile against its store and nothing else
//! asks. The alternative — one trait with fourteen more methods, half of them
//! `unimplemented!()` in every adapter — makes "we do not do MFA" a runtime
//! panic instead of a compile error.
//!
//! The split is by feature and not by table because that is the unit an
//! application turns on. [`Otp`] needs [`OtpStore`] and nothing else;
//! [`LoginAlerts`] needs [`SignInLocationStore`] and nothing else.
//!
//! [`Mfa`]: crate::mfa::Mfa
//! [`Otp`]: crate::otp::Otp
//! [`LoginAlerts`]: crate::alerts::LoginAlerts
//!
//! # Adding a method to a trait breaks every implementor
//!
//! It does, and the three traits above were added in the knowledge of it. One
//! change could not be put behind a new trait: [`SessionRecord`] grew
//! [`SessionRecord::factors`] and [`SessionRecord::impersonation`], so every
//! adapter gains two columns. There was no way to record both identities of an
//! impersonated session without recording them *on the session*, and a session
//! that does not say it is impersonated is the audit failure the feature exists
//! to prevent.
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
//! 3. **[`OtpStore::claim_otp_attempt`] must be atomic.** It is a
//!    read-modify-write on an attempt counter, and two requests that both read
//!    `attempts = 4` before either writes have turned a cap of five into a cap
//!    of six — per concurrent request, which an attacker chooses.
//! 4. **[`MfaStore::claim_totp_step`] must be atomic.** It is the whole replay
//!    defence: two requests carrying the same correct code must not both be
//!    told the step was theirs.
//! 5. **[`MfaStore::consume_recovery_code`] must be atomic**, for the reason
//!    [`AuthStore::consume_token`] must be.
//!
//! [`Accounts`]: crate::accounts::Accounts
//! [`MemoryStore`]: crate::memory::MemoryStore

use async_trait::async_trait;

use crate::clock::UnixTime;
use crate::identity::{Account, AccountId, EmailAddress, ProviderLink};
use crate::impersonation::Impersonation;
use crate::mfa::FactorSet;
use crate::otp::OtpChallengeId;
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
    /// Which factors proved this session.
    ///
    /// Empty on a pre-login session and on an impersonated one — in the second
    /// case because nobody proved anything *as this account*, which is the
    /// point. See [`crate::mfa`].
    pub factors: FactorSet,
    /// Who is really behind the session, when it was opened by an operator
    /// acting as somebody else.
    ///
    /// `None` on an ordinary session. It is never cleared: see
    /// [`crate::impersonation`].
    pub impersonation: Option<Impersonation>,
}

impl SessionRecord {
    /// Whether an operator opened this session on somebody else's behalf.
    pub fn is_impersonated(&self) -> bool {
        self.impersonation.is_some()
    }

    /// The operator behind the session, when there is one.
    pub fn operator(&self) -> Option<&AccountId> {
        self.impersonation.as_ref().map(|i| &i.operator)
    }

    /// Whether an elevation is still fresh.
    ///
    /// Elevation has no timestamp of its own because it does not need one:
    /// [`Sessions::rotate`] mints a new session row at every privilege change,
    /// so `created_at` *is* when this privilege was granted. A session that is
    /// not [`Privilege::Elevated`] is never fresh.
    ///
    /// [`Sessions::rotate`]: crate::session::Sessions::rotate
    pub fn elevation_is_fresh(&self, now: UnixTime, max_age: UnixTime) -> bool {
        self.privilege == Privilege::Elevated && now - self.created_at < max_age
    }
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

/// A stored TOTP secret.
///
/// The secret is held in the clear because TOTP is symmetric and there is
/// nothing else to hold — see [`TotpSecret`] for what that means and what it
/// does not mean.
///
/// [`TotpSecret`]: crate::totp::TotpSecret
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TotpSecretRecord {
    /// The shared secret.
    pub secret: Vec<u8>,
    /// When enrolment started.
    pub created_at: UnixTime,
    /// When the user proved they had scanned it. `None` is a scan that was
    /// never confirmed, and it is not a factor.
    pub confirmed_at: Option<UnixTime>,
    /// The highest RFC 6238 step this account has already spent.
    ///
    /// This is the replay defence's whole state. Zero means none yet.
    pub last_step: i64,
}

/// A stored one-time-code challenge.
///
/// The code is not in the row. `code_hash` is a PHC argon2id string, hashed by
/// the same [`Passwords`] that hashes passwords, for the reasons in
/// [`crate::otp`].
///
/// [`Passwords`]: crate::password::Passwords
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtpChallengeRecord {
    /// The handle the client carries. Not a secret.
    pub id: OtpChallengeId,
    /// Whose challenge it is — or the enumeration sentinel, when the address
    /// belonged to nobody.
    pub account: AccountId,
    /// The PHC argon2id string of the code.
    pub code_hash: String,
    /// When it stops being valid.
    pub expires_at: UnixTime,
    /// Wrong guesses so far.
    pub attempts: u32,
    /// Wrong guesses this challenge survives. Carried on the row rather than
    /// held in the service, so the store can enforce the cap in the same
    /// statement that increments the counter.
    pub max_attempts: u32,
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
    async fn account_by_email(&self, email: &EmailAddress) -> Result<Option<Account>, StoreError>;

    /// Creates an account.
    ///
    /// Must answer [`StoreError::EmailTaken`] when a unique index on the email
    /// rejects it, rather than panicking or inventing a second row.
    async fn create_account(&self, new: NewAccount) -> Result<Account, StoreError>;

    /// Replaces the password hash. `None` removes the password, leaving an
    /// account that can only sign in through a provider.
    async fn set_password_hash(&self, id: &AccountId, hash: Option<&str>)
        -> Result<(), StoreError>;

    /// Records that the address has, or has not, been proven.
    async fn set_email_verified(&self, id: &AccountId, verified: bool) -> Result<(), StoreError>;

    // -- provider links ----------------------------------------------------

    /// Finds the link for a provider's subject, across all accounts.
    async fn link(&self, provider: &str, subject: &str)
        -> Result<Option<ProviderLink>, StoreError>;

    /// Every provider linked to an account.
    async fn links_for_account(&self, account: &AccountId)
        -> Result<Vec<ProviderLink>, StoreError>;

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
    async fn unlink_provider(&self, account: &AccountId, provider: &str) -> Result<(), StoreError>;

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
    async fn delete_sessions_for_account(&self, account: &AccountId) -> Result<u64, StoreError>;

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

/// TOTP secrets and recovery codes, for an application that wants second
/// factors.
///
/// Separate from [`AuthStore`] so that an application which does not want them
/// implements nothing: see the module documentation.
#[async_trait]
pub trait MfaStore: AuthStore {
    /// The account's TOTP secret, confirmed or not.
    async fn totp_secret(
        &self,
        account: &AccountId,
    ) -> Result<Option<TotpSecretRecord>, StoreError>;

    /// Stores a TOTP secret, replacing any previous one.
    ///
    /// Replacing is deliberate: a user who starts enrolment twice has
    /// abandoned the first QR code.
    async fn put_totp_secret(
        &self,
        account: &AccountId,
        record: TotpSecretRecord,
    ) -> Result<(), StoreError>;

    /// Marks the secret proved. A no-op when there is no secret.
    async fn confirm_totp_secret(
        &self,
        account: &AccountId,
        at: UnixTime,
    ) -> Result<(), StoreError>;

    /// Removes the TOTP secret.
    async fn delete_totp_secret(&self, account: &AccountId) -> Result<(), StoreError>;

    /// Spends an RFC 6238 step, and answers whether this caller got it.
    ///
    /// Answers `true` and records the step when `step` is **greater** than
    /// every step already recorded for this account; answers `false` otherwise
    /// and changes nothing. Greater rather than merely different, so that a
    /// code captured now cannot be replayed once the window has moved on and
    /// come back around.
    ///
    /// **Must be atomic.** In SQL that is
    /// `UPDATE ... SET last_step = $2 WHERE account = $1 AND last_step < $2`
    /// and a check on the row count. Two requests carrying the same correct
    /// code must not both win it: that is the replay this exists to stop.
    async fn claim_totp_step(&self, account: &AccountId, step: i64) -> Result<bool, StoreError>;

    /// Replaces the account's recovery codes with these digests.
    ///
    /// An empty list removes them all, which is what disabling a second factor
    /// does.
    async fn replace_recovery_codes(
        &self,
        account: &AccountId,
        digests: Vec<[u8; 32]>,
    ) -> Result<(), StoreError>;

    /// Deletes the recovery code with this digest, and answers whether there
    /// was one.
    ///
    /// **Must be atomic.** Deleting *is* the check, exactly as it is in
    /// [`AuthStore::consume_token`], so that one code cannot be spent twice.
    async fn consume_recovery_code(
        &self,
        account: &AccountId,
        digest: &[u8; 32],
    ) -> Result<bool, StoreError>;

    /// How many recovery codes are left.
    async fn count_recovery_codes(&self, account: &AccountId) -> Result<u32, StoreError>;
}

/// One-time-code challenges.
#[async_trait]
pub trait OtpStore: AuthStore {
    /// Stores a challenge.
    async fn create_otp_challenge(&self, challenge: OtpChallengeRecord) -> Result<(), StoreError>;

    /// Takes one attempt against a challenge.
    ///
    /// Returns the row **with the attempt already counted**, or `None` when
    /// there is no challenge with that id. When the row has expired, or the
    /// attempt it just counted was its last, the row is deleted in the same
    /// step — and still returned, so the caller can do the same amount of work
    /// either way.
    ///
    /// **Must be atomic.** Two requests that both read `attempts = 4` before
    /// either writes have turned a cap of five into a cap of six, and the
    /// attacker picks how many requests to make at once. In SQL this is one
    /// `UPDATE ... RETURNING` plus a conditional `DELETE`, in a transaction.
    async fn claim_otp_attempt(
        &self,
        id: &OtpChallengeId,
        now: UnixTime,
    ) -> Result<Option<OtpChallengeRecord>, StoreError>;

    /// Deletes a challenge. This is what spends a correct code.
    async fn delete_otp_challenge(&self, id: &OtpChallengeId) -> Result<(), StoreError>;

    /// Deletes challenges that expired before `now`, and answers how many.
    async fn purge_otp_challenges(&self, now: UnixTime) -> Result<u64, StoreError>;
}

/// The networks an account has signed in from, for login alerts.
#[async_trait]
pub trait SignInLocationStore: AuthStore {
    /// Records a sign-in from a network, and answers when that network was
    /// last used — `None` when it is new.
    ///
    /// The fingerprint is a keyed digest and the store must treat it as opaque:
    /// there is no address in it to index on, geolocate or log. See
    /// [`crate::alerts`].
    ///
    /// This is an upsert and it must be atomic, so that two sign-ins at once
    /// do not both report the network as new.
    async fn record_sign_in_location(
        &self,
        account: &AccountId,
        fingerprint: &[u8; 32],
        at: UnixTime,
    ) -> Result<Option<UnixTime>, StoreError>;

    /// How many networks the account has on record.
    async fn count_sign_in_locations(&self, account: &AccountId) -> Result<u64, StoreError>;

    /// Forgets networks unused since `unseen_since`, and answers how many rows
    /// went.
    async fn purge_sign_in_locations(&self, unseen_since: UnixTime) -> Result<u64, StoreError>;
}

impl From<StoreError> for crate::error::AuthError {
    fn from(e: StoreError) -> Self {
        crate::error::AuthError::Store(e.to_string())
    }
}
