//! One error type, and no field in it that an attacker chose.

/// Everything this crate can fail with.
///
/// Note what the refusals do **not** carry: the text that was refused. An
/// address and a display name arrive from a form, a reset screen or a provider
/// profile, and an error that quotes them writes attacker-chosen bytes into a
/// log line -- which is the same injection one layer down. `field` and
/// `reason` are both `&'static str` picked from this crate's own vocabulary,
/// so nothing a stranger typed reaches a log through here.
/// [`brocanth-storage`'s `StorageError::InvalidKey`](https://docs.rs/brocanth-storage)
/// makes the same cut for the same reason.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MailError {
    /// The text is not a mailbox address this crate will put on an envelope.
    #[error("that is not a usable email address: {reason}")]
    InvalidAddress {
        /// Which rule it broke, from a fixed list.
        reason: &'static str,
    },

    /// The text carries something a header must not carry.
    ///
    /// A CR or an LF is the whole security case of this crate: one newline in
    /// a subject or a display name ends the header and starts another, and the
    /// next one an attacker writes is `Bcc:`.
    #[error("the {field} cannot go in a header: {reason}")]
    InvalidHeader {
        /// Which header, from a fixed list: `subject`, `display name`.
        field: &'static str,
        /// Which rule it broke, from a fixed list.
        reason: &'static str,
    },

    /// A message with no `To` and no `Cc`.
    ///
    /// Refused at construction rather than at the relay, because a relay
    /// refuses it with a 554 at three in the morning.
    #[error("a message needs at least one recipient")]
    NoRecipient,

    /// The backend could not deliver.
    ///
    /// A string, because this crate cannot usefully tell a refused
    /// authentication from a closed socket, and pretending otherwise would put
    /// a `match` in callers that can only retry either way. The text is the
    /// backend's own.
    #[error("the message was not delivered: {0}")]
    Transport(String),
}

impl MailError {
    /// Builds a [`MailError::Transport`].
    pub fn transport(what: impl Into<String>) -> Self {
        Self::Transport(what.into())
    }

    pub(crate) fn address(reason: &'static str) -> Self {
        Self::InvalidAddress { reason }
    }

    pub(crate) fn header(field: &'static str, reason: &'static str) -> Self {
        Self::InvalidHeader { field, reason }
    }
}
