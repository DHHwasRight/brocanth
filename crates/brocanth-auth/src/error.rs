//! One error type, and a deliberate shortage of variants where a variant would
//! answer a question the caller is not allowed to ask.

use thiserror::Error;

/// Everything this crate can fail with.
///
/// Note what is *not* here: there is no `UnknownAccount`, no `EmailNotFound`
/// and no `AccountExists`. Login answers [`AuthError::InvalidCredentials`] for
/// a wrong password and for an address nobody has registered, because the
/// difference between those two is exactly the fact an attacker enumerating a
/// user list is trying to learn. See [`crate::accounts`].
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AuthError {
    /// The credentials did not authenticate anybody. This is the only answer
    /// login ever gives, whatever the real reason was.
    #[error("invalid credentials")]
    InvalidCredentials,

    /// A single-use token was wrong, already spent, or past its expiry. One
    /// variant for the same reason as above: "expired" and "never existed" are
    /// different facts and the holder of a bad token is entitled to neither.
    #[error("the token is not valid")]
    InvalidToken,

    /// The `state` parameter on the callback did not match the one the flow
    /// started with. Treat it as CSRF and abandon the flow.
    #[error("the OAuth state does not match; the callback is not for this flow")]
    StateMismatch,

    /// The id_token's `nonce` claim did not match the flow's. Treat it as a
    /// replayed id_token.
    #[error("the OIDC nonce does not match; the id_token is not for this flow")]
    NonceMismatch,

    /// The flow state is older than the flow is allowed to take.
    #[error("the authorization flow expired before the callback arrived")]
    FlowExpired,

    /// The provider answered the authorization request with an error.
    #[error("the provider refused the authorization: {error}{}\nhelp: the person said no, or the app's registration at the provider is wrong (client id, redirect URI, scopes)", .description.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())]
    ProviderRefused {
        /// The provider's `error` code.
        error: String,
        /// The provider's `error_description`, when it sent one.
        description: Option<String>,
    },

    /// The provider's response could not be understood.
    #[error("the provider `{provider}` returned a response this crate cannot read: {detail}\nhelp: check the provider's endpoints in its catalog entry; the provider may have changed its API")]
    ProviderResponse {
        /// Provider id.
        provider: String,
        /// What went wrong.
        detail: String,
    },

    /// The provider's profile response did not contain a field the provider's
    /// [`ProfileMap`] says it does. The mapping is wrong, or the provider
    /// changed.
    ///
    /// [`ProfileMap`]: crate::providers::ProfileMap
    #[error("the provider `{provider}` returned no `{field}`; its profile mapping expects one\nhelp: check the scopes the app asks for, and the provider's ProfileMap")]
    ProfileFieldMissing {
        /// Provider id.
        provider: String,
        /// The logical field: `subject`, `email`, and so on.
        field: &'static str,
    },

    /// The session cookie was absent, unsigned, forged, idle too long or past
    /// its absolute lifetime.
    #[error("no valid session")]
    NoSession,

    /// An impersonated session attempted something only the account holder
    /// may do.
    ///
    /// Not a credential failure and not a bug: the operator is exactly who they
    /// said they were. See [`crate::impersonation::Restricted`] for the list
    /// and for which type refuses each item.
    #[error("an impersonated session may not do that")]
    Impersonated,

    /// The action needs a session that re-proved its identity recently, and
    /// this one has not.
    ///
    /// The caller's move is a step-up: ask for a factor, then
    /// [`Sessions::rotate`] to [`Privilege::Elevated`].
    ///
    /// [`Sessions::rotate`]: crate::session::Sessions::rotate
    /// [`Privilege::Elevated`]: crate::session::Privilege::Elevated
    #[error("that action needs a recently re-proved session")]
    ElevationRequired,

    /// The account already has this factor, confirmed. Enrolling again would
    /// replace the working one; remove it first.
    #[error("that factor is already enrolled on this account\nhelp: remove it before you enrol a new one")]
    AlreadyEnrolled,

    /// The provider identity is already linked to a different account.
    #[error("that provider account is already linked to another account")]
    AlreadyLinked,

    /// The address is not a usable email address.
    #[error("`{0}` is not a usable email address")]
    InvalidEmail(String),

    /// A configuration value is wrong, and it is wrong before any request is
    /// served.
    #[error("auth is misconfigured: {}", with_help(.0, "fix the value this message names where the application builds its auth config"))]
    Misconfigured(String),

    /// The storage implementation failed.
    #[error("the auth store failed: {}", with_help(.0, "check that the database is reachable and its tables exist (PgStore::migrate)"))]
    Store(String),

    /// The HTTP client failed.
    #[error("the request to the provider failed: {}", with_help(.0, "check that this host reaches the provider over HTTPS, and retry"))]
    Http(String),

    /// Hashing, signing or verifying failed for a reason that is not a wrong
    /// password.
    #[error("a cryptographic operation failed: {}", with_help(.0, "check the signing key (32 bytes, base64url) and the operating system's random source"))]
    Crypto(String),
}

/// `message`, with `help` as its `help:` line unless it has one already.
///
/// The operator-facing variants carry a `help:` line. Many messages are
/// built where the exact next step is known and say it; the rest get the
/// variant's general advice.
fn with_help(message: &str, help: &str) -> String {
    if message.contains("\nhelp:") {
        message.to_string()
    } else {
        format!("{message}\nhelp: {help}")
    }
}

impl AuthError {
    /// Builds a [`AuthError::Misconfigured`].
    pub fn misconfigured(what: impl Into<String>) -> Self {
        Self::Misconfigured(what.into())
    }

    /// Builds a [`AuthError::Crypto`].
    pub fn crypto(what: impl Into<String>) -> Self {
        Self::Crypto(what.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_operator_error_says_what_to_do_next() {
        for err in [
            AuthError::misconfigured("the key is short"),
            AuthError::Store("connection refused".into()),
            AuthError::Http("timed out".into()),
            AuthError::crypto("no entropy"),
            AuthError::ProviderRefused {
                error: "access_denied".into(),
                description: None,
            },
            AuthError::ProviderResponse {
                provider: "github".into(),
                detail: "not JSON".into(),
            },
            AuthError::ProfileFieldMissing {
                provider: "github".into(),
                field: "subject",
            },
            AuthError::from(crate::storage::StoreError::backend("connection refused")),
        ] {
            let text = err.to_string();
            assert_eq!(text.matches("\nhelp: ").count(), 1, "{text}");
        }
        // A message that names its own next step keeps it, and only it.
        let own = AuthError::misconfigured("bad\nhelp: do this");
        assert!(own.to_string().ends_with("help: do this"), "{own}");
        // What a person reads stays plain.
        assert_eq!(
            AuthError::InvalidCredentials.to_string(),
            "invalid credentials"
        );
    }
}
