//! A Postgres server started from the binary, for development.
//!
//! The `db-embedded` module writes a plugin over this: it starts the server
//! before the `db` plugin opens its pool, and hands `db` the URL through
//! [`EmbeddedUrl`]. The pool stays `db`'s. This module opens no pool.
//!
//! Built on `postgresql_embedded` with its `bundled` feature: the server
//! archive is in the binary, so the first start extracts it and needs no
//! network and no Postgres on the machine.
//!
//! **Never in production.** One server per checkout, on one disk, with a
//! generated password in a file beside its data. `brocanth check --deploy`
//! refuses the module, and the generated plugin refuses to start outside
//! `development` and `test`.
//!
//! The directories, under [`EmbeddedConfig::dir`]:
//!
//! - `server/<version>/`: the extracted binaries, once (270 MB on macOS).
//! - `data/`: the cluster. It survives a restart. Delete the directory to
//!   start from an empty database.
//! - `password`: the superuser password, made at the first start.
//!
//! Every call blocks. Each one runs on a thread of its own with its own
//! runtime, so a plugin hook calls it on any runtime, current-thread
//! included, and a command calls it with none.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use postgresql_embedded::{PostgreSQL, Settings, Status};
use thiserror::Error;

/// The URL of the running embedded server, for the `db` plugin.
///
/// The `db-embedded` plugin inserts it into the application's services in
/// `configure`. The `db` plugin, registered after it, reads it in its own
/// `configure` and opens the pool on it in place of `[db] url`.
#[derive(Clone)]
pub struct EmbeddedUrl(pub String);

impl std::fmt::Debug for EmbeddedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("EmbeddedUrl").field(&"<redacted>").finish()
    }
}

/// Where the server lives and what it serves.
#[derive(Debug, Clone)]
pub struct EmbeddedConfig {
    /// The directory for the binaries, the data and the password. Relative
    /// to the working directory: `cargo run` starts in the project root.
    pub dir: PathBuf,
    /// The port on localhost. 0 takes a free one at each start.
    pub port: u16,
    /// The database the URL names, created at the first start.
    pub database: String,
    /// The longest `initdb` or `pg_ctl` may take, each.
    pub timeout: Duration,
}

impl EmbeddedConfig {
    /// A server under `dir` that serves `database`, on a free port.
    pub fn new(dir: impl Into<PathBuf>, database: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            port: 0,
            database: database.into(),
            timeout: Duration::from_secs(60),
        }
    }
}

/// What went wrong while the server started or stopped.
///
/// No variant holds the URL: it carries the password.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EmbeddedError {
    /// The configuration cannot work. Nothing started.
    #[error("embedded Postgres: {message}\nhelp: {help}")]
    Config {
        /// What is wrong.
        message: String,
        /// What to change.
        help: String,
    },
    /// The files under the directory could not be read or written.
    #[error("embedded Postgres: {path}: {source}\nhelp: {help}")]
    Io {
        /// The file or directory.
        path: String,
        /// The operating system's error.
        #[source]
        source: std::io::Error,
        /// What to check.
        help: &'static str,
    },
    /// `initdb`, `pg_ctl` or the first connection failed.
    #[error("embedded Postgres could not {action}: {message}\nhelp: {help}")]
    Server {
        /// `start`, `stop` or `create the database`.
        action: &'static str,
        /// What the server or the command said.
        message: String,
        /// What to do next.
        help: String,
    },
}

/// A running development server. Stop it with [`EmbeddedPostgres::stop`];
/// a drop stops it too, without waiting for an answer.
pub struct EmbeddedPostgres {
    /// `None` once stopped.
    server: Option<PostgreSQL>,
    url: String,
    redacted: String,
    version: String,
    data_dir: PathBuf,
}

impl std::fmt::Debug for EmbeddedPostgres {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedPostgres")
            .field("url", &self.redacted)
            .field("version", &self.version)
            .finish()
    }
}

impl EmbeddedPostgres {
    /// Extracts the binaries on the first call, initialises the cluster if
    /// `data/` is empty, starts the server and creates the database.
    ///
    /// A server that a killed process left running on the same `data/` is
    /// stopped first, so a restart after `kill -9` works.
    ///
    /// # Errors
    ///
    /// [`EmbeddedError`], with a `help:` line, when a directory cannot be
    /// written, the port is taken, or the server does not answer.
    pub fn start(config: &EmbeddedConfig) -> Result<Self, EmbeddedError> {
        if config.database.is_empty() {
            return Err(EmbeddedError::Config {
                message: "the database name is empty".to_string(),
                help: "set `database` under [db_embedded] to the name of the database to create"
                    .to_string(),
            });
        }
        let dir = absolute(&config.dir)?;
        std::fs::create_dir_all(&dir).map_err(|source| EmbeddedError::Io {
            path: dir.display().to_string(),
            source,
            help: "set `dir` under [db_embedded] to a directory this process can write",
        })?;
        let settings = Settings {
            installation_dir: dir.join("server"),
            password_file: dir.join("password"),
            data_dir: dir.join("data"),
            host: "localhost".to_string(),
            port: config.port,
            password: password(&dir.join("password"))?,
            // The cluster survives the process: a drop stops the server and
            // keeps the files.
            temporary: false,
            timeout: Some(config.timeout),
            ..settings_without_temp_dirs()
        };
        let data_dir = settings.data_dir.clone();
        let database = config.database.clone();
        on_own_runtime(move || async move {
            let mut server = PostgreSQL::new(settings);
            server.setup().await.map_err(|e| EmbeddedError::Server {
                action: "set up",
                message: e.to_string(),
                help: format!(
                    "delete {} to extract and initialise it again",
                    data_dir.parent().unwrap_or(&data_dir).display()
                ),
            })?;
            if server.status() == Status::Started {
                // A `postmaster.pid` from a process that did not stop it:
                // `pg_ctl stop` ends an orphan, and a stale file is removed
                // by the next start.
                let _ = server.stop().await;
            }
            server.start().await.map_err(|e| EmbeddedError::Server {
                action: "start",
                message: e.to_string(),
                help: format!(
                    "read {}; a taken port is fixed by `port = 0` under [db_embedded]",
                    data_dir.join("start.log").display()
                ),
            })?;
            let exists = server
                .database_exists(&database)
                .await
                .map_err(|e| create_error(&e))?;
            if !exists {
                server
                    .create_database(&database)
                    .await
                    .map_err(|e| create_error(&e))?;
            }
            let settings = server.settings();
            let url = settings.url(&database);
            let redacted = format!(
                "postgresql://{}:<redacted>@{}:{}/{database}",
                settings.username, settings.host, settings.port
            );
            let version = settings.version.to_string();
            Ok(Self {
                server: Some(server),
                url,
                redacted,
                version: version.trim_start_matches('=').to_string(),
                data_dir,
            })
        })
    }

    /// The URL the pool opens: user, password, port and database.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// [`Self::url`] with the password replaced, for a log line.
    pub fn redacted_url(&self) -> &str {
        &self.redacted
    }

    /// The server's version: `18.1.0`.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// The cluster's directory.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Stops the server with `pg_ctl stop -m fast` and waits for it: open
    /// connections are closed, and each transaction in flight rolls back.
    ///
    /// A server that already stopped is not an error: Ctrl-C in a terminal
    /// reaches the server too, and it stops on its own.
    ///
    /// # Errors
    ///
    /// [`EmbeddedError::Server`] when the server still runs after the call.
    pub fn stop(mut self) -> Result<(), EmbeddedError> {
        let Some(server) = self.server.take() else {
            return Ok(());
        };
        let log = self.data_dir.join("start.log");
        on_own_runtime(move || async move {
            if server.status() != Status::Started {
                return Ok(());
            }
            match server.stop().await {
                Ok(()) => Ok(()),
                Err(_) if server.status() != Status::Started => Ok(()),
                Err(e) => Err(EmbeddedError::Server {
                    action: "stop",
                    message: e.to_string(),
                    help: format!(
                        "read {}; `pg_ctl stop -D <dir>/data` stops it by hand",
                        log.display()
                    ),
                }),
            }
        })
    }
}

impl Drop for EmbeddedPostgres {
    fn drop(&mut self) {
        // `PostgreSQL`'s own drop runs `pg_ctl stop -m fast` when the server
        // runs, and keeps the files because `temporary` is false.
        drop(self.server.take());
    }
}

fn create_error(e: &postgresql_embedded::Error) -> EmbeddedError {
    EmbeddedError::Server {
        action: "create the database",
        message: e.to_string(),
        help: "the server started and refused the superuser: delete the `password` file and \
               `data/` under `dir` together, and start again"
            .to_string(),
    }
}

/// `Settings::new` makes a temporary data directory and password file and
/// keeps them. This takes its other defaults and removes those two, so a
/// start leaves nothing in the temporary directory.
fn settings_without_temp_dirs() -> Settings {
    let defaults = Settings::new();
    let _ = std::fs::remove_dir(&defaults.data_dir);
    if let Some(dir) = defaults.password_file.parent() {
        let _ = std::fs::remove_dir(dir);
    }
    defaults
}

fn absolute(dir: &Path) -> Result<PathBuf, EmbeddedError> {
    std::path::absolute(dir).map_err(|source| EmbeddedError::Io {
        path: dir.display().to_string(),
        source,
        help: "start the process from a directory that exists",
    })
}

/// The superuser password: the file's, or a new one written to it.
///
/// `initdb` reads the file once, when it creates the cluster, so the two
/// stay together. A new one comes from the standard library's random hash
/// keys and the time: this guards a development server on localhost, not a
/// secret.
fn password(file: &Path) -> Result<String, EmbeddedError> {
    let io = |source| EmbeddedError::Io {
        path: file.display().to_string(),
        source,
        help: "delete the `password` file and `data/` beside it together, and start again",
    };
    match std::fs::read_to_string(file) {
        Ok(found) if !found.trim().is_empty() => return Ok(found.trim().to_string()),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io(e)),
    }
    let mut made = String::new();
    for _ in 0..2 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        made.push_str(&format!("{:016x}", hasher.finish()));
    }
    std::fs::write(file, &made).map_err(io)?;
    Ok(made)
}

/// Runs `work` to the end on a new thread with a current-thread runtime.
///
/// A plugin hook is synchronous and may run inside the application's
/// runtime, where `block_on` panics. A thread of its own has no runtime.
fn on_own_runtime<F, Fut, T>(work: F) -> Result<T, EmbeddedError>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, EmbeddedError>>,
    T: Send + 'static,
{
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|source| EmbeddedError::Io {
                path: "a Tokio runtime".to_string(),
                source,
                help: "the process could not make a thread's runtime: check its limits",
            })?;
        runtime.block_on(work())
    })
    .join()
    .unwrap_or_else(|_| {
        Err(EmbeddedError::Server {
            action: "start or stop",
            message: "the thread that drives it panicked".to_string(),
            help: "read the panic message above; it names the cause".to_string(),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_is_made_once_and_read_back() {
        let dir = std::env::temp_dir().join(format!("brocanth-embedded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        let file = dir.join("password");
        let made = password(&file).expect("made");
        assert_eq!(made.len(), 32);
        assert_eq!(password(&file).expect("read"), made);
        std::fs::remove_dir_all(&dir).expect("removed");
    }

    #[test]
    fn neither_debug_form_prints_the_url() {
        let url = EmbeddedUrl("postgresql://postgres:secret@localhost:1/x".to_string());
        assert!(!format!("{url:?}").contains("secret"));
    }

    #[test]
    fn an_empty_database_name_is_refused_with_help() {
        let refused = EmbeddedPostgres::start(&EmbeddedConfig::new("unused", ""))
            .expect_err("no database name");
        let text = refused.to_string();
        assert!(text.contains("help:"), "{text}");
    }
}
