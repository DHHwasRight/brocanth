//! What can go wrong between an application and the cache.
//!
//! A **miss is not an error**. A miss is `Ok(None)`, because a miss is the
//! ordinary case a cache exists to have, and a type that makes you `match` on
//! an error to find out whether a key was there teaches you to ignore errors.
//! The same rule is why [`Storage::variant`] answers `Option` where
//! [`Storage::get`] answers `NotFound`.
//!
//! An expired entry is also a miss, not an error. See [`crate::cache`].
//!
//! [`Storage::variant`]: https://docs.rs/brocanth-storage
//! [`Storage::get`]: https://docs.rs/brocanth-storage

use thiserror::Error;

/// A cache operation that did not happen.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum CacheError {
    /// The backend said no: the database is unreachable, the pool is
    /// exhausted, a statement failed.
    ///
    /// Boxed rather than typed as [`sqlx::Error`] on purpose. The error type
    /// has to be the same type whether or not the `postgres` feature is on,
    /// because a SQLite backend in a later crate implements the same
    /// [`Cache`](crate::Cache) trait and returns the same error, and an enum
    /// whose variants change with a feature is an enum callers cannot match on.
    #[error("the cache backend: {0}")]
    Backend(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The value is bigger than the backend's cap.
    ///
    /// This is an error rather than a silent skip, and that is a decision
    /// worth defending: a cache that quietly refuses to store the one value
    /// you cared about is a cache that is always a miss and never says so.
    /// You find this in development and either raise the cap with
    /// [`PgCache::with_max_value_bytes`] or stop caching the thing.
    ///
    /// See [`Cache::max_value_bytes`](crate::Cache::max_value_bytes) for what
    /// the default is and why.
    ///
    /// [`PgCache::with_max_value_bytes`]: crate::PgCache::with_max_value_bytes
    #[error(
        "the value for `{namespace}/{key}` is {bytes} bytes, over this cache's \
         {limit}-byte cap"
    )]
    TooLarge {
        /// The namespace the key is in.
        namespace: String,
        /// The key.
        key: String,
        /// How big the value was.
        bytes: usize,
        /// How big a value is allowed to be.
        limit: usize,
    },

    /// A namespace name is not usable.
    ///
    /// See [`Namespace`](crate::Namespace) for the rules and why there are any.
    #[error("`{name}` is not a usable cache namespace: {reason}")]
    InvalidNamespace {
        /// What was handed in.
        name: String,
        /// Why it was rejected.
        reason: &'static str,
    },

    /// A key is not usable.
    #[error("that cache key is not usable: {reason}")]
    InvalidKey {
        /// Why it was rejected.
        reason: &'static str,
    },

    /// A tag is not usable.
    ///
    /// See [`Tag`](crate::Tag) for the rules.
    #[error("`{tag}` is not a usable cache tag: {reason}")]
    InvalidTag {
        /// What was handed in.
        tag: String,
        /// Why it was rejected.
        reason: &'static str,
    },

    /// A time to live of zero, or one so long it overflows an instant.
    ///
    /// A zero TTL is rejected rather than treated as "store it and expire it
    /// immediately", because the only thing that distinguishes the two is
    /// whether you meant it, and a write that is a no-op is the bug this crate
    /// is least able to help you find. Use
    /// [`Cache::delete`](crate::Cache::delete) to remove an entry.
    #[error("{0} is not a usable time to live")]
    InvalidTtl(String),

    /// The database URL names something this build cannot talk to.
    ///
    /// The backend is a Cargo feature, so this is usually a `Cargo.toml`
    /// problem rather than a configuration problem, and the message says so.
    #[error("{0}")]
    Config(String),
}

impl CacheError {
    /// Wraps a backend's own error.
    ///
    /// For an implementation of [`Cache`](crate::Cache) living in another
    /// crate: this is how its driver error becomes a [`CacheError`].
    pub fn backend(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        CacheError::Backend(Box::new(source))
    }
}

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for CacheError {
    fn from(source: sqlx::Error) -> Self {
        CacheError::Backend(Box::new(source))
    }
}

/// The result of a cache operation.
pub type Result<T> = std::result::Result<T, CacheError>;
