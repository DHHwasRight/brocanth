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
    #[error("the provider refused the authorization: {error}{}", .description.as_deref().map(|d| format!(" ({d})")).unwrap_or_default())]
    ProviderRefused {
        /// The provider's `error` code.
        error: String,
        /// The provider's `error_description`, when it sent one.
        description: Option<String>,
    },

    /// The provider's response could not be understood.
    #[error("the provider `{provider}` returned a response this crate cannot read: {detail}")]
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
    #[error("the provider `{provider}` returned no `{field}`; its profile mapping expects one")]
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

    /// The provider identity is already linked to a different account.
    #[error("that provider account is already linked to another account")]
    AlreadyLinked,

    /// The address is not a usable email address.
    #[error("`{0}` is not a usable email address")]
    InvalidEmail(String),

    /// A configuration value is wrong, and it is wrong before any request is
    /// served.
    #[error("auth is misconfigured: {0}")]
    Misconfigured(String),

    /// The storage implementation failed.
    #[error("the auth store failed: {0}")]
    Store(String),

    /// The HTTP client failed.
    #[error("the request to the provider failed: {0}")]
    Http(String),

    /// Hashing, signing or verifying failed for a reason that is not a wrong
    /// password.
    #[error("a cryptographic operation failed: {0}")]
    Crypto(String),
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
