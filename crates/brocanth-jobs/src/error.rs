//! What can go wrong between an application and the queue.
//!
//! A *job* failing is not one of these: that is a [`JobFailure`], it is
//! expected, and the queue's answer to it is a retry and then the dead letter.
//! An [`Error`] is the queue itself failing — the database is unreachable, a
//! payload will not serialise, a cron expression does not parse.
//!
//! [`JobFailure`]: crate::JobFailure

use thiserror::Error;

/// A queue operation that did not happen.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The database said no.
    #[error("the jobs database: {0}")]
    Database(#[from] sqlx::Error),

    /// A job value would not serialise into its payload.
    #[error("the payload for job `{job}` will not serialise: {source}")]
    Encode {
        /// The job's [`Job::NAME`](crate::Job::NAME).
        job: &'static str,
        /// What `serde_json` said.
        #[source]
        source: serde_json::Error,
    },

    /// A stored payload would not deserialise back into its job.
    #[error("the stored payload for job `{job}` will not deserialise: {source}")]
    Decode {
        /// The job name recorded on the row.
        job: String,
        /// What `serde_json` said.
        #[source]
        source: serde_json::Error,
    },

    /// A cron expression did not parse.
    #[error("`{expression}` is not a cron expression: {reason}")]
    Cron {
        /// What was handed in.
        expression: String,
        /// Why it was rejected.
        reason: String,
    },

    /// A schedule named a job the registry does not know, or a row names one.
    ///
    /// The worker reports this rather than guessing, because a job name that
    /// silently does nothing is how a queue goes quiet without anybody noticing.
    #[error("no job named `{name}` is registered on this worker; registered: {known}")]
    UnknownJob {
        /// The name on the row.
        name: String,
        /// What this worker does know, comma-separated.
        known: String,
    },

    /// The database URL names a backend this build does not contain.
    ///
    /// The backend is a Cargo feature, so this is a `Cargo.toml` problem rather
    /// than a configuration problem, and the message says so.
    #[error("{0}")]
    Backend(String),

    /// A worker was asked to start without a Tokio runtime to start on.
    #[error("there is no Tokio runtime here to run workers on: {0}")]
    NoRuntime(String),
}

/// The result of a queue operation.
pub type Result<T> = std::result::Result<T, Error>;
