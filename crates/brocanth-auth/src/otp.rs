//! One-time codes: short enough for a human to type, and therefore guessable.
//!
//! PocketBase's shape, and the crate half of it. `request` answers a challenge
//! identifier; `authenticate` takes that identifier and the code. The code
//! itself goes to the address, never to the caller of `request`.
//!
//! # The arithmetic, which is one decision and not four
//!
//! Length, alphabet, attempt cap and lifetime are a single coupled choice, so
//! here is the sum rather than four defaults.
//!
//! * **Alphabet: the ten digits.** A code arrives in a notification and is
//!   typed on a phone keypad, read down a telephone, or copied by somebody with
//!   their reading glasses elsewhere. Letters raise the code space (36^8 is
//!   2.8 × 10^12 against 10^8) and raise the mistype rate with it — and the
//!   only lever that compensates for mistypes is a larger attempt cap, which
//!   hands the guess budget straight back.
//! * **Length: eight digits.** 10^8 = 100,000,000, about 2^26.6.
//! * **Attempts: five, then the challenge is destroyed.**
//! * **Lifetime: ten minutes.**
//!
//! So an attacker holding one challenge gets **5 guesses against 10^8**, or one
//! chance in 20,000,000, inside a ten-minute window. To improve on that they
//! must ask for more challenges — and every challenge sends a mail to the
//! person they are attacking. Expected challenges before one hit: 2 × 10^7. The
//! attack is twenty million emails to the victim's inbox, which is not a
//! quiet attack.
//!
//! **This is why the code is eight digits and not six.** Six digits is 10^6, so
//! five attempts is one chance in 200,000, and the expected attack is 200,000
//! mails. That is a weekend for somebody with a botnet, and it is the number
//! most products are quietly running.
//!
//! One more bound, and it is free: a guess costs **one argon2id verification**,
//! because the code is hashed with [`Passwords`] exactly as a password is. At
//! OWASP's cost that is tens of milliseconds of server CPU per guess, which
//! rate-limits the attack whether or not the application remembered to install
//! a rate limiter. It is also what makes a stolen database useless: a bare
//! SHA-256 of an eight-digit code is recovered in under a second.
//!
//! # Why this is not a [`TokenPurpose`]
//!
//! Every other single-use secret in this crate is a `selector.verifier` pair
//! and goes through [`Tokens`]. An OTP cannot, and the reason is the attempt
//! cap rather than taste.
//!
//! [`Tokens::consume`] **deletes the row and then validates it**, which is
//! exactly right for a 256-bit secret in a link: the first wrong guess is an
//! attack, so killing the token is the correct response. A code a human types
//! is different — the first wrong attempt is usually a typo, and a token that
//! dies on a typo is a product that does not work. Tolerating typos means a
//! counter, a counter means read-modify-write where the token protocol has
//! delete-and-return, and [`TokenRecord`] has nowhere to put it. A counter
//! bolted onto a delete-first protocol is not the same protocol.
//!
//! So OTP has its own record, its own atomic operation
//! ([`OtpStore::claim_otp_attempt`]) and its own store trait. What it does
//! *not* have is its own enumeration story: see below.
//!
//! # Enumeration
//!
//! [`Otp::request`] resists exactly the way [`request_password_reset`] does. An
//! address nobody has registered still mints a code, still pays the argon2id
//! hash, still writes a challenge row — against a per-process sentinel account
//! id — and still answers with a challenge identifier of the same shape. The
//! caller cannot tell, and neither can a clock.
//!
//! [`Otp::authenticate`] has one failure variant,
//! [`AuthError::InvalidCredentials`], for a wrong code, an expired code, a
//! challenge that never existed, a challenge whose attempts are spent and a
//! challenge that belongs to the sentinel. "Wrong, or expired, or never
//! issued?" is not a question this function answers.
//!
//! [`Passwords`]: crate::password::Passwords
//! [`Tokens`]: crate::tokens::Tokens
//! [`Tokens::consume`]: crate::tokens::Tokens::consume
//! [`TokenPurpose`]: crate::tokens::TokenPurpose
//! [`TokenRecord`]: crate::storage::TokenRecord
//! [`OtpStore::claim_otp_attempt`]: crate::storage::OtpStore::claim_otp_attempt
//! [`request_password_reset`]: crate::accounts::Accounts::request_password_reset

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::clock::Clock;
use crate::error::AuthError;
use crate::identity::{AccountId, EmailAddress};
use crate::password::Passwords;
use crate::random::{random_bytes, random_secret};
use crate::storage::{OtpChallengeRecord, OtpStore};

/// Digits in a code, unless an application insists otherwise.
pub const DEFAULT_DIGITS: u32 = 8;

/// Wrong guesses a challenge survives.
pub const DEFAULT_MAX_ATTEMPTS: u32 = 5;

/// How long a challenge lives, in seconds.
pub const DEFAULT_TTL: i64 = 10 * 60;

/// The handle a client carries between the two calls.
///
/// It is 256 random bits and it is *not* a secret: anybody who asked for the
/// challenge has it. It identifies which challenge a code is being presented
/// against, and nothing else. Knowing one is worth nothing without the code,
/// which only the mailbox receives.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct OtpChallengeId(String);

impl OtpChallengeId {
    /// A fresh identifier.
    ///
    /// # Errors
    /// See [`crate::random::random_secret`].
    pub fn generate() -> Result<Self, AuthError> {
        Ok(Self(random_secret()?))
    }

    /// Wraps an identifier a client sent back, or a store read.
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The identifier as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OtpChallengeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The code itself. It exists in this value, in the mail you send, and in the
/// user's head.
///
/// The store has only an argon2id digest of it. Logging this is the same
/// mistake as logging a password, which is why `Debug` says nothing.
#[derive(Clone, PartialEq, Eq)]
pub struct OtpCode(String);

impl OtpCode {
    /// The digits, to put in the message.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for OtpCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OtpCode(<redacted>)")
    }
}

/// What to send, which is not what to tell the browser.
///
/// **The HTTP response must not depend on this value.** Same rule as
/// [`RegistrationNotice`] and [`ResetOutcome`]: render one page — "if that
/// address can receive mail, a code is on its way" — for both variants. The
/// variant decides what the *mailer* does, and the mailer is not an oracle
/// because the attacker never receives the message.
///
/// [`RegistrationNotice`]: crate::accounts::RegistrationNotice
/// [`ResetOutcome`]: crate::accounts::ResetOutcome
#[derive(Clone, Debug)]
pub enum OtpDelivery {
    /// Mail this code to this address.
    Send {
        /// Whose account.
        account: AccountId,
        /// Where to write.
        email: EmailAddress,
        /// What to write.
        code: OtpCode,
    },
    /// No account, so no mail. Say nothing different to the browser.
    Nothing,
}

/// The result of asking for a code, which always succeeds.
#[derive(Clone, Debug)]
pub struct OtpRequested {
    /// Hand this to the client; it comes back with the code.
    pub challenge: OtpChallengeId,
    /// What to mail. See [`OtpDelivery`].
    pub delivery: OtpDelivery,
}

/// Issues and spends one-time codes.
pub struct Otp<S: OtpStore, C: Clock> {
    store: Arc<S>,
    clock: Arc<C>,
    passwords: Arc<Passwords>,
    /// An account id that belongs to nobody. See [`crate::accounts`], which
    /// uses the same device for the same reason.
    sentinel: AccountId,
    digits: u32,
    ttl: i64,
    max_attempts: u32,
}

impl<S: OtpStore, C: Clock> Otp<S, C> {
    /// Builds the service at the defaults the module documentation computes:
    /// eight digits, five attempts, ten minutes.
    ///
    /// # Errors
    /// Fails when the OS has no randomness for the sentinel id.
    pub fn new(store: Arc<S>, clock: Arc<C>, passwords: Arc<Passwords>) -> Result<Self, AuthError> {
        Ok(Self {
            store,
            clock,
            passwords,
            sentinel: AccountId::generate()?,
            digits: DEFAULT_DIGITS,
            ttl: DEFAULT_TTL,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
        })
    }

    /// Changes the code length.
    ///
    /// Read the arithmetic in the module documentation before you lower it:
    /// each digit removed divides the attacker's work by ten.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] outside 6..=10 digits.
    pub fn with_digits(mut self, digits: u32) -> Result<Self, AuthError> {
        if !(6..=10).contains(&digits) {
            return Err(AuthError::misconfigured(format!(
                "a one-time code is between 6 and 10 digits, not {digits}"
            )));
        }
        self.digits = digits;
        Ok(self)
    }

    /// Changes the attempt cap.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] outside 1..=10. Ten attempts
    /// against six digits is a code an attacker breaks with 100,000 mails.
    pub fn with_max_attempts(mut self, attempts: u32) -> Result<Self, AuthError> {
        if !(1..=10).contains(&attempts) {
            return Err(AuthError::misconfigured(format!(
                "a one-time code survives between 1 and 10 wrong guesses, not {attempts}"
            )));
        }
        self.max_attempts = attempts;
        Ok(self)
    }

    /// Changes the lifetime, in seconds.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] outside 60..=3600. An hour is the
    /// ceiling because a code sitting in a mailbox is a live credential.
    pub fn with_ttl(mut self, seconds: i64) -> Result<Self, AuthError> {
        if !(60..=3600).contains(&seconds) {
            return Err(AuthError::misconfigured(format!(
                "a one-time code lives between 60 and 3600 seconds, not {seconds}"
            )));
        }
        self.ttl = seconds;
        Ok(self)
    }

    /// Digits per code.
    pub fn digits(&self) -> u32 {
        self.digits
    }

    /// Wrong guesses a challenge survives.
    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    /// How long a challenge lives, in seconds.
    pub fn ttl(&self) -> i64 {
        self.ttl
    }

    /// Asks for a code by address. Always succeeds.
    ///
    /// An address nobody has registered takes the same path, pays the same
    /// argon2id hash and writes the same row against the sentinel, so neither
    /// the answer nor the clock says whether the address exists.
    ///
    /// # Errors
    /// Fails only on a genuine store or randomness failure.
    pub async fn request(&self, email: &str) -> Result<OtpRequested, AuthError> {
        // A malformed address does not short-circuit: it takes the no-account
        // path, which costs the same as every other outcome.
        let parsed = EmailAddress::parse(email).ok();
        let account = match &parsed {
            Some(address) => self.store.account_by_email(address).await?,
            None => None,
        };

        let code = self.mint_code()?;
        let owner = match &account {
            Some(found) => found.id.clone(),
            None => self.sentinel.clone(),
        };
        let challenge = self.store_challenge(&owner, &code).await?;

        let delivery = match (account, parsed) {
            (Some(found), Some(address)) => OtpDelivery::Send {
                account: found.id,
                email: address,
                code,
            },
            // The code is dropped here, unread, exactly as the sentinel token
            // in `Accounts::request_password_reset` is.
            _ => OtpDelivery::Nothing,
        };
        Ok(OtpRequested {
            challenge,
            delivery,
        })
    }

    /// Asks for a code for an account that is already identified.
    ///
    /// This is the second-factor-by-mail path: the primary factor has already
    /// succeeded, so there is no address to protect and no sentinel to hide
    /// behind. Do not call it from an unauthenticated handler — use
    /// [`Otp::request`], which does the hiding.
    ///
    /// # Errors
    /// Fails when randomness or the store fails.
    pub async fn request_for_account(
        &self,
        account: &AccountId,
    ) -> Result<(OtpChallengeId, OtpCode), AuthError> {
        let code = self.mint_code()?;
        let challenge = self.store_challenge(account, &code).await?;
        Ok((challenge, code))
    }

    /// Spends a code and answers whose it was.
    ///
    /// The attempt is claimed from the store *before* anything is compared, so
    /// a guess counts whatever happens afterwards and two concurrent guesses
    /// cannot share one attempt. The comparison then runs through
    /// [`Passwords::verify_or_dummy`], so a challenge that does not exist costs
    /// the same argon2id verification as one that does.
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidCredentials`] for a wrong code, an expired
    /// challenge, an unknown challenge, a challenge whose attempts are spent,
    /// and a challenge that was minted for an address nobody has registered.
    /// One variant, five causes, one timing.
    ///
    /// [`Passwords::verify_or_dummy`]: crate::password::Passwords::verify_or_dummy
    pub async fn authenticate(
        &self,
        challenge: &OtpChallengeId,
        presented: &str,
    ) -> Result<AccountId, AuthError> {
        let now = self.clock.now();
        let record = self.store.claim_otp_attempt(challenge, now).await?;

        // Always one verification, against the stored digest or against the
        // hasher's own dummy. This runs before any branch on `record`.
        let correct = self
            .passwords
            .verify_or_dummy(record.as_ref().map(|r| r.code_hash.as_str()), presented);

        let Some(record) = record else {
            return Err(AuthError::InvalidCredentials);
        };
        if now >= record.expires_at || !correct || record.account == self.sentinel {
            return Err(AuthError::InvalidCredentials);
        }
        // Single use. The row survived the attempt claim because attempts were
        // left; a correct code spends the whole challenge.
        self.store.delete_otp_challenge(challenge).await?;
        Ok(record.account)
    }

    /// Deletes challenges that expired before now, and answers how many.
    ///
    /// Challenges are **not** replaced the way [`Tokens::issue`] replaces a
    /// reset token. Two reasons: a request made for somebody else's address
    /// must not invalidate the code they are part-way through typing, and a
    /// challenge that the sentinel owns must behave exactly like one a real
    /// account owns — which it cannot if asking twice deletes the first. The
    /// cost is rows that outlive their use, so call this from a sweeper.
    ///
    /// [`Tokens::issue`]: crate::tokens::Tokens::issue
    ///
    /// # Errors
    /// Fails when the store fails.
    pub async fn purge_expired(&self) -> Result<u64, AuthError> {
        Ok(self.store.purge_otp_challenges(self.clock.now()).await?)
    }

    async fn store_challenge(
        &self,
        account: &AccountId,
        code: &OtpCode,
    ) -> Result<OtpChallengeId, AuthError> {
        let id = OtpChallengeId::generate()?;
        let code_hash = self.passwords.hash(code.as_str())?;
        self.store
            .create_otp_challenge(OtpChallengeRecord {
                id: id.clone(),
                account: account.clone(),
                code_hash,
                expires_at: self.clock.now() + self.ttl,
                attempts: 0,
                max_attempts: self.max_attempts,
            })
            .await?;
        Ok(id)
    }

    fn mint_code(&self) -> Result<OtpCode, AuthError> {
        let mut digits = String::with_capacity(self.digits as usize);
        while digits.len() < self.digits as usize {
            // Rejection sampling. `byte % 10` is biased towards 0..=5 because
            // 10 does not divide 256, and a biased first digit is a sixth of a
            // digit handed to the guesser for free.
            for byte in random_bytes(self.digits as usize)? {
                if byte < 250 {
                    digits.push(char::from(b'0' + byte % 10));
                    if digits.len() == self.digits as usize {
                        break;
                    }
                }
            }
        }
        Ok(OtpCode(digits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_the_right_length_and_every_digit_occurs() {
        let store = Arc::new(crate::memory::MemoryStore::new());
        let otp =
            Otp::new(
                store,
                Arc::new(crate::clock::TestClock::default()),
                Arc::new(
                    crate::password::Passwords::new(
                        crate::password::PasswordCost::insecure_for_tests(),
                    )
                    .unwrap(),
                ),
            )
            .unwrap();

        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let code = otp.mint_code().unwrap();
            assert_eq!(code.as_str().len(), DEFAULT_DIGITS as usize);
            assert!(code.as_str().bytes().all(|b| b.is_ascii_digit()));
            seen.extend(code.as_str().chars());
        }
        // Rejection sampling must still reach every digit; a `% 10` bug would
        // show up as a skew rather than a missing digit, which is what the
        // chi-squared-shaped assertion below is for.
        assert_eq!(seen.len(), 10, "not every digit occurred: {seen:?}");
    }

    #[test]
    fn absurd_parameters_are_refused() {
        let store = Arc::new(crate::memory::MemoryStore::new());
        let build = || {
            Otp::new(
                store.clone(),
                Arc::new(crate::clock::TestClock::default()),
                Arc::new(
                    crate::password::Passwords::new(
                        crate::password::PasswordCost::insecure_for_tests(),
                    )
                    .unwrap(),
                ),
            )
            .unwrap()
        };
        assert!(build().with_digits(5).is_err());
        assert!(build().with_digits(11).is_err());
        assert!(build().with_max_attempts(0).is_err());
        assert!(build().with_max_attempts(11).is_err());
        assert!(build().with_ttl(59).is_err());
        assert!(build().with_ttl(3601).is_err());
    }
}
