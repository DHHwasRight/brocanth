//! What a pool needs to know: where the server is, how many connections to
//! hold, and the session every connection starts with.
//!
//! # Session settings live on the connection, not on the query
//!
//! `search_path`, `statement_timeout`, `default_transaction_read_only` and
//! `application_name` go into the startup packet (`-c key=value` in the
//! `options` parameter). The server applies them before the first statement.
//! So a connection the pool hands out is already in the right state, and no
//! query pays a `SET` round trip. A `SET` per query would also leak: a
//! transaction that fails after its `SET` leaves the next caller guessing.
//!
//! # No `connect_timeout`
//!
//! sqlx 0.9 has no per-connection connect timeout for Postgres, and it
//! ignores a `connect_timeout` in the URL. `acquire_timeout` bounds the wait
//! for a connection, and that wait includes opening one. So one setting
//! covers both, and this type does not offer a second one that does nothing.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};

use crate::error::DbError;

/// How to open a [`Db`](crate::Db).
///
/// Start from [`DbConfig::new`] and change the fields you need. The struct is
/// `non_exhaustive`, so a new setting is not a breaking change.
///
/// `Debug` hides the password in the URL.
#[derive(Clone)]
#[non_exhaustive]
#[cfg_attr(feature = "serde", derive(serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(deny_unknown_fields))]
pub struct DbConfig {
    /// `postgres://user:password@host:5432/database?sslmode=require`.
    ///
    /// `postgresql://` works too. A SQLAlchemy URL such as
    /// `postgresql+psycopg2://…` works as is: the `+driver` part goes away.
    pub url: String,

    /// The most connections the pool opens. Default 10.
    #[cfg_attr(feature = "serde", serde(default = "defaults::max_connections"))]
    pub max_connections: u32,

    /// The connections the pool keeps open when idle. Default 0.
    #[cfg_attr(feature = "serde", serde(default))]
    pub min_connections: u32,

    /// How long a caller waits for a connection, opening included.
    /// Default 30 s.
    #[cfg_attr(
        feature = "serde",
        serde(
            default = "defaults::acquire_timeout",
            deserialize_with = "duration::required"
        )
    )]
    pub acquire_timeout: Duration,

    /// How long an idle connection stays open. Default 10 min. `None` keeps
    /// it open.
    #[cfg_attr(
        feature = "serde",
        serde(
            default = "defaults::idle_timeout",
            deserialize_with = "duration::optional"
        )
    )]
    pub idle_timeout: Option<Duration>,

    /// How long a connection lives before the pool replaces it. Default
    /// 30 min, which spreads load again after a failover. `None` keeps it.
    #[cfg_attr(
        feature = "serde",
        serde(
            default = "defaults::max_lifetime",
            deserialize_with = "duration::optional"
        )
    )]
    pub max_lifetime: Option<Duration>,

    /// The server cancels a statement that runs longer. Default `None`: no
    /// limit. Postgres counts milliseconds, so a value under 1 ms becomes
    /// 1 ms.
    #[cfg_attr(
        feature = "serde",
        serde(default, deserialize_with = "duration::optional")
    )]
    pub statement_timeout: Option<Duration>,

    /// The schemas an unqualified name resolves in, first to last. Empty
    /// keeps the server default (`"$user", public`).
    #[cfg_attr(feature = "serde", serde(default))]
    pub search_path: Vec<String>,

    /// Every transaction starts read-only, so a write fails with SQLSTATE
    /// `25006`. For a reporting or analytics pool. Default `false`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub read_only: bool,

    /// The name `pg_stat_activity` shows for these connections.
    #[cfg_attr(feature = "serde", serde(default))]
    pub application_name: Option<String>,

    /// A PEM bundle of the CAs to trust. When set, the connection uses
    /// `sslmode=verify-full` against it, whatever the URL says. When unset,
    /// the URL's `sslmode` applies (sqlx's default is `prefer`).
    #[cfg_attr(feature = "serde", serde(default))]
    pub ca_file: Option<PathBuf>,
}

#[cfg(feature = "serde")]
mod defaults {
    use std::time::Duration;

    pub fn max_connections() -> u32 {
        super::DEFAULT_MAX_CONNECTIONS
    }
    pub fn acquire_timeout() -> Duration {
        super::DEFAULT_ACQUIRE_TIMEOUT
    }
    pub fn idle_timeout() -> Option<Duration> {
        Some(super::DEFAULT_IDLE_TIMEOUT)
    }
    pub fn max_lifetime() -> Option<Duration> {
        Some(super::DEFAULT_MAX_LIFETIME)
    }
}

const DEFAULT_MAX_CONNECTIONS: u32 = 10;
const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_MAX_LIFETIME: Duration = Duration::from_secs(30 * 60);

impl DbConfig {
    /// A config for `url`, with the defaults each field documents.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            max_connections: DEFAULT_MAX_CONNECTIONS,
            min_connections: 0,
            acquire_timeout: DEFAULT_ACQUIRE_TIMEOUT,
            idle_timeout: Some(DEFAULT_IDLE_TIMEOUT),
            max_lifetime: Some(DEFAULT_MAX_LIFETIME),
            statement_timeout: None,
            search_path: Vec::new(),
            read_only: false,
            application_name: None,
            ca_file: None,
        }
    }

    /// The sqlx connect options this config describes: the URL, the TLS
    /// settings and the session settings.
    ///
    /// # Errors
    /// [`DbError::Config`] for a URL that is not Postgres or does not parse,
    /// or a schema name that is empty.
    pub fn connect_options(&self) -> Result<PgConnectOptions, DbError> {
        let url = normalize_url(&self.url)?;
        let mut options = PgConnectOptions::from_str(&url).map_err(|e| {
            // sqlx's message comes from the URL parser and names the part
            // that failed, not the URL. Still, take only the message.
            DbError::config(
                format!("the URL does not parse: {e}"),
                "write it as postgres://user:password@host:5432/database; \
                 percent-encode special characters in the password",
            )
        })?;

        if let Some(name) = &self.application_name {
            options = options.application_name(name);
        }

        let mut session: Vec<(&str, String)> = Vec::new();
        if !self.search_path.is_empty() {
            session.push(("search_path", search_path_value(&self.search_path)?));
        }
        if let Some(timeout) = self.statement_timeout {
            let ms = timeout.as_millis().max(1);
            session.push(("statement_timeout", ms.to_string()));
        }
        if self.read_only {
            session.push(("default_transaction_read_only", "on".to_owned()));
        }
        if !session.is_empty() {
            options = options.options(session);
        }

        if let Some(ca) = &self.ca_file {
            options = options.ssl_mode(PgSslMode::VerifyFull).ssl_root_cert(ca);
        }
        Ok(options)
    }

    /// The sqlx pool options this config describes.
    ///
    /// # Errors
    /// [`DbError::Config`] when `max_connections` is 0 or under
    /// `min_connections`.
    pub fn pool_options(&self) -> Result<PgPoolOptions, DbError> {
        if self.max_connections == 0 {
            return Err(DbError::config(
                "`max_connections` is 0",
                "set it to 1 or more; a pool of zero cannot run a query",
            ));
        }
        if self.min_connections > self.max_connections {
            return Err(DbError::config(
                format!(
                    "`min_connections` ({}) is over `max_connections` ({})",
                    self.min_connections, self.max_connections
                ),
                "lower `min_connections` or raise `max_connections`",
            ));
        }
        Ok(PgPoolOptions::new()
            .max_connections(self.max_connections)
            .min_connections(self.min_connections)
            .acquire_timeout(self.acquire_timeout)
            .idle_timeout(self.idle_timeout)
            .max_lifetime(self.max_lifetime))
    }

    /// The URL with its password replaced by `***`, for a log line.
    pub fn redacted_url(&self) -> String {
        redact(&self.url)
    }
}

impl fmt::Debug for DbConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbConfig")
            .field("url", &self.redacted_url())
            .field("max_connections", &self.max_connections)
            .field("min_connections", &self.min_connections)
            .field("acquire_timeout", &self.acquire_timeout)
            .field("idle_timeout", &self.idle_timeout)
            .field("max_lifetime", &self.max_lifetime)
            .field("statement_timeout", &self.statement_timeout)
            .field("search_path", &self.search_path)
            .field("read_only", &self.read_only)
            .field("application_name", &self.application_name)
            .field("ca_file", &self.ca_file)
            .finish()
    }
}

/// `postgres://` or `postgresql://`, with a SQLAlchemy `+driver` suffix
/// removed. Any other scheme is an error that names the scheme only.
pub(crate) fn normalize_url(url: &str) -> Result<String, DbError> {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(DbError::config(
            "the database URL has no scheme",
            "write it as postgres://user:password@host:5432/database",
        ));
    };
    let base = scheme.split('+').next().unwrap_or(scheme);
    let base = base.to_ascii_lowercase();
    if base != "postgres" && base != "postgresql" {
        return Err(DbError::config(
            format!("`{scheme}` is not a Postgres URL scheme"),
            "brocanth-db speaks Postgres only; use postgres:// or postgresql://",
        ));
    }
    Ok(format!("{base}://{rest}"))
}

/// The `search_path` value: each schema as a quoted identifier, so a name
/// with a capital, a dash or a space means what it says.
fn search_path_value(schemas: &[String]) -> Result<String, DbError> {
    let mut parts = Vec::with_capacity(schemas.len());
    for schema in schemas {
        if schema.is_empty() {
            return Err(DbError::config(
                "`search_path` has an empty schema name",
                "remove the empty entry",
            ));
        }
        // `$user` is a token the server expands, so it stays as written.
        if schema == "$user" {
            parts.push("\"$user\"".to_owned());
        } else {
            parts.push(format!("\"{}\"", schema.replace('"', "\"\"")));
        }
    }
    Ok(parts.join(","))
}

/// Replaces the password of `scheme://user:password@host/…` with `***`.
fn redact(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return "***".to_owned();
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    match authority.rsplit_once('@') {
        Some((userinfo, host)) => match userinfo.split_once(':') {
            Some((user, _password)) => format!("{scheme}://{user}:***@{host}{tail}"),
            None => format!("{scheme}://{userinfo}@{host}{tail}"),
        },
        None => url.to_owned(),
    }
}

#[cfg(feature = "serde")]
mod duration {
    //! A duration in config: a number of seconds (`30`, `0.25`) or a string
    //! with a unit (`"250ms"`, `"30s"`, `"5m"`, `"1h"`). For an optional
    //! setting, `0` turns it off. Postgres reads `statement_timeout = 0` the
    //! same way.

    use std::time::Duration;

    use serde::{de::Error as _, Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Seconds(f64),
        Text(String),
    }

    fn parse(raw: Raw) -> Result<Duration, String> {
        match raw {
            Raw::Seconds(s) => {
                Duration::try_from_secs_f64(s).map_err(|_| format!("{s} is not a duration"))
            }
            Raw::Text(text) => {
                let t = text.trim();
                let split = t
                    .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                    .unwrap_or(t.len());
                let (number, unit) = t.split_at(split);
                let n: f64 = number
                    .parse()
                    .map_err(|_| format!("`{text}` is not a duration; write \"30s\" or 30"))?;
                let scale = match unit.trim() {
                    "ms" => 0.001,
                    "" | "s" => 1.0,
                    "m" | "min" => 60.0,
                    "h" => 3600.0,
                    other => {
                        return Err(format!(
                            "`{other}` is not a duration unit; use ms, s, m or h"
                        ))
                    }
                };
                Duration::try_from_secs_f64(n * scale)
                    .map_err(|_| format!("`{text}` is not a duration"))
            }
        }
    }

    pub fn required<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        parse(Raw::deserialize(d)?).map_err(D::Error::custom)
    }

    pub fn optional<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        let Some(raw) = Option::<Raw>::deserialize(d)? else {
            return Ok(None);
        };
        let value = parse(raw).map_err(D::Error::custom)?;
        Ok((!value.is_zero()).then_some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlalchemy_driver_suffix_is_removed() {
        assert_eq!(
            normalize_url("postgresql+psycopg2://u:p@h:5432/d").unwrap(),
            "postgresql://u:p@h:5432/d"
        );
        assert_eq!(
            normalize_url("POSTGRES+asyncpg://h/d").unwrap(),
            "postgres://h/d"
        );
        assert_eq!(normalize_url("postgres://h/d").unwrap(), "postgres://h/d");
    }

    #[test]
    fn other_schemes_fail_without_echoing_the_url() {
        let err = normalize_url("mysql://root:hunter2@h/d").unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("`mysql` is not a Postgres URL scheme"),
            "{text}"
        );
        assert!(text.contains("help:"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");

        let err = normalize_url("hunter2").unwrap_err();
        assert!(!err.to_string().contains("hunter2"));
    }

    #[test]
    fn a_bad_url_does_not_leak_the_password() {
        let cfg = DbConfig::new("postgres://u:hunter2@h:notaport/d");
        let text = cfg.connect_options().unwrap_err().to_string();
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("help:"), "{text}");
    }

    #[test]
    fn debug_redacts_the_password() {
        let cfg =
            DbConfig::new("postgresql+psycopg2://superset:hunter2@db:5432/app?sslmode=require");
        let text = format!("{cfg:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(
            text.contains("superset:***@db:5432/app?sslmode=require"),
            "{text}"
        );
        assert_eq!(redact("postgres://h/d"), "postgres://h/d");
        assert_eq!(redact("postgres://u@h/d"), "postgres://u@h/d");
        assert_eq!(redact("postgres://u:p:q@h"), "postgres://u:***@h");
    }

    #[test]
    fn session_settings_go_into_the_startup_options() {
        let mut cfg = DbConfig::new("postgres://u:p@localhost/app");
        cfg.search_path = vec!["tenant_1".into(), "$user".into(), "Odd \"name\"".into()];
        cfg.statement_timeout = Some(Duration::from_millis(1500));
        cfg.read_only = true;
        cfg.application_name = Some("web".into());
        let options = cfg.connect_options().unwrap();
        // sqlx escapes spaces with a backslash, as libpq does.
        assert_eq!(
            options.get_options(),
            Some(
                r#"-c search_path="tenant_1","$user","Odd\ ""name""" -c statement_timeout=1500 -c default_transaction_read_only=on"#
            )
        );
        assert_eq!(options.get_application_name(), Some("web"));
    }

    #[test]
    fn no_session_settings_means_no_options() {
        let options = DbConfig::new("postgres://localhost/app")
            .connect_options()
            .unwrap();
        assert_eq!(options.get_options(), None);
    }

    #[test]
    fn a_tiny_statement_timeout_is_one_millisecond() {
        let mut cfg = DbConfig::new("postgres://localhost/app");
        cfg.statement_timeout = Some(Duration::from_micros(10));
        let options = cfg.connect_options().unwrap();
        assert_eq!(options.get_options(), Some("-c statement_timeout=1"));
    }

    #[test]
    fn ca_file_means_verify_full_and_otherwise_the_url_decides() {
        let mut cfg = DbConfig::new("postgres://h/d?sslmode=disable");
        assert!(matches!(
            cfg.connect_options().unwrap().get_ssl_mode(),
            PgSslMode::Disable
        ));
        cfg.ca_file = Some("/etc/ssl/rds.pem".into());
        assert!(matches!(
            cfg.connect_options().unwrap().get_ssl_mode(),
            PgSslMode::VerifyFull
        ));
    }

    #[test]
    fn empty_schema_is_rejected() {
        let mut cfg = DbConfig::new("postgres://h/d");
        cfg.search_path = vec![String::new()];
        assert!(cfg.connect_options().is_err());
    }

    #[test]
    fn pool_sizes_are_checked() {
        let mut cfg = DbConfig::new("postgres://h/d");
        cfg.max_connections = 0;
        assert!(cfg.pool_options().is_err());
        cfg.max_connections = 2;
        cfg.min_connections = 3;
        assert!(cfg
            .pool_options()
            .unwrap_err()
            .to_string()
            .contains("help:"));
        cfg.min_connections = 1;
        assert!(cfg.pool_options().is_ok());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn deserializes_with_seconds_or_units() {
        let cfg: DbConfig = serde_json::from_value(serde_json::json!({
            "url": "postgres://h/d",
            "acquire_timeout": "5s",
            "idle_timeout": 0,
            "statement_timeout": "250ms",
            "max_lifetime": 90,
            "search_path": ["app"],
        }))
        .unwrap();
        assert_eq!(cfg.acquire_timeout, Duration::from_secs(5));
        assert_eq!(cfg.idle_timeout, None);
        assert_eq!(cfg.statement_timeout, Some(Duration::from_millis(250)));
        assert_eq!(cfg.max_lifetime, Some(Duration::from_secs(90)));
        assert_eq!(cfg.max_connections, 10);
        assert_eq!(cfg.search_path, ["app"]);

        let err = serde_json::from_value::<DbConfig>(serde_json::json!({
            "url": "postgres://h/d",
            "statement_timeout": "5 fortnights",
        }))
        .unwrap_err();
        assert!(err.to_string().contains("not a duration unit"), "{err}");
    }
}
