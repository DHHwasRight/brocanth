//! One error type, and a deliberate shortage of variants where a variant would
//! answer a question the caller is not allowed to ask.

/// Everything this crate can fail with.
///
/// Note what is *not* here. There is no `Expired`, no `Forged` and no
/// `Malformed` for a file token: all three are [`StorageError::InvalidToken`],
/// because the holder of a bad token is entitled to none of those three facts
/// and the difference between them is exactly what an attacker probes for.
/// [`brocanth-auth`'s `AuthError`](https://docs.rs/brocanth-auth) makes the
/// same cut for the same reason.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StorageError {
    /// Nothing is stored at that address.
    #[error("no object is stored at that address")]
    NotFound,

    /// The text is not an object address.
    ///
    /// One variant and no detail. The text came from a URL, and an error that
    /// repeats it puts attacker-chosen bytes in a log line.
    #[error("that is not an object address")]
    InvalidKey,

    /// The text is not a size specification. The reason is a fixed string, so
    /// nothing a client sent reaches a log through here.
    #[error("that is not a size specification: {0}")]
    InvalidSpec(&'static str),

    /// A file token was forged, malformed, or past its expiry.
    #[error("the file token is not valid")]
    InvalidToken,

    /// The stored bytes are not an image this crate can derive a variant from.
    #[error("the object is not an image")]
    NotAnImage,

    /// The image declares more pixels than this crate will decode.
    ///
    /// Raised from the header, before any decoding. See
    /// [`MAX_SOURCE_PIXELS`](crate::MAX_SOURCE_PIXELS).
    #[error("the image declares {width}x{height}, which is past the decode limit")]
    ImageTooLarge {
        /// Declared width, from the header.
        width: u32,
        /// Declared height, from the header.
        height: u32,
    },

    /// The image could not be decoded, resized or re-encoded.
    #[error("the image could not be processed: {0}")]
    Image(String),

    /// The object is bigger than the backend will store.
    ///
    /// Distinct from [`StorageError::Backend`] because it is the one backend
    /// refusal a caller can act on: it is a 413, it is the client's fault, and
    /// retrying it is pointless. Both numbers are the backend's own, so
    /// nothing a client sent reaches a log through here.
    #[error("the object is {size} bytes, past this backend's limit of {limit}")]
    TooLarge {
        /// How many bytes the caller tried to store.
        size: u64,
        /// The most this backend stores in one object.
        limit: u64,
    },

    /// Anything the backend failed at.
    ///
    /// A string because this crate cannot usefully tell a full disk from a
    /// revoked credential, and pretending otherwise would put a `match` in
    /// callers that can only log either way.
    #[error("{0}")]
    Backend(String),
}

impl StorageError {
    /// Builds a [`StorageError::Backend`].
    pub fn backend(what: impl Into<String>) -> Self {
        Self::Backend(what.into())
    }

    /// Turns an I/O error into either [`StorageError::NotFound`] or a
    /// [`StorageError::Backend`] carrying `context`.
    ///
    /// The context names the operation, never the path: a path contains the
    /// object address, and an address that reaches an error page is an address
    /// that reaches a log aggregator.
    pub(crate) fn io(context: &'static str, e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::NotFound {
            Self::NotFound
        } else {
            Self::Backend(format!("{context}: {e}"))
        }
    }
}
