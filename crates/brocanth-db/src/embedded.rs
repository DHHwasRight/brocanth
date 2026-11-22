//! A Postgres server started from the binary: for development with the
//! `db-embedded` module, and for production too with `db-embedded-prod`.
//!
//! The plugin of either module starts the server before the `db` plugin
//! opens its pool, and hands `db` the URL through [`EmbeddedUrl`]. The pool
//! stays `db`'s. This module opens no pool.
//!
//! Built on `postgresql_embedded`, with one pinned Postgres release,
//! [`POSTGRES_VERSION`], and one pinned SHA-256 per target,
//! [`ARCHIVE_SHA256`]. Every archive is checked against the pin before it
//! is extracted, whatever its source. A build needs no network. The first
//! start installs the binaries from the first of:
//!
//! 1. the archive bundled into the binary at build time
//!    (`BROCANTH_POSTGRES_BUNDLE=<file> cargo build`, see [`bundled`]);
//! 2. [`EmbeddedConfig::archive`], then the file that [`ARCHIVE_ENV`]
//!    (`BROCANTH_POSTGRES_ARCHIVE`) names;
//! 3. in [`Mode::Development`] only: the cache,
//!    `~/.theseus/postgresql/postgresql-<version>-<target>.tar.gz`, then one
//!    download from the GitHub releases of `theseus-rs/postgresql-binaries`,
//!    written to the cache. `GITHUB_TOKEN` lifts the API rate limit.
//!
//! A later start finds the binaries under `server/` and reads no archive.
//!
//! The directories, under [`EmbeddedConfig::dir`]:
//!
//! - `server/<version>/`: the extracted binaries, once (270 MB on macOS).
//! - `data/`: the cluster. It survives a restart.
//! - `password`: the superuser password, made at the first start, mode
//!   `0600`. `app_password`: the application role's, when there is one.
//! - `setup.lock`, `users.lock`: see [`EmbeddedPostgres::start`].
//!
//! [`Mode::Development`]: one server per directory, shared. Every
//! [`EmbeddedPostgres`] on one directory shares it: the handles of one
//! process through a registry keyed by the canonical directory, the
//! processes through the two lock files. The last handle released stops it.
//!
//! [`Mode::Production`]: one process owns the server, the directory is an
//! absolute path, the start never downloads, and the server log goes to
//! stderr. `docs/embedded-postgres-production.md` has the reasons.
//!
//! Every call blocks. Each one runs on a thread of its own with its own
//! runtime, so a plugin hook calls it on any runtime, current-thread
//! included, and a command calls it with none.

use std::collections::HashMap;
use std::fs::{File, TryLockError};
use std::io::{Read, Seek, SeekFrom};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use postgresql_archive::configuration::theseus;
use postgresql_embedded::{PostgreSQL, Settings, Status, VersionReq};
use sha2::{Digest, Sha256};
use sqlx::Connection;
use thiserror::Error;

include!("embedded_pins.rs");

/// The variable that names an archive file to install from, in place of the
/// cache and the download: `postgresql-<POSTGRES_VERSION>-<target>.tar.gz`
/// from the releases of `theseus-rs/postgresql-binaries`.
pub const ARCHIVE_ENV: &str = "BROCANTH_POSTGRES_ARCHIVE";

/// The archive `build.rs` embedded, after it checked the pin.
#[cfg(brocanth_bundled)]
const BUNDLED: Option<&[u8]> = Some(include_bytes!(env!("BROCANTH_POSTGRES_BUNDLED_FILE")));
#[cfg(not(brocanth_bundled))]
const BUNDLED: Option<&[u8]> = None;

/// Whether this binary carries the archive: it was built with
/// `BROCANTH_POSTGRES_BUNDLE` set to the archive file. A first start then
/// reads no file and no network.
pub fn bundled() -> bool {
    BUNDLED.is_some()
}

/// The binaries a start, a backup and a restore run. A copy of
/// `server/<version>/` without one of them is not installed.
const REQUIRED_BINARIES: &[&str] = &["postgres", "pg_ctl", "initdb", "pg_dump", "pg_restore"];

/// The URL of the running embedded server, for the `db` plugin.
///
/// The embedded plugin inserts it into the application's services in
/// `configure`. The `db` plugin, registered after it, reads it in its own
/// `configure` and opens the pool on it in place of `[db] url`.
#[derive(Clone)]
pub struct EmbeddedUrl(pub String);

impl std::fmt::Debug for EmbeddedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("EmbeddedUrl").field(&"<redacted>").finish()
    }
}

/// How the server treats its directory and the other processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Every process on the directory shares one server; the last one
    /// stops it. The directory may be relative. A first start may download
    /// the archive.
    #[default]
    Development,
    /// One process owns the server: a second one is refused. The directory
    /// is an absolute path outside the temporary directory. A start never
    /// downloads and never reads the per-user cache. The server log goes to
    /// stderr.
    Production,
}

/// Where the server lives and what it serves.
#[derive(Debug, Clone)]
pub struct EmbeddedConfig {
    /// The directory for the binaries, the data and the passwords. Relative
    /// to the working directory: `cargo run` starts in the project root.
    pub dir: PathBuf,
    /// The port on localhost. 0 takes a free one at each start.
    pub port: u16,
    /// The database the URL names, created at the first start.
    pub database: String,
    /// The longest `initdb` or `pg_ctl` may take, each.
    pub timeout: Duration,
    /// Shared or owned. See [`Mode`].
    pub mode: Mode,
    /// An archive file to install from, checked against the pin. Before
    /// [`ARCHIVE_ENV`]; after the bundled archive.
    pub archive: Option<PathBuf>,
    /// The role the URL connects as, when it is not the superuser. The start
    /// creates it (`LOGIN CREATEROLE`, no `SUPERUSER`, `CREATEDB` or
    /// `BYPASSRLS`), gives it a password in `app_password` and makes it the
    /// owner of the database.
    pub app_role: Option<String>,
}

impl EmbeddedConfig {
    /// A development server under `dir` that serves `database`, on a free
    /// port, connected as the superuser.
    pub fn new(dir: impl Into<PathBuf>, database: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            port: 0,
            database: database.into(),
            timeout: Duration::from_secs(60),
            mode: Mode::Development,
            archive: None,
            app_role: None,
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
    /// The archive could not be read, downloaded, checked or extracted.
    #[error("embedded Postgres could not install {version}: {message}\nhelp: {help}")]
    Install {
        /// [`POSTGRES_VERSION`].
        version: &'static str,
        /// What failed.
        message: String,
        /// What to do next.
        help: String,
    },
    /// `initdb`, `pg_ctl`, a client binary or a setup statement failed.
    #[error("embedded Postgres could not {action}: {message}\nhelp: {help}")]
    Server {
        /// `start`, `stop`, `create the database`, `back up`, `restore`.
        action: &'static str,
        /// What the server or the command said.
        message: String,
        /// What to do next.
        help: String,
    },
}

/// A handle on the server of one directory. Release it with
/// [`EmbeddedPostgres::stop`]; a drop releases it too.
///
/// In [`Mode::Development`] every handle on one directory shares one
/// server, in this process and across processes, and the last handle
/// released stops it. In [`Mode::Production`] there is one handle.
pub struct EmbeddedPostgres {
    /// `None` once released.
    shared: Option<Arc<Mutex<Shared>>>,
    dir: PathBuf,
    url: String,
    redacted: String,
    data_dir: PathBuf,
    port: u16,
}

impl std::fmt::Debug for EmbeddedPostgres {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddedPostgres")
            .field("url", &self.redacted)
            .field("version", &POSTGRES_VERSION)
            .finish()
    }
}

/// The server of one directory, as this process sees it.
#[derive(Default)]
struct Shared {
    /// The handles of this process that hold the server.
    users: usize,
    /// Set while `users > 0`.
    running: Option<Running>,
}

struct Running {
    /// The settings the server answers on: the port is the real one.
    settings: Settings,
    /// A lock on `users.lock`, held while this process uses the server:
    /// shared in development, exclusive in production. The operating
    /// system drops it when the process dies.
    _in_use: File,
    /// Production: the thread that copies the server log to stderr.
    follower: Option<Follower>,
}

/// One entry per canonical directory, for the life of the process.
fn registry(dir: &Path) -> Arc<Mutex<Shared>> {
    static REGISTRY: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<Shared>>>>> =
        LazyLock::new(Mutex::default);
    let mut map = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    map.entry(dir.to_path_buf()).or_default().clone()
}

impl EmbeddedPostgres {
    /// Starts the server of `config.dir`, or joins it in development.
    ///
    /// The first start installs the binaries and initialises `data/`. Every
    /// call creates `config.database` if it is missing, and the application
    /// role when [`EmbeddedConfig::app_role`] names one.
    ///
    /// Two lock files in the directory order the processes:
    ///
    /// - `setup.lock`, exclusive, is held while a process installs,
    ///   initialises, starts, joins or stops. A second process waits for the
    ///   first to finish, and `initdb` runs once.
    /// - `users.lock` is held by each process that uses the server: shared
    ///   in development, so a second process joins; exclusive in
    ///   production, so a second process is refused. A process that dies,
    ///   `kill -9` included, drops its lock. A server that runs while nobody
    ///   holds `users.lock` is an orphan: the next start stops it and starts
    ///   it again.
    ///
    /// # Errors
    ///
    /// [`EmbeddedError`], with a `help:` line, when the configuration cannot
    /// work (a production directory that is relative, a cluster of another
    /// major version, a server another process owns), a directory cannot be
    /// written, the archive cannot be installed or does not match its pin,
    /// the port is taken, or the server does not answer.
    pub fn start(config: &EmbeddedConfig) -> Result<Self, EmbeddedError> {
        if config.database.is_empty() {
            return Err(EmbeddedError::Config {
                message: "the database name is empty".to_string(),
                help: "set `database` under [db_embedded] to the name of the database to create"
                    .to_string(),
            });
        }
        if let Some(role) = &config.app_role {
            check_role_name(role)?;
        }
        if config.mode == Mode::Production {
            check_production_dir(&config.dir)?;
        }
        let dir = absolute(&config.dir)?;
        std::fs::create_dir_all(&dir).map_err(|source| EmbeddedError::Io {
            path: dir.display().to_string(),
            source,
            help: "set `dir` under [db_embedded] to a directory this process can write",
        })?;
        let dir = std::fs::canonicalize(&dir).map_err(|source| EmbeddedError::Io {
            path: dir.display().to_string(),
            source,
            help: "set `dir` under [db_embedded] to a directory this process can read",
        })?;
        check_major(&dir.join("data"))?;
        let shared = registry(&dir);
        let (config, held) = (config.clone(), shared.clone());
        let task_dir = dir.clone();
        let (url, port) = on_own_runtime(move |runtime| {
            let dir = task_dir;
            // Held to the end: the handles of this process take turns.
            let mut guard = held.lock().unwrap_or_else(PoisonError::into_inner);
            let state: &mut Shared = &mut guard;
            if config.mode == Mode::Production && state.running.is_some() {
                return Err(owned(&dir, "this process"));
            }
            let _setup = lock_file(&dir.join("setup.lock"))?;
            runtime.block_on(async {
                if state.running.is_none() {
                    state.running = Some(join_or_start(&dir, &config).await?);
                }
                state.users += 1;
                let settings = state.running.as_ref().map(|r| r.settings.clone());
                let settings = settings.expect("set above");
                match connect_url(&dir, &settings, &config).await {
                    Ok(url) => Ok((url, settings.port)),
                    Err(e) => {
                        let _ = release(state, &dir).await;
                        Err(e)
                    }
                }
            })
        })?;
        let redacted = redact(&url);
        Ok(Self {
            shared: Some(shared),
            data_dir: dir.join("data"),
            dir,
            url,
            redacted,
            port,
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

    /// The server's version: [`POSTGRES_VERSION`].
    pub fn version(&self) -> &str {
        POSTGRES_VERSION
    }

    /// The cluster's directory.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The port the server answers on, on localhost.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Releases this handle. The last handle on the directory, in every
    /// process, stops the server with `pg_ctl stop -m fast` and waits for it:
    /// open connections are closed, and each transaction in flight rolls
    /// back. Close the pool first.
    ///
    /// A server that already stopped is not an error: Ctrl-C in a terminal
    /// reaches the server too, and it stops on its own.
    ///
    /// # Errors
    ///
    /// [`EmbeddedError::Server`] when the server still runs after the call.
    pub fn stop(mut self) -> Result<(), EmbeddedError> {
        self.release()
    }

    fn release(&mut self) -> Result<(), EmbeddedError> {
        let Some(shared) = self.shared.take() else {
            return Ok(());
        };
        let dir = self.dir.clone();
        on_own_runtime(move |runtime| {
            let mut guard = shared.lock().unwrap_or_else(PoisonError::into_inner);
            let _setup = lock_file(&dir.join("setup.lock"))?;
            runtime.block_on(release(&mut guard, &dir))
        })
    }
}

impl Drop for EmbeddedPostgres {
    fn drop(&mut self) {
        // A panic or an early error: release the server, and stop it when
        // this was the last handle. Nobody reads an error here.
        let _ = self.release();
    }
}

/// Installs and initialises when needed, then joins the server another
/// process runs (development), or starts it. The caller holds `setup.lock`.
async fn join_or_start(dir: &Path, config: &EmbeddedConfig) -> Result<Running, EmbeddedError> {
    let mut settings = Settings {
        installation_dir: dir.join("server"),
        password_file: dir.join("password"),
        data_dir: dir.join("data"),
        host: "localhost".to_string(),
        port: config.port,
        password: secret(&dir.join("password"))?,
        version: pinned_version(),
        // The cluster survives the process: a stop keeps the files.
        temporary: false,
        timeout: Some(config.timeout),
        ..settings_without_temp_dirs()
    };
    // On the command line, so no edit of postgresql.conf opens the server
    // to the network.
    settings
        .configuration
        .insert("listen_addresses".to_string(), "localhost".to_string());
    install(&settings.installation_dir, config).await?;
    initialise(&settings).await?;
    let users_file = dir.join("users.lock");
    let users = open_lock(&users_file)?;
    let lock_io = |source| EmbeddedError::Io {
        path: users_file.display().to_string(),
        source,
        help: "set `dir` under [db_embedded] to a directory on a local disk",
    };
    let mut server = ManuallyDrop::new(PostgreSQL::new(settings));
    let log = dir.join("data/start.log");
    match users.try_lock() {
        Ok(()) => {
            // No other process uses the server. A `postmaster.pid` is an
            // orphan's, from a process killed with its server running, or a
            // stale file: `pg_ctl stop` ends the one, the start removes the
            // other. Postgres replays its WAL after a crash.
            if server.status() == Status::Started {
                let _ = server.stop().await;
            }
            let log_start = std::fs::metadata(&log).map_or(0, |m| m.len());
            server.start().await.map_err(|e| EmbeddedError::Server {
                action: "start",
                message: e.to_string(),
                help: format!(
                    "read {}; a taken port is fixed by `port = 0` under [db_embedded]; \
                     Postgres refuses to run as root",
                    log.display()
                ),
            })?;
            let settings = server.settings().clone();
            if config.mode == Mode::Production {
                // Kept exclusive: a second process is refused.
                return Ok(Running {
                    settings,
                    _in_use: users,
                    follower: Some(Follower::start(log, log_start)),
                });
            }
            // Still under `setup.lock`: no process sees the switch.
            users.unlock().map_err(lock_io)?;
            users.lock_shared().map_err(lock_io)?;
            Ok(Running {
                settings,
                _in_use: users,
                follower: None,
            })
        }
        Err(TryLockError::WouldBlock) if config.mode == Mode::Production => {
            Err(owned(dir, "another process"))
        }
        Err(TryLockError::WouldBlock) => {
            // Another process holds the server and started it under
            // `setup.lock`: take its port. A production owner holds the
            // lock exclusive, and is not joined.
            match users.try_lock_shared() {
                Ok(()) => {}
                Err(TryLockError::WouldBlock) => return Err(owned(dir, "another process")),
                Err(TryLockError::Error(source)) => return Err(lock_io(source)),
            }
            let mut settings = server.settings().clone();
            settings.port = running_port(&settings.data_dir)?;
            Ok(Running {
                settings,
                _in_use: users,
                follower: None,
            })
        }
        Err(TryLockError::Error(source)) => Err(lock_io(source)),
    }
}

/// The refusal of a second owner of a production server.
fn owned(dir: &Path, who: &str) -> EmbeddedError {
    EmbeddedError::Config {
        message: format!(
            "{who} runs the server of {} in production mode, and one process owns it",
            dir.display()
        ),
        help: "stop the other process first, or give this one its own `dir`; `db_backup` \
               reads a running server without owning it"
            .to_string(),
    }
}

/// Drops one user. The last user of this process drops the process's
/// `users.lock`, and stops the server when no other process holds one. The
/// caller holds `setup.lock`.
async fn release(state: &mut Shared, dir: &Path) -> Result<(), EmbeddedError> {
    state.users = state.users.saturating_sub(1);
    if state.users > 0 {
        return Ok(());
    }
    let Some(Running {
        settings,
        _in_use: in_use,
        follower,
    }) = state.running.take()
    else {
        return Ok(());
    };
    drop(in_use);
    let stopped = stop_if_last(settings, dir).await;
    // After the stop, so the shutdown lines reach stderr too.
    if let Some(follower) = follower {
        follower.finish();
    }
    stopped
}

async fn stop_if_last(settings: Settings, dir: &Path) -> Result<(), EmbeddedError> {
    let users = open_lock(&dir.join("users.lock"))?;
    if users.try_lock().is_err() {
        // Another process uses the server: it stops it.
        return Ok(());
    }
    let server = ManuallyDrop::new(PostgreSQL::new(settings));
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
                "read {}; `pg_ctl stop -D {}` stops it by hand",
                dir.join("data/start.log").display(),
                dir.join("data").display()
            ),
        }),
    }
}

/// Runs `initdb` once, into `data.partial/`, and renames it to `data/`: a
/// process killed during `initdb` leaves no `data/` that looks initialised.
/// The caller holds `setup.lock`.
async fn initialise(settings: &Settings) -> Result<(), EmbeddedError> {
    let data = &settings.data_dir;
    if data.join("postgresql.conf").exists() {
        return Ok(());
    }
    let io = |path: &Path, source| EmbeddedError::Io {
        path: path.display().to_string(),
        source,
        help: "set `dir` under [db_embedded] to a directory this process can write",
    };
    if data.exists() {
        // Empty: remove it so the rename succeeds. Not empty: refuse, the
        // files are someone's.
        std::fs::remove_dir(data).map_err(|_| EmbeddedError::Server {
            action: "set up",
            message: format!("{} has files but no cluster", data.display()),
            help: format!(
                "an initdb stopped halfway: delete {} and start again",
                data.display()
            ),
        })?;
    }
    let partial = data.with_extension("partial");
    if partial.exists() {
        std::fs::remove_dir_all(&partial).map_err(|e| io(&partial, e))?;
    }
    let mut init = settings.clone();
    init.data_dir.clone_from(&partial);
    let mut server = ManuallyDrop::new(PostgreSQL::new(init));
    server.setup().await.map_err(|e| EmbeddedError::Server {
        action: "set up",
        message: e.to_string(),
        help: format!(
            "delete {} to install and initialise it again; initdb refuses to run as root",
            data.parent().unwrap_or(data).display()
        ),
    })?;
    let hba = partial.join("pg_hba.conf");
    let text = std::fs::read_to_string(&hba).map_err(|e| io(&hba, e))?;
    std::fs::write(&hba, scram_only(&text)).map_err(|e| io(&hba, e))?;
    std::fs::rename(&partial, data).map_err(|e| io(data, e))
}

/// `pg_hba.conf` with each `password` method replaced by `scram-sha-256`:
/// `initdb --auth=password` accepts a password sent in clear text.
fn scram_only(hba: &str) -> String {
    let mut out = String::with_capacity(hba.len() + 64);
    for line in hba.lines() {
        let rule = !line.trim_start().starts_with('#');
        match line.trim_end().strip_suffix("password") {
            Some(head) if rule && head.ends_with(char::is_whitespace) => {
                out.push_str(head);
                out.push_str("scram-sha-256");
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Creates the database when it is missing, and the application role when
/// the config names one; returns the URL the pool opens. The caller holds
/// `setup.lock`, so two processes do not both create them.
async fn connect_url(
    dir: &Path,
    settings: &Settings,
    config: &EmbeddedConfig,
) -> Result<String, EmbeddedError> {
    let database = &config.database;
    let server = ManuallyDrop::new(PostgreSQL::new(settings.clone()));
    let exists = server
        .database_exists(database)
        .await
        .map_err(|e| create_error(&e.to_string()))?;
    if !exists {
        server
            .create_database(database)
            .await
            .map_err(|e| create_error(&e.to_string()))?;
    }
    let Some(role) = &config.app_role else {
        return Ok(settings.url(database));
    };
    let password = secret(&dir.join("app_password"))?;
    let mut conn = superuser(settings).await?;
    let quoted = quote_ident(role);
    // The password is hex: it needs no escape.
    for sql in [
        format!(
            "DO $$ BEGIN IF NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '{role}') \
             THEN CREATE ROLE {quoted} LOGIN; END IF; END $$"
        ),
        format!(
            "ALTER ROLE {quoted} WITH LOGIN NOSUPERUSER NOCREATEDB CREATEROLE NOBYPASSRLS \
             PASSWORD '{password}'"
        ),
        format!("ALTER DATABASE {} OWNER TO {quoted}", quote_ident(database)),
    ] {
        execute(&mut conn, &sql, "create the application role").await?;
    }
    let _ = conn.close().await;
    Ok(format!(
        "postgresql://{role}:{password}@{}:{}/{database}",
        settings.host, settings.port
    ))
}

/// A connection to the `postgres` database as the superuser.
async fn superuser(settings: &Settings) -> Result<sqlx::PgConnection, EmbeddedError> {
    let mut admin = settings.clone();
    admin.username = postgresql_embedded::BOOTSTRAP_SUPERUSER.to_string();
    sqlx::PgConnection::connect(&admin.url(postgresql_embedded::BOOTSTRAP_DATABASE))
        .await
        .map_err(|e| create_error(&e.to_string()))
}

async fn execute(
    conn: &mut sqlx::PgConnection,
    sql: &str,
    action: &'static str,
) -> Result<(), EmbeddedError> {
    sqlx::raw_sql(sqlx::AssertSqlSafe(sql))
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(|e| EmbeddedError::Server {
            action,
            message: e.to_string(),
            help: "read the server's message above; the statement ran as the superuser".to_string(),
        })
}

/// `"name"`, with each `"` doubled.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A role name the start writes into SQL unquoted in one place: lower-case
/// letters, digits and `_`, at most 63 bytes.
fn check_role_name(role: &str) -> Result<(), EmbeddedError> {
    let fine = !role.is_empty()
        && role.len() <= 63
        && role
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !role.starts_with(|c: char| c.is_ascii_digit());
    if fine {
        return Ok(());
    }
    Err(EmbeddedError::Config {
        message: format!("`{role}` is not a role name the server can create"),
        help: "use lower-case letters, digits and `_`, starting with a letter, at most 63 \
               characters"
            .to_string(),
    })
}

/// Production keeps its data where a restart finds it: an absolute path,
/// outside the system's temporary directory.
fn check_production_dir(dir: &Path) -> Result<(), EmbeddedError> {
    let help = "pass --db_embedded.dir=<absolute path> on a volume that survives a restart, \
                such as /var/lib/<app>";
    if !dir.is_absolute() {
        return Err(EmbeddedError::Config {
            message: format!(
                "`dir` is {}, a relative path, and production needs an absolute one",
                dir.display()
            ),
            help: help.to_string(),
        });
    }
    let temp = std::env::temp_dir();
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let under_temp = dir.starts_with(&temp)
        || canonical(dir).starts_with(canonical(&temp))
        || dir.starts_with(canonical(&temp));
    if under_temp {
        return Err(EmbeddedError::Config {
            message: format!(
                "`dir` is {}, under the temporary directory {}",
                dir.display(),
                temp.display()
            ),
            help: help.to_string(),
        });
    }
    Ok(())
}

/// Refuses a cluster of another major version: Postgres cannot read its
/// files, and nothing here upgrades them.
fn check_major(data: &Path) -> Result<(), EmbeddedError> {
    let Ok(found) = std::fs::read_to_string(data.join("PG_VERSION")) else {
        return Ok(());
    };
    let found = found.trim();
    let major = POSTGRES_VERSION
        .split('.')
        .next()
        .unwrap_or(POSTGRES_VERSION);
    if found == major {
        return Ok(());
    }
    Err(EmbeddedError::Config {
        message: format!(
            "{} holds a Postgres {found} cluster, and this binary runs Postgres {POSTGRES_VERSION}",
            data.display()
        ),
        help: format!(
            "Postgres cannot read the files of another major version, and the start does not \
             upgrade them: with the binary that runs Postgres {found}, run `db_backup <file>`; \
             start this one once on an empty `dir`, stop it, and run `db_restore <file>`"
        ),
    })
}

/// The port in `postmaster.pid`: its fourth line.
fn running_port(data_dir: &Path) -> Result<u16, EmbeddedError> {
    let file = data_dir.join("postmaster.pid");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    text.lines()
        .nth(3)
        .and_then(|line| line.trim().parse().ok())
        .ok_or_else(|| EmbeddedError::Server {
            action: "join the running server",
            message: format!(
                "another process uses {}, and {} names no port",
                data_dir.display(),
                file.display()
            ),
            help: "stop the other application on this `dir` and start again".to_string(),
        })
}

fn open_lock(path: &Path) -> Result<File, EmbeddedError> {
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|source| EmbeddedError::Io {
            path: path.display().to_string(),
            source,
            help: "set `dir` under [db_embedded] to a directory this process can write",
        })
}

/// Opens `path` and locks it, exclusive; it waits for the lock.
fn lock_file(path: &Path) -> Result<File, EmbeddedError> {
    let file = open_lock(path)?;
    file.lock().map_err(|source| EmbeddedError::Io {
        path: path.display().to_string(),
        source,
        help: "set `dir` under [db_embedded] to a directory on a local disk",
    })?;
    Ok(file)
}

/// The URL with its password replaced.
fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(scheme), Some(at)) => {
            let user_start = scheme + 3;
            let user_end = url[user_start..at].find(':').map_or(at, |i| user_start + i);
            format!("{}:<redacted>{}", &url[..user_end], &url[at..])
        }
        _ => "<redacted>".to_string(),
    }
}

fn pinned_version() -> VersionReq {
    VersionReq::parse(&format!("={POSTGRES_VERSION}")).expect("POSTGRES_VERSION is a version")
}

/// `postgresql-<version>-<target>.tar.gz`, the release asset's name.
fn archive_name() -> String {
    format!(
        "postgresql-{POSTGRES_VERSION}-{}.tar.gz",
        postgresql_archive::matcher::target()
    )
}

/// The cached archive, shared by every checkout on the machine.
fn cached_archive() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".theseus/postgresql").join(archive_name()))
}

/// The archive file on this machine: the one [`ARCHIVE_ENV`] names, or the
/// cache when it holds it. A test that starts a production server passes it
/// as [`EmbeddedConfig::archive`].
pub fn local_archive() -> Option<PathBuf> {
    std::env::var_os(ARCHIVE_ENV)
        .map(PathBuf::from)
        .or_else(|| cached_archive().filter(|file| file.is_file()))
}

/// Whether a first start can install [`POSTGRES_VERSION`] with no network:
/// the archive is bundled, [`ARCHIVE_ENV`] is set, or the cache holds the
/// archive. A test that starts a real server reads it to skip on a machine
/// with none of them.
pub fn archive_on_machine() -> bool {
    bundled() || local_archive().is_some()
}

/// The cached archive, downloaded and checked against the pin when the
/// cache does not hold it. For a test or a tool that needs the file; a
/// start does this itself in development.
///
/// # Errors
///
/// [`EmbeddedError::Install`] when the download fails or the archive does
/// not match its pin.
pub fn fetch_archive() -> Result<PathBuf, EmbeddedError> {
    if let Some(file) = local_archive() {
        return Ok(file);
    }
    let bytes = on_own_runtime(|runtime| runtime.block_on(download()))?;
    verify(&bytes)?;
    cached_archive()
        .filter(|file| file.is_file())
        .ok_or_else(|| {
            install_error(
                "the archive was downloaded, and the cache could not be written".to_string(),
                "check that the home directory is writable",
            )
        })
}

/// Extracts [`POSTGRES_VERSION`] into `server/<version>/` unless every
/// binary in [`REQUIRED_BINARIES`] is there.
///
/// An incomplete copy (a cache that keeps directories and drops files) is
/// removed first. The extraction goes to a directory beside it and is
/// renamed into place, so a process killed halfway leaves no directory that
/// looks installed.
async fn install(server_dir: &Path, config: &EmbeddedConfig) -> Result<(), EmbeddedError> {
    let installed = server_dir.join(POSTGRES_VERSION);
    if is_installed(&installed) {
        return Ok(());
    }
    let io = |path: &Path, source| EmbeddedError::Io {
        path: path.display().to_string(),
        source,
        help: "set `dir` under [db_embedded] to a directory this process can write",
    };
    if installed.exists() {
        std::fs::remove_dir_all(&installed).map_err(|e| io(&installed, e))?;
    }
    let bytes = archive_bytes(config).await?;
    verify(&bytes)?;
    let partial = server_dir.join(format!(".{POSTGRES_VERSION}.partial"));
    let _ = std::fs::remove_dir_all(&partial);
    postgresql_archive::extract(theseus::URL, &bytes, &partial)
        .await
        .map_err(|e| {
            install_error(
                e.to_string(),
                "delete the `server` directory and start again",
            )
        })?;
    std::fs::rename(&partial, &installed).map_err(|e| io(&installed, e))
}

/// Whether `dir` holds every binary in [`REQUIRED_BINARIES`], each a file
/// that is not empty.
fn is_installed(dir: &Path) -> bool {
    REQUIRED_BINARIES.iter().all(|name| {
        let file = dir
            .join("bin")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        std::fs::metadata(file).is_ok_and(|m| m.is_file() && m.len() > 0)
    })
}

/// Checks `bytes` against the SHA-256 pinned for this target.
fn verify(bytes: &[u8]) -> Result<(), EmbeddedError> {
    let target = postgresql_archive::matcher::target();
    let Some(pinned) = pinned_sha256(target) else {
        return Err(install_error(
            format!("Postgres {POSTGRES_VERSION} has no archive for {target}"),
            "run on one of the targets in brocanth_db::embedded::ARCHIVE_SHA256",
        ));
    };
    let found = sha256_hex(bytes);
    if found == pinned {
        return Ok(());
    }
    Err(install_error(
        format!(
            "the archive's SHA-256 is {found}, and brocanth-db pins {pinned} for {}; nothing \
             was extracted",
            archive_name()
        ),
        format!(
            "the file is not the release archive: delete it and download {} again from \
             {}/releases/tag/{POSTGRES_VERSION}",
            archive_name(),
            theseus::URL
        ),
    ))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The archive, from the first source that has it: the bundled bytes,
/// [`EmbeddedConfig::archive`], [`ARCHIVE_ENV`], then in development the
/// cache or a download. The caller checks the pin.
async fn archive_bytes(config: &EmbeddedConfig) -> Result<Vec<u8>, EmbeddedError> {
    if let Some(bytes) = BUNDLED {
        // A copy: `extract` takes a `Vec`.
        return Ok(bytes.to_vec());
    }
    let named = config
        .archive
        .clone()
        .filter(|file| !file.as_os_str().is_empty())
        .map(|file| (file, "`archive` under [db_embedded]".to_string()))
        .or_else(|| {
            std::env::var_os(ARCHIVE_ENV)
                .filter(|v| !v.is_empty())
                .map(|file| (PathBuf::from(file), ARCHIVE_ENV.to_string()))
        });
    if let Some((file, from)) = named {
        return std::fs::read(&file).map_err(|e| {
            install_error(
                format!("{from} names {}: {e}", file.display()),
                format!("set it to the path of {}", archive_name()),
            )
        });
    }
    if config.mode == Mode::Production {
        return Err(install_error(
            "production never downloads the archive, and none is bundled, named or installed"
                .to_string(),
            format!(
                "bundle it (`BROCANTH_POSTGRES_BUNDLE=<file> cargo build --release`), or pass \
                 --db_embedded.archive=<file> with {} from {}/releases/tag/{POSTGRES_VERSION}",
                archive_name(),
                theseus::URL
            ),
        ));
    }
    if let Some(bytes) = cached_archive().and_then(|file| std::fs::read(file).ok()) {
        return Ok(bytes);
    }
    download().await
}

/// One download from the GitHub releases, written to the cache. The caller
/// checks the pin.
async fn download() -> Result<Vec<u8>, EmbeddedError> {
    let (_, bytes) = postgresql_archive::get_archive(theseus::URL, &pinned_version())
        .await
        .map_err(|e| {
            install_error(
                format!("the download failed: {e}"),
                format!(
                    "the first start downloads {} from {}/releases: check the network, set \
                     GITHUB_TOKEN behind a rate limit, or download it by hand and set {ARCHIVE_ENV} \
                     to its path",
                    archive_name(),
                    theseus::URL
                ),
            )
        })?;
    // Not cached unless it matches the pin.
    verify(&bytes)?;
    if let Some(file) = cached_archive() {
        // Best effort: a cache that cannot be written costs a download at the
        // next install, nothing more. Written aside and renamed, so a reader
        // never sees half a file.
        let aside = file.with_extension(format!("{}.partial", std::process::id()));
        let written = file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&aside, &bytes))
            .and_then(|()| std::fs::rename(&aside, &file));
        if written.is_err() {
            let _ = std::fs::remove_file(&aside);
        }
    }
    Ok(bytes)
}

fn install_error(message: String, help: impl Into<String>) -> EmbeddedError {
    EmbeddedError::Install {
        version: POSTGRES_VERSION,
        message,
        help: help.into(),
    }
}

fn create_error(message: &str) -> EmbeddedError {
    EmbeddedError::Server {
        action: "create the database",
        message: message.to_string(),
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

/// A password: the file's, or a new one written to it with mode `0600`.
///
/// For the superuser, `initdb` reads the file once, when it creates the
/// cluster, so the two stay together. A new one is 32 bytes from the
/// operating system's random source, in hex.
fn secret(file: &Path) -> Result<String, EmbeddedError> {
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
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| io(std::io::Error::other(e.to_string())))?;
    let made: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    write_private(file, &made).map_err(io)?;
    Ok(made)
}

/// Writes `contents` to `path`, readable by its owner only.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        let mut file = options.open(path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(contents.as_bytes())
    }
    #[cfg(not(unix))]
    {
        options.open(path)?.write_all(contents.as_bytes())
    }
}

/// Production: copies each new line of the server's log file to stderr,
/// with the prefix `postgres: `. `pg_ctl` detaches the server and writes its
/// log to a file; a container platform collects stderr only.
struct Follower {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Follower {
    /// Follows `log` from byte `from`.
    fn start(log: PathBuf, from: u64) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut offset = from;
            let mut pending = String::new();
            loop {
                let last = flag.load(Ordering::Acquire);
                let read = read_from(&log, &mut offset);
                pending.push_str(&read);
                while let Some(end) = pending.find('\n') {
                    eprintln!("postgres: {}", &pending[..end]);
                    pending.drain(..=end);
                }
                if last {
                    if !pending.is_empty() {
                        eprintln!("postgres: {pending}");
                    }
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }

    /// Copies what is left, and ends the thread.
    fn finish(mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// What `log` holds after `offset`, as text; moves `offset` to its end.
fn read_from(log: &Path, offset: &mut u64) -> String {
    let Ok(mut file) = File::open(log) else {
        return String::new();
    };
    if file.seek(SeekFrom::Start(*offset)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    *offset += bytes.len() as u64;
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Writes a `pg_dump --format=custom` of `config.database` to `file`.
///
/// When a process runs the server of `config.dir`, the dump connects to it
/// and owns nothing: the application keeps serving. Otherwise this starts
/// the server for the dump, in `config.mode`, and stops it. The dump is
/// written aside and renamed, so `file` is whole or absent.
///
/// # Errors
///
/// [`EmbeddedError`], with a `help:` line, when `dir` holds no cluster, the
/// server cannot start, or `pg_dump` fails.
pub fn backup(config: &EmbeddedConfig, file: &Path) -> Result<(), EmbeddedError> {
    let dir = existing_cluster(config)?;
    let aside = file.with_extension("partial");
    let run = |port: u16| {
        let mut dump = client(&dir, "pg_dump", port)?;
        dump.arg("--format=custom")
            .arg("--file")
            .arg(&aside)
            .arg(&config.database);
        run_client(dump, "back up")?;
        // The dump holds every row: readable by its owner only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&aside, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&aside, file).map_err(|source| EmbeddedError::Io {
            path: file.display().to_string(),
            source,
            help: "write the backup to a directory this process can write",
        })
    };
    if let Some(port) = running_elsewhere(&dir)? {
        return run(port);
    }
    let server = EmbeddedPostgres::start(config)?;
    let done = run(server.port());
    server.stop()?;
    done
}

/// Replaces `config.database` with the content of a `pg_dump
/// --format=custom` file, such as one [`backup`] wrote.
///
/// The application must be stopped: the call is refused while a process
/// runs the server. It starts the server, restores into a new database in
/// one transaction, and swaps the names only when the restore succeeded, so
/// a failed restore leaves the database as it was. Then it stops the
/// server.
///
/// The roles the dump names (the application role, and roles a migration
/// created) must exist: restore into the directory the dump came from, or
/// start the application once on the new directory first.
///
/// # Errors
///
/// [`EmbeddedError`], with a `help:` line, when a process runs the server,
/// `file` cannot be read, or `pg_restore` fails.
pub fn restore(config: &EmbeddedConfig, file: &Path) -> Result<(), EmbeddedError> {
    if !file.is_file() {
        return Err(EmbeddedError::Config {
            message: format!("{} is not a file", file.display()),
            help: "pass the path of a file `db_backup` wrote".to_string(),
        });
    }
    let dir = existing_cluster(config)?;
    if running_elsewhere(&dir)?.is_some() {
        return Err(EmbeddedError::Config {
            message: format!("a process runs the server of {}", dir.display()),
            help: "stop the application, run the restore, and start the application again"
                .to_string(),
        });
    }
    let database = config.database.clone();
    let incoming = format!("{database}_restore");
    let previous = format!("{database}_previous");
    if previous.len() > 63 {
        return Err(EmbeddedError::Config {
            message: format!("the database name `{database}` is too long to restore beside"),
            help: "use a database name of at most 54 characters".to_string(),
        });
    }
    let server = EmbeddedPostgres::start(config)?;
    let port = server.port();
    let settings = admin_settings(&dir, port)?;
    let owner = config
        .app_role
        .clone()
        .unwrap_or_else(|| postgresql_embedded::BOOTSTRAP_SUPERUSER.to_string());
    let (q_db, q_in, q_prev) = (
        quote_ident(&database),
        quote_ident(&incoming),
        quote_ident(&previous),
    );
    let sql = |statements: Vec<String>| {
        let settings = settings.clone();
        on_own_runtime(move |runtime| {
            runtime.block_on(async {
                let mut conn = superuser(&settings).await?;
                for statement in &statements {
                    execute(&mut conn, statement, "restore").await?;
                }
                let _ = conn.close().await;
                Ok(())
            })
        })
    };
    let done = sql(vec![
        format!("DROP DATABASE IF EXISTS {q_in}"),
        format!("CREATE DATABASE {q_in} OWNER {}", quote_ident(&owner)),
    ])
    .and_then(|()| {
        let mut load = client(&dir, "pg_restore", port)?;
        load.args(["--exit-on-error", "--single-transaction", "--dbname"])
            .arg(&incoming)
            .arg(file);
        run_client(load, "restore").inspect_err(|_| {
            let _ = sql(vec![format!("DROP DATABASE IF EXISTS {q_in}")]);
        })
    })
    .and_then(|()| {
        sql(vec![
            format!("DROP DATABASE IF EXISTS {q_prev}"),
            format!("ALTER DATABASE {q_db} RENAME TO {q_prev}"),
            format!("ALTER DATABASE {q_in} RENAME TO {q_db}"),
            format!("DROP DATABASE {q_prev}"),
        ])
    });
    server.stop()?;
    done
}

/// The canonical `config.dir`, when it holds a cluster.
fn existing_cluster(config: &EmbeddedConfig) -> Result<PathBuf, EmbeddedError> {
    let dir = std::fs::canonicalize(&config.dir).unwrap_or_else(|_| config.dir.clone());
    if dir.join("data/postgresql.conf").is_file() {
        return Ok(dir);
    }
    Err(EmbeddedError::Config {
        message: format!("{} holds no cluster", dir.display()),
        help: "pass the `dir` the application runs on (`--db_embedded.dir`), or start the \
               application once to create it"
            .to_string(),
    })
}

/// The port of the server another handle runs on `dir`, or `None` when
/// nobody holds `users.lock`.
fn running_elsewhere(dir: &Path) -> Result<Option<u16>, EmbeddedError> {
    let users = open_lock(&dir.join("users.lock"))?;
    match users.try_lock() {
        Ok(()) => Ok(None),
        Err(TryLockError::WouldBlock) => running_port(&dir.join("data")).map(Some),
        Err(TryLockError::Error(source)) => Err(EmbeddedError::Io {
            path: dir.join("users.lock").display().to_string(),
            source,
            help: "set `dir` under [db_embedded] to a directory on a local disk",
        }),
    }
}

/// The superuser's settings for the server on `port`.
fn admin_settings(dir: &Path, port: u16) -> Result<Settings, EmbeddedError> {
    Ok(Settings {
        installation_dir: dir.join("server").join(POSTGRES_VERSION),
        password_file: dir.join("password"),
        data_dir: dir.join("data"),
        host: "localhost".to_string(),
        port,
        password: secret(&dir.join("password"))?,
        version: pinned_version(),
        temporary: false,
        ..settings_without_temp_dirs()
    })
}

/// A client binary of the installed server, connected as the superuser to
/// the server on `port`. The password goes in `PGPASSWORD`, never on the
/// command line.
fn client(dir: &Path, name: &str, port: u16) -> Result<Command, EmbeddedError> {
    let password = secret(&dir.join("password"))?;
    let binary = dir
        .join("server")
        .join(POSTGRES_VERSION)
        .join("bin")
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    let mut command = Command::new(binary);
    command
        .env("PGPASSWORD", password)
        .args(["--host", "localhost", "--port", &port.to_string()])
        .args(["--username", postgresql_embedded::BOOTSTRAP_SUPERUSER])
        .arg("--no-password");
    Ok(command)
}

fn run_client(mut command: Command, action: &'static str) -> Result<(), EmbeddedError> {
    let program = command.get_program().to_string_lossy().into_owned();
    let out = command.output().map_err(|e| EmbeddedError::Server {
        action,
        message: format!("{program}: {e}"),
        help: "start the application once on this `dir`, so the binaries are installed".to_string(),
    })?;
    if out.status.success() {
        return Ok(());
    }
    Err(EmbeddedError::Server {
        action,
        message: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        help: format!("{program} failed with the message above; nothing was replaced"),
    })
}

/// Runs `work` to the end on a new thread with a current-thread runtime.
///
/// A plugin hook is synchronous and may run inside the application's
/// runtime, where `block_on` panics. A thread of its own has no runtime.
fn on_own_runtime<F, T>(work: F) -> Result<T, EmbeddedError>
where
    F: FnOnce(&tokio::runtime::Runtime) -> Result<T, EmbeddedError> + Send + 'static,
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
        work(&runtime)
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

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("brocanth-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    #[test]
    fn a_password_is_made_once_and_read_back() {
        let dir = temp("embedded");
        let file = dir.join("password");
        let made = secret(&file).expect("made");
        assert_eq!(made.len(), 64);
        assert!(made.chars().all(|c| c.is_ascii_hexdigit()), "{made}");
        assert_eq!(secret(&file).expect("read"), made);
        assert_ne!(secret(&dir.join("other")).expect("made"), made);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "{mode:o}");
        }
        std::fs::remove_dir_all(&dir).expect("removed");
    }

    #[test]
    fn a_redacted_url_keeps_all_but_the_password() {
        assert_eq!(
            redact("postgresql://postgres:secret@localhost:5432/app"),
            "postgresql://postgres:<redacted>@localhost:5432/app"
        );
    }

    #[test]
    fn the_port_is_the_fourth_line_of_postmaster_pid() {
        let dir = temp("pid");
        std::fs::write(
            dir.join("postmaster.pid"),
            "123\n/data\n1700000000\n54321\n",
        )
        .expect("written");
        assert_eq!(running_port(&dir).expect("a port"), 54321);
        std::fs::remove_dir_all(&dir).expect("removed");
        assert!(running_port(&dir).is_err());
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

    #[test]
    fn every_supported_target_has_a_pin() {
        assert_eq!(ARCHIVE_SHA256.len(), 8);
        for (target, hash) in ARCHIVE_SHA256 {
            assert_eq!(hash.len(), 64, "{target}");
            assert!(
                hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
                "{target}"
            );
        }
        assert!(pinned_sha256(postgresql_archive::matcher::target()).is_some());
        assert!(pinned_sha256("riscv64gc-unknown-linux-gnu").is_none());
    }

    #[test]
    fn an_archive_that_does_not_match_its_pin_is_refused() {
        let refused = verify(b"not the release archive").expect_err("refused");
        let text = refused.to_string();
        assert!(text.contains("SHA-256"), "{text}");
        assert!(text.contains("nothing was extracted"), "{text}");
        assert!(text.contains("help:"), "{text}");
    }

    #[test]
    fn a_production_directory_is_absolute_and_not_temporary() {
        let relative = check_production_dir(Path::new("tmp/postgres")).expect_err("relative");
        assert!(
            relative.to_string().contains("--db_embedded.dir="),
            "{relative}"
        );
        let temporary = std::env::temp_dir().join("app");
        let refused = check_production_dir(&temporary).expect_err("temporary");
        assert!(refused.to_string().contains("temporary"), "{refused}");
        assert!(check_production_dir(Path::new("/var/lib/my-app")).is_ok());
    }

    #[test]
    fn a_cluster_of_another_major_version_is_refused_with_the_steps() {
        let dir = temp("major");
        assert!(check_major(&dir).is_ok(), "no cluster yet");
        std::fs::write(dir.join("PG_VERSION"), "18\n").expect("written");
        assert!(check_major(&dir).is_ok());
        std::fs::write(dir.join("PG_VERSION"), "17\n").expect("written");
        let text = check_major(&dir).expect_err("17").to_string();
        assert!(text.contains("Postgres 17 cluster"), "{text}");
        assert!(
            text.contains("help:") && text.contains("db_restore"),
            "{text}"
        );
        std::fs::remove_dir_all(&dir).expect("removed");
    }

    #[test]
    fn an_emptied_server_copy_is_not_installed() {
        let dir = temp("installed");
        std::fs::create_dir_all(dir.join("bin")).expect("bin");
        assert!(!is_installed(&dir), "an empty tree");
        for name in REQUIRED_BINARIES {
            let file = dir
                .join("bin")
                .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            std::fs::write(&file, "").expect("written");
        }
        assert!(!is_installed(&dir), "empty files");
        for name in REQUIRED_BINARIES {
            let file = dir
                .join("bin")
                .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
            std::fs::write(&file, "binary").expect("written");
        }
        assert!(is_installed(&dir));
        std::fs::remove_dir_all(&dir).expect("removed");
    }

    #[test]
    fn pg_hba_password_rules_become_scram() {
        let hba = "# TYPE DATABASE USER ADDRESS METHOD\n\
                   # \"local\" uses password\n\
                   local all all password\n\
                   host all all 127.0.0.1/32 password\n\
                   host replication all ::1/128 trust\n";
        let out = scram_only(hba);
        assert!(out.contains("local all all scram-sha-256\n"), "{out}");
        assert!(out.contains("127.0.0.1/32 scram-sha-256\n"), "{out}");
        assert!(out.contains("# \"local\" uses password\n"), "comments stay");
        assert!(out.contains("::1/128 trust\n"));
    }

    #[test]
    fn a_role_name_is_checked_before_it_reaches_sql() {
        assert!(check_role_name("my_blog").is_ok());
        for bad in ["", "My", "a-b", "1x", "x'; DROP", &"a".repeat(64)] {
            assert!(check_role_name(bad).is_err(), "{bad}");
        }
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
    }
}
