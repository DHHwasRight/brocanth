//! The common identity: what every provider is mapped onto, and what the
//! application stores.

use serde::{Deserialize, Serialize};

use crate::clock::UnixTime;
use crate::error::AuthError;
use crate::random::random_secret;

/// An account's primary key, as an opaque string.
///
/// It is a string and not an integer because this crate does not know what
/// your database numbers things with. A SeaORM adapter maps its `i64` in and
/// out; [`MemoryStore`] mints a random one. Nothing here parses it.
///
/// [`MemoryStore`]: crate::memory::MemoryStore
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AccountId(String);

impl AccountId {
    /// Wraps an existing identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// A fresh random identifier, for a store that does not assign its own.
    ///
    /// # Errors
    /// See [`random_secret`].
    pub fn generate() -> Result<Self, AuthError> {
        Ok(Self(random_secret()?))
    }

    /// The identifier as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An email address, normalised once at the boundary.
///
/// Normalisation is trim plus lowercase, and nothing more. It is deliberately
/// not "strip dots and `+tags` the way Gmail does": that rule is Gmail's, not
/// email's, and applying it everywhere merges two addresses that genuinely
/// belong to two people at providers that treat them as distinct.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EmailAddress(String);

impl EmailAddress {
    /// Normalises and checks an address.
    ///
    /// The check is shallow on purpose — one `@`, something on each side, no
    /// whitespace. Deciding deliverability from a regular expression is a
    /// well-known way to reject real addresses; the only proof that an address
    /// works is that a message sent to it was opened, which is what
    /// [`TokenPurpose::EmailVerification`] is for.
    ///
    /// [`TokenPurpose::EmailVerification`]: crate::tokens::TokenPurpose::EmailVerification
    ///
    /// # Errors
    /// Returns [`AuthError::InvalidEmail`] when the address has no local part,
    /// no domain, more than one `@`, or any whitespace.
    pub fn parse(raw: &str) -> Result<Self, AuthError> {
        let trimmed = raw.trim();
        let lowered = trimmed.to_lowercase();
        let mut parts = lowered.split('@');
        let (local, domain) = match (parts.next(), parts.next(), parts.next()) {
            (Some(l), Some(d), None) => (l, d),
            _ => return Err(AuthError::InvalidEmail(raw.to_string())),
        };
        if local.is_empty()
            || domain.is_empty()
            || !domain.contains('.')
            || lowered.chars().any(char::is_whitespace)
        {
            return Err(AuthError::InvalidEmail(raw.to_string()));
        }
        Ok(Self(lowered))
    }

    /// The normalised address.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EmailAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// An account, as this crate understands one.
///
/// Everything else your application knows about a user — a display name, a
/// role, a billing plan — belongs to your own table, keyed by [`Account::id`].
/// This crate stores the minimum it needs to authenticate somebody and nothing
/// that would make it a user model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// Primary key.
    pub id: AccountId,
    /// The address, when the account has one. An account created from a
    /// provider that returned no email has none.
    pub email: Option<EmailAddress>,
    /// Whether *we* have proof the address reaches this person: either they
    /// opened a verification link, or a provider we trust asserted it.
    pub email_verified: bool,
    /// The PHC string, when the account has a password. An account that only
    /// signs in with a provider has none, and [`Accounts::login`] must not
    /// treat that as "any password works".
    ///
    /// [`Accounts::login`]: crate::accounts::Accounts::login
    pub password_hash: Option<String>,
    /// When the account was created.
    pub created_at: UnixTime,
}

/// A provider profile, mapped onto the common shape.
///
/// This is the one type the flow code produces and the only thing [`linking`]
/// consumes, which is what makes a provider a struct literal rather than a
/// module.
///
/// [`linking`]: crate::linking
#[derive(Clone, Debug, PartialEq)]
pub struct ProviderIdentity {
    /// The provider's id: `google`, `github`, and so on.
    pub provider: String,
    /// The provider's stable identifier for this person — `sub`, or `id`.
    ///
    /// This, not the email, is the join key. An email can change hands; a
    /// subject does not.
    pub subject: String,
    /// The address the provider reported, when it reported one.
    pub email: Option<EmailAddress>,
    /// Whether the provider asserted the address is verified.
    ///
    /// A provider that never says is recorded as `false`, not as "probably".
    /// [`linking`] reads this field and refuses to act on `false`.
    ///
    /// [`linking`]: crate::linking
    pub email_verified: bool,
    /// A display name, when the provider gave one.
    pub name: Option<String>,
    /// An avatar URL, when the provider gave one.
    pub avatar_url: Option<String>,
    /// The raw profile document, for an application that wants a field this
    /// crate does not map.
    pub raw: serde_json::Value,
}

/// A provider account attached to a local account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderLink {
    /// The local account.
    pub account: AccountId,
    /// Provider id.
    pub provider: String,
    /// The provider's subject.
    pub subject: String,
    /// The address at the time of linking, kept for display only. It is not a
    /// lookup key: see [`ProviderIdentity::subject`].
    pub email: Option<EmailAddress>,
    /// When the link was made.
    pub linked_at: UnixTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_normalise_but_are_not_mangled() {
        assert_eq!(
            EmailAddress::parse("  Alice@Example.COM ")
                .unwrap()
                .as_str(),
            "alice@example.com"
        );
        // Gmail would treat these as one person. Email does not, and neither
        // do we.
        assert_ne!(
            EmailAddress::parse("a.b@example.com").unwrap(),
            EmailAddress::parse("ab@example.com").unwrap()
        );
        assert_ne!(
            EmailAddress::parse("a+x@example.com").unwrap(),
            EmailAddress::parse("a@example.com").unwrap()
        );
    }

    #[test]
    fn obvious_nonsense_is_refused() {
        for bad in [
            "",
            "alice",
            "@example.com",
            "alice@",
            "a@b",
            "a b@c.com",
            "a@b@c.com",
        ] {
            assert!(EmailAddress::parse(bad).is_err(), "accepted `{bad}`");
        }
    }
}
