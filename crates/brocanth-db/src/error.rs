//! What can go wrong between an application and its database.
//!
//! Two rules shape this type.
//!
//! **No variant holds the URL.** A database URL carries a password, and an
//! error goes to a log, a panic message and sometimes a browser. A
//! configuration error names the scheme or the setting, never the URL.
//!
//! **Display ends with a `help:` line when there is something to do.** The
//! Postgres message says what happened. The help line says what to change:
//! raise `statement_timeout`, check `search_path`, drop `read_only`.

use thiserror::Error;

/// A database operation that did not happen.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum DbError {
    /// A [`DbConfig`](crate::DbConfig) this crate cannot use. Nothing reached
    /// the network.
    #[error("database config: {message}\nhelp: {help}")]
    Config {
        /// What is wrong.
        message: String,
        /// What to change.
        help: String,
    },

    /// The pool could not open a connection.
    #[error("{message}\nhelp: {help}")]
    Connect {
        /// What the server or the network said.
        message: String,
        /// What to check.
        help: &'static str,
        /// The driver error.
        #[source]
        source: sqlx::Error,
    },

    /// A statement failed: the server refused it, the pool had no free
    /// connection in time, or a column did not decode.
    #[error("{}", query_display(.message, .code.as_deref(), *.help))]
    Query {
        /// The Postgres error code (SQLSTATE), when the server sent one.
        code: Option<String>,
        /// The server's message, or the driver's.
        message: String,
        /// What to change, when the code says something useful.
        help: Option<&'static str>,
        /// The driver error.
        #[source]
        source: sqlx::Error,
    },

    /// [`Exec::one`](crate::Exec::one) found no row.
    #[error("the query returned no row\nhelp: use `optional` when no row is a normal answer")]
    NotFound,

    /// A row from [`Exec::json`](crate::Exec::json) was not valid JSON.
    #[error("a row did not decode as JSON: {0}")]
    Json(#[source] serde_json::Error),
}

fn query_display(message: &str, code: Option<&str>, help: Option<&str>) -> String {
    let mut out = match code {
        Some(code) => format!("query failed: {message} (SQLSTATE {code})"),
        None => format!("query failed: {message}"),
    };
    if let Some(help) = help {
        out.push_str("\nhelp: ");
        out.push_str(help);
    }
    out
}

impl DbError {
    /// The Postgres error code (SQLSTATE) of a failed statement, when the
    /// server sent one.
    pub fn code(&self) -> Option<&str> {
        match self {
            DbError::Query { code, .. } => code.as_deref(),
            _ => None,
        }
    }

    /// Whether a unique constraint rejected the write (SQLSTATE `23505`).
    pub fn is_unique_violation(&self) -> bool {
        self.code() == Some("23505")
    }

    /// Whether [`Exec::one`](crate::Exec::one) found no row.
    pub fn is_not_found(&self) -> bool {
        matches!(self, DbError::NotFound)
    }

    pub(crate) fn config(message: impl Into<String>, help: impl Into<String>) -> Self {
        DbError::Config {
            message: message.into(),
            help: help.into(),
        }
    }

    /// Wraps a driver error from a statement.
    pub(crate) fn query(source: sqlx::Error) -> Self {
        if matches!(source, sqlx::Error::RowNotFound) {
            return DbError::NotFound;
        }
        let (code, message) = match source.as_database_error() {
            Some(db) => (db.code().map(|c| c.into_owned()), db.message().to_owned()),
            None => (None, source.to_string()),
        };
        let help = match (&source, code.as_deref()) {
            (sqlx::Error::PoolTimedOut, _) => Some(
                "every connection was busy for `acquire_timeout`; raise `max_connections`, \
                 or find the code that holds a connection or a transaction too long",
            ),
            (_, Some(code)) => help_for(code),
            _ => None,
        };
        DbError::Query {
            code,
            message,
            help,
            source,
        }
    }

    /// Wraps a driver error from opening the pool.
    pub(crate) fn connect(source: sqlx::Error) -> Self {
        let code = source
            .as_database_error()
            .and_then(|e| e.code())
            .map(|c| c.into_owned());
        let (message, help) = match (&source, code.as_deref()) {
            (_, Some("28P01" | "28000")) => (
                "the server refused the user or the password".to_owned(),
                "check the user and the password in the URL",
            ),
            (_, Some("3D000")) => (
                "the database in the URL does not exist".to_owned(),
                "create it (`createdb <name>`) or fix the path of the URL",
            ),
            (sqlx::Error::Io(e), _) => (
                format!("cannot reach the database server: {e}"),
                "check the host and the port, and that the server runs",
            ),
            (sqlx::Error::Tls(e), _) => (
                format!("the TLS handshake failed: {e}"),
                "check `ca_file` and the `sslmode` of the URL; `verify-full` needs \
                 the host name in the URL to match the certificate",
            ),
            (sqlx::Error::PoolTimedOut, _) => (
                "no connection opened within `acquire_timeout`".to_owned(),
                "check the host, the port and the network, or raise `acquire_timeout`",
            ),
            (_, _) => match source.as_database_error() {
                Some(db) => (
                    db.message().to_owned(),
                    "read the server message above; it names the setting it refused",
                ),
                None => (
                    source.to_string(),
                    "check the URL and the server; the message above is from the driver",
                ),
            },
        };
        DbError::Connect {
            message,
            help,
            source,
        }
    }
}

/// A statement run straight on [`Db::pool`](crate::Db::pool) gets the same
/// error, and the same `help:` line, as one run through [`Exec`](crate::Exec).
/// `brocanth-search` runs its ranked queries this way.
impl From<sqlx::Error> for DbError {
    fn from(source: sqlx::Error) -> Self {
        DbError::query(source)
    }
}

/// A help line for the Postgres errors an application causes most.
fn help_for(code: &str) -> Option<&'static str> {
    Some(match code {
        "57014" => {
            "the statement ran longer than `statement_timeout`; add an index, \
             narrow the query, or raise `statement_timeout`"
        }
        "25006" => {
            "this connection is read-only (`DbConfig::read_only`); write through \
             a `Db` opened without it"
        }
        "42P01" => {
            "no such table on the `search_path`; check `DbConfig::search_path` \
             and that the migration ran"
        }
        "3F000" => "no such schema; check `DbConfig::search_path`",
        "42703" => "no such column; check the statement against the migration",
        "40001" => "a serialization failure; run the transaction again",
        "40P01" => "a deadlock; take locks in one order, or run the transaction again",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_says_what_to_do() {
        let e = DbError::query(sqlx::Error::RowNotFound);
        assert!(e.is_not_found());
        assert!(e.to_string().contains("help: use `optional`"));
    }

    #[test]
    fn pool_timeout_has_help() {
        let e = DbError::query(sqlx::Error::PoolTimedOut);
        let text = e.to_string();
        assert!(text.contains("help: every connection was busy"), "{text}");
    }
}
