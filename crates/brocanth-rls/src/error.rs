//! [`RlsError`]: a definition this crate refuses, a file it cannot read, or a
//! statement Postgres refused.

use std::path::PathBuf;

/// What went wrong. Each message ends with a `help:` line that says what to
/// change.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RlsError {
    /// A definition that cannot become valid SQL, or a schema whose parts do
    /// not fit: an unknown role, a policy on an undeclared table, a cycle.
    #[error("{message}\nhelp: {help}")]
    Definition {
        /// What is wrong.
        message: String,
        /// What to change.
        help: String,
    },

    /// A snapshot or migration file could not be read or written.
    #[error("{}: {source}\nhelp: {help}", .path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: std::io::Error,
        /// What to check.
        help: &'static str,
    },

    /// `migrations/rls.json` is not a snapshot this crate wrote.
    #[error("{}: {source}\nhelp: restore the file from version control; it is written by `rls make`, not by hand", .path.display())]
    Snapshot {
        /// The file.
        path: PathBuf,
        /// The parse error.
        #[source]
        source: serde_json::Error,
    },

    /// The migration file could not be made: a name that is not snake case,
    /// or a file that exists.
    #[error(transparent)]
    Migrate(#[from] brocanth_db::migrate::MigrateError),

    /// A statement against the database failed.
    #[error(transparent)]
    Db(#[from] brocanth_db::DbError),
}

impl RlsError {
    /// A `brocanth-db` refusal of a name or key, as a definition error.
    pub(crate) fn from_config(e: brocanth_db::DbError) -> RlsError {
        match e {
            brocanth_db::DbError::Config { message, help } => {
                RlsError::Definition { message, help }
            }
            other => RlsError::Db(other),
        }
    }

    pub(crate) fn definition(message: impl Into<String>, help: impl Into<String>) -> RlsError {
        RlsError::Definition {
            message: message.into(),
            help: help.into(),
        }
    }
}
