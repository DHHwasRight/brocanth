//! Local accounts, and the rule that none of them admits whether an address is
//! registered.
//!
//! # Account enumeration, in both channels
//!
//! An attacker with a list of addresses wants to know which ones have accounts
//! here. That is worth money on its own — it is a list of your customers — and
//! it is step one of a credential-stuffing run. There are two ways a login
//! form tells them.
//!
//! **The response.** "No such user" and "wrong password" are different
//! answers. So are a 404 and a 401, a redirect to one page versus another, and
//! a reset form that says "we sent you a link" only when it did. Every
//! function here answers with one shape: [`Accounts::login`] fails with
//! [`AuthError::InvalidCredentials`] and nothing else,
//! [`Accounts::register`] always succeeds, and
//! [`Accounts::request_password_reset`] always succeeds.
//!
//! **The timing.** A lookup that misses returns in a microsecond; a lookup
//! that hits spends 50 ms in argon2. That difference is visible over the
//! network, and it is the part most libraries get wrong after getting the
//! response right. Three things fix it here:
//!
//! * [`Passwords::verify_or_dummy`] runs a full argon2 verification against a
//!   dummy hash when there is no account, so login costs the same either way.
//! * [`Accounts::register`] hashes the password **before** it tries to create
//!   the account, so the expensive half happens whether or not the address is
//!   taken.
//! * The paths that have no password work — reset and registration-refused —
//!   perform the same store operations against a per-process *sentinel*
//!   account id, so the number and kind of queries do not depend on whether
//!   the address exists.
//!
//! # What the caller must not undo
//!
//! Each of these returns enough for the application to send the right email,
//! and the response it renders must not depend on which one it got. A screen
//! that says "check your inbox" for a known address and "no account found" for
//! an unknown one has reintroduced the whole problem one layer up, which is
//! why [`RegistrationNotice`] and [`ResetOutcome`] say so in their own
//! documentation.
//!
//! [`Passwords::verify_or_dummy`]: crate::password::Passwords::verify_or_dummy

use std::sync::Arc;

use crate::clock::Clock;
use crate::error::AuthError;
use crate::identity::{Account, AccountId, EmailAddress};
use crate::password::Passwords;
use crate::storage::{AuthStore, NewAccount, StoreError};
use crate::tokens::{IssuedToken, TokenPurpose, Tokens};

/// Why a login failed. There is one reason, on purpose.
///
/// This type exists so that the single-variant-ness is visible in the
/// signature rather than being a comment somebody deletes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoginRejection;

/// What to tell the person who just registered — which is the same thing
/// either way — and what to put in the email, which is not.
///
/// **The HTTP response must not depend on this value.** Render the same page
/// for both variants: "If that address can receive mail, we have sent a
/// message to it." The variant decides which message the *mailer* sends, and
/// the mailer is not an oracle because the attacker does not receive it.
#[derive(Clone, Debug)]
pub enum RegistrationNotice {
    /// A new account exists and needs its address proved.
    Created {
        /// The account.
        account: Account,
        /// The token for the verification link.
        verification: IssuedToken,
    },
    /// The address already has an account. Mail its owner a note saying
    /// somebody tried to register with it and offering a password reset — that
    /// is useful to the real owner and useless to the attacker.
    AlreadyRegistered {
        /// The address, so the mailer knows where to write.
        email: EmailAddress,
    },
}

/// The result of a registration, which always succeeds.
#[derive(Clone, Debug)]
pub struct RegistrationAccepted {
    /// What to mail. See [`RegistrationNotice`].
    pub notice: RegistrationNotice,
}

/// What to mail after a password-reset request, which always succeeds.
///
/// Same rule as [`RegistrationNotice`]: the response is identical, the email
/// is not.
#[derive(Clone, Debug)]
pub enum ResetOutcome {
    /// Mail the link.
    Issue {
        /// Whose account.
        account: Account,
        /// The token for the link.
        token: IssuedToken,
    },
    /// No account, so no mail. Say nothing different to the browser.
    Nothing,
}

/// The result of a reset request, which always succeeds.
#[derive(Clone, Debug)]
pub struct ResetRequested {
    /// What to mail. See [`ResetOutcome`].
    pub outcome: ResetOutcome,
}

/// Registration, login, password reset and email verification.
pub struct Accounts<S: AuthStore, C: Clock> {
    store: Arc<S>,
    passwords: Arc<Passwords>,
    tokens: Tokens<S, C>,
    /// A random account id that belongs to nobody.
    ///
    /// The "this address has no account" paths run their store operations
    /// against this id so that the query pattern a database sees — and the
    /// time it takes — does not depend on whether the address exists. The rows
    /// it leaves behind are overwritten by the next such request, because
    /// [`Tokens::issue`] deletes an account's outstanding tokens first.
    sentinel: AccountId,
}

impl<S: AuthStore, C: Clock> Accounts<S, C> {
    /// Builds the service.
    ///
    /// # Errors
    /// Fails when the OS has no randomness for the sentinel id.
    pub fn new(
        store: Arc<S>,
        clock: Arc<C>,
        passwords: Arc<Passwords>,
    ) -> Result<Self, AuthError> {
        Ok(Self {
            tokens: Tokens::new(store.clone(), clock),
            store,
            passwords,
            sentinel: AccountId::generate()?,
        })
    }

    /// Replaces the token service, to change its lifetimes.
    #[must_use]
    pub fn with_tokens(mut self, tokens: Tokens<S, C>) -> Self {
        self.tokens = tokens;
        self
    }

    /// The token service, for an application issuing its own verification
    /// mails.
    pub fn tokens(&self) -> &Tokens<S, C> {
        &self.tokens
    }

    /// Registers an account. Always succeeds.
    ///
    /// The password is hashed first, before the store is touched, so the
    /// expensive work happens whether or not the address is taken.
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidEmail`] for an address that is not an
    /// address — which reveals nothing, because it is a property of the input
    /// and not of the database — and [`AuthError::Store`] or
    /// [`AuthError::Crypto`] for genuine failures.
    pub async fn register(
        &self,
        email: &str,
        password: &str,
    ) -> Result<RegistrationAccepted, AuthError> {
        let email = EmailAddress::parse(email)?;
        // Before the lookup, always. This is the whole timing defence for
        // registration: argon2 dominates everything else by three orders of
        // magnitude, so as long as both paths pay it once they are
        // indistinguishable.
        let hash = self.passwords.hash(password)?;

        match self
            .store
            .create_account(NewAccount {
                email: Some(email.clone()),
                email_verified: false,
                password_hash: Some(hash),
            })
            .await
        {
            Ok(account) => {
                let verification = self
                    .tokens
                    .issue(TokenPurpose::EmailVerification, &account.id)
                    .await?;
                Ok(RegistrationAccepted {
                    notice: RegistrationNotice::Created {
                        account,
                        verification,
                    },
                })
            }
            Err(StoreError::EmailTaken) => {
                // The same store work as the success path, against an id that
                // belongs to nobody, so the two take the same time and issue
                // the same queries. The token is discarded unread.
                let _ = self
                    .tokens
                    .issue(TokenPurpose::EmailVerification, &self.sentinel)
                    .await?;
                Ok(RegistrationAccepted {
                    notice: RegistrationNotice::AlreadyRegistered { email },
                })
            }
            Err(other) => Err(other.into()),
        }
    }

    /// Authenticates an address and a password.
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidCredentials`] for a wrong password, an
    /// address nobody has registered, an address that is not an address, and
    /// an account that has no password because it only signs in through a
    /// provider. One error, one timing, four causes.
    pub async fn login(&self, email: &str, password: &str) -> Result<Account, AuthError> {
        // A malformed address does not short-circuit: it goes down the same
        // path with no account, so it costs the same as every other failure.
        let account = match EmailAddress::parse(email) {
            Ok(parsed) => self.store.account_by_email(&parsed).await?,
            Err(_) => None,
        };
        let hash = account.as_ref().and_then(|a| a.password_hash.as_deref());
        if self.passwords.verify_or_dummy(hash, password) {
            // `verify_or_dummy` answered true, so `account` is `Some` and has
            // a hash.
            account.ok_or(AuthError::InvalidCredentials)
        } else {
            Err(AuthError::InvalidCredentials)
        }
    }

    /// Starts a password reset. Always succeeds.
    ///
    /// # Errors
    /// Fails only on a genuine store or randomness failure.
    pub async fn request_password_reset(
        &self,
        email: &str,
    ) -> Result<ResetRequested, AuthError> {
        let account = match EmailAddress::parse(email) {
            Ok(parsed) => self.store.account_by_email(&parsed).await?,
            Err(_) => None,
        };
        match account {
            Some(account) => {
                let token = self
                    .tokens
                    .issue(TokenPurpose::PasswordReset, &account.id)
                    .await?;
                Ok(ResetRequested {
                    outcome: ResetOutcome::Issue { account, token },
                })
            }
            None => {
                // Identical store work against the sentinel. Without this, a
                // reset form answers in two very different times and is a
                // perfectly good enumeration oracle even though its HTML never
                // varies.
                let _ = self
                    .tokens
                    .issue(TokenPurpose::PasswordReset, &self.sentinel)
                    .await?;
                Ok(ResetRequested {
                    outcome: ResetOutcome::Nothing,
                })
            }
        }
    }

    /// Spends a reset token and sets a new password.
    ///
    /// **Revokes every session the account has**, which is not optional and is
    /// why it happens here rather than being left to the caller: a reset is
    /// what somebody does when they think their account is compromised, and a
    /// reset that leaves the attacker's session open has done nothing. The
    /// caller opens a fresh session afterwards if it wants the user signed in.
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidToken`] for a token that is wrong, spent or
    /// expired.
    pub async fn reset_password(
        &self,
        token: &str,
        new_password: &str,
    ) -> Result<AccountId, AuthError> {
        let account = self
            .tokens
            .consume(TokenPurpose::PasswordReset, token)
            .await?;
        let hash = self.passwords.hash(new_password)?;
        self.store.set_password_hash(&account, Some(&hash)).await?;
        self.store.delete_sessions_for_account(&account).await?;
        Ok(account)
    }

    /// Changes a password, given the current one.
    ///
    /// Revokes every session, for the same reason as
    /// [`Accounts::reset_password`].
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidCredentials`] when the current password is
    /// wrong.
    pub async fn change_password(
        &self,
        account: &AccountId,
        current: &str,
        new_password: &str,
    ) -> Result<(), AuthError> {
        let stored = self
            .store
            .account(account)
            .await?
            .ok_or(AuthError::InvalidCredentials)?;
        if !self
            .passwords
            .verify_or_dummy(stored.password_hash.as_deref(), current)
        {
            return Err(AuthError::InvalidCredentials);
        }
        let hash = self.passwords.hash(new_password)?;
        self.store.set_password_hash(account, Some(&hash)).await?;
        self.store.delete_sessions_for_account(account).await?;
        Ok(())
    }

    /// Issues an email-verification token.
    ///
    /// # Errors
    /// Fails when the store or randomness fails.
    pub async fn request_email_verification(
        &self,
        account: &AccountId,
    ) -> Result<IssuedToken, AuthError> {
        self.tokens
            .issue(TokenPurpose::EmailVerification, account)
            .await
    }

    /// Spends a verification token and marks the address proved.
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidToken`] for a token that is wrong, spent or
    /// expired.
    pub async fn verify_email(&self, token: &str) -> Result<AccountId, AuthError> {
        let account = self
            .tokens
            .consume(TokenPurpose::EmailVerification, token)
            .await?;
        self.store.set_email_verified(&account, true).await?;
        Ok(account)
    }
}
