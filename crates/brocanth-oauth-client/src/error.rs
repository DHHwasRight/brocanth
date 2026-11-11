//! What can fail. Each message ends with a `help:` line that says what to
//! do next. No message holds a token.

use crate::store::StoreError;

/// What failed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No provider has this id.
    #[error(
        "oauth-client: no provider `{0}`\n\
         help: register it with `OauthClient::builder(..).provider(..)`, or check the id"
    )]
    UnknownProvider(String),

    /// The account has no token for this provider.
    #[error(
        "oauth-client: account `{account}` has not connected `{provider}`\n\
         help: send the person to the connect page for `{provider}` first"
    )]
    NotConnected {
        /// The account.
        account: String,
        /// The provider id.
        provider: String,
    },

    /// The connection no longer works: the refresh token was revoked, has
    /// expired, or never existed. Only the person can fix it.
    #[error(
        "oauth-client: the `{provider}` connection of account `{account}` no longer works: {reason}\n\
         help: send the person to connect `{provider}` again; the old grant is gone"
    )]
    Reconnect {
        /// The account.
        account: String,
        /// The provider id.
        provider: String,
        /// What the provider said, or why no refresh was possible.
        reason: String,
    },

    /// The connect flow failed. The message is safe to show a visitor.
    #[error("oauth-client: {0}")]
    Flow(#[from] FlowError),

    /// The provider answered a token request with an error, or the request
    /// did not reach it. Worth a retry later.
    #[error(
        "oauth-client: the token request to `{provider}` failed: {message}\n\
         help: check the client id, the secret and the token URL of `{provider}`; \
         a network error is worth a retry"
    )]
    Provider {
        /// The provider id.
        provider: String,
        /// What went wrong. Never a token.
        message: String,
    },

    /// The store failed.
    #[error("oauth-client: {0}\nhelp: check the token store; with Postgres, check `[db] url`")]
    Store(#[from] StoreError),

    /// A sealed token did not open: the key changed, or the row was edited.
    #[error(
        "oauth-client: a stored token does not open with this key ({0})\n\
         help: the key in `[oauth_client] key` changed, or the row was moved to another \
         account; disconnect and connect again"
    )]
    Crypto(String),

    /// The configuration is wrong.
    #[error("oauth-client: {0}")]
    Config(String),

    /// An API request through [`crate::Authorized`] failed to send.
    #[error("oauth-client: the API request failed: {0}\nhelp: check the URL; a network error is worth a retry")]
    Http(String),
}

/// Why a connect flow was refused. Each message is safe to show a visitor.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlowError {
    /// The callback came with no flow cookie: another browser, or a
    /// cookie that ran out.
    #[error(
        "this connection did not start in this browser, or it took longer than ten minutes\n\
         help: start again from the connect page"
    )]
    NotStarted,
    /// The flow cookie did not open: it was changed, or another key sealed it.
    #[error("this connection is not valid\nhelp: start again from the connect page")]
    Tampered,
    /// The flow is older than ten minutes.
    #[error(
        "this connection took longer than ten minutes\nhelp: start again from the connect page"
    )]
    Expired,
    /// The `state` in the callback is not the one this browser started with.
    #[error(
        "the provider's answer does not belong to this connection\n\
         help: start again from the connect page"
    )]
    StateMismatch,
    /// The flow started for another provider.
    #[error("this answer is for another provider\nhelp: start again from the connect page")]
    WrongProvider,
    /// Somebody else is signed in now than when the flow started.
    #[error(
        "you are signed in as somebody else than when this connection started\n\
         help: start again from the connect page"
    )]
    WrongAccount,
    /// The provider said no, or the person cancelled.
    #[error("the provider did not grant access ({0})\nhelp: start again and accept the request")]
    Denied(String),
    /// The callback has no `code`.
    #[error("the provider's answer has no code\nhelp: start again from the connect page")]
    NoCode,
}
