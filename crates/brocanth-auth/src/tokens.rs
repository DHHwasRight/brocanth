//! Single-use, expiring tokens for password reset and email verification.
//!
//! # The shape, and why it is a selector and a verifier
//!
//! The token a user receives is `selector.verifier`. The store keeps the
//! selector in the clear and only `SHA-256(verifier)`.
//!
//! * The selector makes the lookup an index hit. The obvious alternative —
//!   store the hash and scan — turns every reset into a table scan, and the
//!   alternative to *that* is storing the token in the clear, which is how a
//!   database backup becomes a set of live password-reset links.
//! * Hashing only the verifier means the row is useless to whoever reads it. A
//!   leaked row contains a selector, which identifies nothing, and a digest,
//!   which cannot be presented.
//! * SHA-256 and not argon2, because the verifier is 256 bits from the OS
//!   CSPRNG. Key stretching defends a low-entropy secret; this one has no
//!   entropy to add.
//!
//! # Single use means deleted before it is checked
//!
//! [`Tokens::consume`] deletes the row *first* and validates what came back
//! second. A token presented twice therefore finds nothing on the second
//! attempt even if the first attempt failed validation, and two concurrent
//! requests cannot both succeed as long as the store honours the atomicity
//! [`AuthStore::consume_token`] requires.
//!
//! [`AuthStore::consume_token`]: crate::storage::AuthStore::consume_token

use std::sync::Arc;

use crate::clock::Clock;
use crate::error::AuthError;
use crate::identity::AccountId;
use crate::random::{ct_eq, random_secret, sha256};
use crate::storage::{AuthStore, TokenRecord};

/// What a single-use token is for.
///
/// The purpose is part of the lookup key, so a token minted to verify an email
/// address cannot be spent to reset the password on that account. Without this
/// a verification link mailed to an unproven address is a password reset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenPurpose {
    /// Resets a password.
    PasswordReset,
    /// Proves an email address reaches its owner.
    EmailVerification,
}

impl TokenPurpose {
    /// A stable string, for a store that keys on a column.
    pub fn as_str(self) -> &'static str {
        match self {
            TokenPurpose::PasswordReset => "password_reset",
            TokenPurpose::EmailVerification => "email_verification",
        }
    }
}

impl std::fmt::Display for TokenPurpose {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A token that has just been minted.
///
/// The secret exists in this value and nowhere else: the store has only a
/// digest of half of it. Put it in the link you mail and then drop it. Logging
/// it is the same mistake as logging a password.
#[derive(Clone)]
pub struct IssuedToken {
    secret: String,
    /// Whose token it is.
    pub account: AccountId,
    /// What it is for.
    pub purpose: TokenPurpose,
}

impl IssuedToken {
    /// The value to put in the link.
    pub fn secret(&self) -> &str {
        &self.secret
    }
}

impl std::fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IssuedToken")
            .field("account", &self.account)
            .field("purpose", &self.purpose)
            .field("secret", &"<redacted>")
            .finish()
    }
}

/// Mints and spends single-use tokens.
pub struct Tokens<S: AuthStore, C: Clock> {
    store: Arc<S>,
    clock: Arc<C>,
    reset_ttl: i64,
    verification_ttl: i64,
}

impl<S: AuthStore, C: Clock> Tokens<S, C> {
    /// Builds the service with default lifetimes: one hour for a reset, one
    /// day for a verification.
    ///
    /// A reset token is short because it is a live credential sitting in a
    /// mailbox. A verification token is longer because it proves nothing an
    /// attacker wants and people read their mail the next morning.
    pub fn new(store: Arc<S>, clock: Arc<C>) -> Self {
        Self {
            store,
            clock,
            reset_ttl: 60 * 60,
            verification_ttl: 24 * 60 * 60,
        }
    }

    /// Overrides the lifetimes, in seconds.
    pub fn with_ttls(mut self, reset: i64, verification: i64) -> Self {
        self.reset_ttl = reset;
        self.verification_ttl = verification;
        self
    }

    /// How long a token of this purpose lives.
    pub fn ttl(&self, purpose: TokenPurpose) -> i64 {
        match purpose {
            TokenPurpose::PasswordReset => self.reset_ttl,
            TokenPurpose::EmailVerification => self.verification_ttl,
        }
    }

    /// Mints a token, replacing any outstanding token of the same purpose.
    ///
    /// Replacing is the point: asking for a second reset link must kill the
    /// first, so a link forwarded or logged by accident stops working as soon
    /// as the user asks again.
    ///
    /// # Errors
    /// Fails when randomness or the store fails.
    pub async fn issue(
        &self,
        purpose: TokenPurpose,
        account: &AccountId,
    ) -> Result<IssuedToken, AuthError> {
        self.store.delete_tokens_for_account(account, purpose).await?;
        let selector = random_secret()?;
        let verifier = random_secret()?;
        self.store
            .store_token(TokenRecord {
                purpose,
                selector: selector.clone(),
                verifier_hash: sha256(verifier.as_bytes()),
                account: account.clone(),
                expires_at: self.clock.now() + self.ttl(purpose),
            })
            .await?;
        Ok(IssuedToken {
            secret: format!("{selector}.{verifier}"),
            account: account.clone(),
            purpose,
        })
    }

    /// Spends a token and answers whose it was.
    ///
    /// Deletes first, validates second: see the module documentation.
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidToken`] for a malformed, unknown, already
    /// spent, expired or wrong-purpose token. One variant, because telling the
    /// holder of a bad token which of those it was tells an attacker whether
    /// the selector existed.
    pub async fn consume(
        &self,
        purpose: TokenPurpose,
        presented: &str,
    ) -> Result<AccountId, AuthError> {
        let (selector, verifier) = presented.split_once('.').ok_or(AuthError::InvalidToken)?;
        let record = self
            .store
            .consume_token(purpose, selector)
            .await?
            .ok_or(AuthError::InvalidToken)?;

        // Expiry before the digest comparison, and the digest comparison in
        // constant time. The row is already gone either way, so an expired
        // token cannot be retried once the clock is wrong.
        if self.clock.now() >= record.expires_at {
            return Err(AuthError::InvalidToken);
        }
        if !ct_eq(&sha256(verifier.as_bytes()), &record.verifier_hash) {
            return Err(AuthError::InvalidToken);
        }
        Ok(record.account)
    }
}
