//! A Postgres server started from the binary, for development.
//!
//! The `db-embedded` module writes a plugin over this: it starts the server
//! before the `db` plugin opens its pool, and hands `db` the URL through
//! [`EmbeddedUrl`]. The pool stays `db`'s. This module opens no pool.
//!
//! Built on `postgresql_embedded`, with one pinned Postgres release,
//! [`POSTGRES_VERSION`]. A build needs no network. The first start installs
//! the binaries from the first of:
//!
//! 1. the file that [`ARCHIVE_ENV`] (`BROCANTH_POSTGRES_ARCHIVE`) names, for
//!    a machine with no network;
//! 2. the cache, `~/.theseus/postgresql/postgresql-<version>-<target>.tar.gz`
//!    (the file name `postgresql_embedded`'s own build script caches to);
//! 3. one download from the GitHub releases of `theseus-rs/postgresql-binaries`,
//!    checked against its SHA-256 file and written to the cache.
//!    `GITHUB_TOKEN` lifts the API rate limit.
//!
//! A later start finds the binaries under `server/` and reads nothing.
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
//! - `setup.lock`, `users.lock`: see [`EmbeddedPostgres::start`].
//!
//! One server per directory. Every [`EmbeddedPostgres`] on one directory
//! shares it: the handles of one process through a registry keyed by the
//! canonical directory, the processes through the two lock files. Tests
//! that run in parallel, and a second terminal, join the running server.
//! The last handle released stops it.
//!
//! Every call blocks. Each one runs on a thread of its own with its own
//! runtime, so a plugin hook calls it on any runtime, current-thread
//! included, and a command calls it with none.

use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::fs::{File, TryLockError};
use std::hash::{BuildHasher, Hasher};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use postgresql_archive::configuration::theseus;
use postgresql_embedded::{PostgreSQL, Settings, Status, VersionReq};
use thiserror::Error;

/// The Postgres release the server runs. Pinned, so a start never asks
/// which release is the latest, and every machine runs the same server.
pub const POSTGRES_VERSION: &str = "18.7.0";

/// The variable that names an archive file to install from, in place of the
/// cache and the download: `postgresql-<POSTGRES_VERSION>-<target>.tar.gz`
/// from the releases of `theseus-rs/postgresql-binaries`.
pub const ARCHIVE_ENV: &str = "BROCANTH_POSTGRES_ARCHIVE";

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
    /// The archive could not be read, downloaded or extracted.
    #[error("embedded Postgres could not install {version}: {message}\nhelp: {help}")]
    Install {
        /// [`POSTGRES_VERSION`].
        version: &'static str,
        /// What failed.
        message: String,
        /// What to do next.
        help: String,
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

/// A handle on the development server of one directory. Release it with
/// [`EmbeddedPostgres::stop`]; a drop releases it too.
///
/// Every handle on one directory shares one server, in this process and
/// across processes. The last handle released stops it.
pub struct EmbeddedPostgres {
    /// `None` once released.
    shared: Option<Arc<Mutex<Shared>>>,
    dir: PathBuf,
    url: String,
    redacted: String,
    data_dir: PathBuf,
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
    /// A shared lock on `users.lock`, held while this process uses the
    /// server. The operating system drops it when the process dies.
    _in_use: File,
}

/// One entry per canonical directory, for the life of the process.
fn registry(dir: &Path) -> Arc<Mutex<Shared>> {
    static REGISTRY: LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<Shared>>>>> =
        LazyLock::new(Mutex::default);
    let mut map = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    map.entry(dir.to_path_buf()).or_default().clone()
}

impl EmbeddedPostgres {
    /// Joins the server of `config.dir`, or starts it.
    ///
    /// The first start installs the binaries and initialises `data/`. When
    /// a handle of this process or of another process holds the server, the
    /// call joins it: same port, same password. Otherwise it starts the
    /// server. Every call creates `config.database` if it is missing.
    ///
    /// Two lock files in the directory order the processes:
    ///
    /// - `setup.lock`, exclusive, is held while a process installs,
    ///   initialises, starts, joins or stops. A second process waits for the
    ///   first to finish, and `initdb` runs once.
    /// - `users.lock`, shared, is held by each process that uses the server.
    ///   A process that dies, `kill -9` included, drops its lock. A server
    ///   that runs while nobody holds `users.lock` is an orphan: the next
    ///   start stops it and starts it again.
    ///
    /// # Errors
    ///
    /// [`EmbeddedError`], with a `help:` line, when a directory cannot be
    /// written, the archive cannot be installed, the port is taken, or the
    /// server does not answer.
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
        let dir = std::fs::canonicalize(&dir).map_err(|source| EmbeddedError::Io {
            path: dir.display().to_string(),
            source,
            help: "set `dir` under [db_embedded] to a directory this process can read",
        })?;
        let shared = registry(&dir);
        let (config, held) = (config.clone(), shared.clone());
        let task_dir = dir.clone();
        let url = on_own_runtime(move |runtime| {
            let dir = task_dir;
            // Held to the end: the handles of this process take turns.
            let mut guard = held.lock().unwrap_or_else(PoisonError::into_inner);
            let state: &mut Shared = &mut guard;
            let _setup = lock_file(&dir.join("setup.lock"))?;
            runtime.block_on(async {
                if state.running.is_none() {
                    state.running = Some(join_or_start(&dir, &config).await?);
                }
                state.users += 1;
                let settings = state.running.as_ref().map(|r| r.settings.clone());
                let settings = settings.expect("set above");
                match ensure_database(&settings, &config.database).await {
                    Ok(()) => Ok(settings.url(&config.database)),
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
/// process runs, or starts it. The caller holds `setup.lock`.
async fn join_or_start(dir: &Path, config: &EmbeddedConfig) -> Result<Running, EmbeddedError> {
    let settings = Settings {
        installation_dir: dir.join("server"),
        password_file: dir.join("password"),
        data_dir: dir.join("data"),
        host: "localhost".to_string(),
        port: config.port,
        password: password(&dir.join("password"))?,
        version: pinned_version(),
        // The cluster survives the process: a stop keeps the files.
        temporary: false,
        timeout: Some(config.timeout),
        ..settings_without_temp_dirs()
    };
    install(&settings.installation_dir).await?;
    initialise(&settings).await?;
    let users_file = dir.join("users.lock");
    let users = open_lock(&users_file)?;
    let lock_io = |source| EmbeddedError::Io {
        path: users_file.display().to_string(),
        source,
        help: "set `dir` under [db_embedded] to a directory on a local disk",
    };
    let mut server = ManuallyDrop::new(PostgreSQL::new(settings));
    let settings = match users.try_lock() {
        Ok(()) => {
            // No other process uses the server. A `postmaster.pid` is an
            // orphan's, from a process killed with its server running, or a
            // stale file: `pg_ctl stop` ends the one, the start removes the
            // other.
            if server.status() == Status::Started {
                let _ = server.stop().await;
            }
            server.start().await.map_err(|e| EmbeddedError::Server {
                action: "start",
                message: e.to_string(),
                help: format!(
                    "read {}; a taken port is fixed by `port = 0` under [db_embedded]",
                    dir.join("data/start.log").display()
                ),
            })?;
            // Still under `setup.lock`: no process sees the switch.
            users.unlock().map_err(lock_io)?;
            server.settings().clone()
        }
        Err(TryLockError::WouldBlock) => {
            // Another process holds the server and started it under
            // `setup.lock`: take its port.
            let mut settings = server.settings().clone();
            settings.port = running_port(&settings.data_dir)?;
            settings
        }
        Err(TryLockError::Error(source)) => return Err(lock_io(source)),
    };
    users.lock_shared().map_err(lock_io)?;
    Ok(Running {
        settings,
        _in_use: users,
    })
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
    }) = state.running.take()
    else {
        return Ok(());
    };
    drop(in_use);
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
            "delete {} to install and initialise it again",
            data.parent().unwrap_or(data).display()
        ),
    })?;
    std::fs::rename(&partial, data).map_err(|e| io(data, e))
}

/// Creates `database` when it is missing. The caller holds `setup.lock`,
/// so two processes do not both create it.
async fn ensure_database(settings: &Settings, database: &str) -> Result<(), EmbeddedError> {
    let server = ManuallyDrop::new(PostgreSQL::new(settings.clone()));
    let exists = server
        .database_exists(database)
        .await
        .map_err(|e| create_error(&e))?;
    if !exists {
        server
            .create_database(database)
            .await
            .map_err(|e| create_error(&e))?;
    }
    Ok(())
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

/// Whether a first start can install [`POSTGRES_VERSION`] with no network:
/// [`ARCHIVE_ENV`] is set, or the cache holds the archive. A test that starts
/// a real server reads it to skip on a machine with neither.
pub fn archive_on_machine() -> bool {
    std::env::var_os(ARCHIVE_ENV).is_some() || cached_archive().is_some_and(|file| file.is_file())
}

/// Extracts [`POSTGRES_VERSION`] into `server/<version>/` unless it is there.
///
/// The extraction goes to a directory beside it and is renamed into place,
/// so a process killed halfway leaves no directory that looks installed.
async fn install(server_dir: &Path) -> Result<(), EmbeddedError> {
    let installed = server_dir.join(POSTGRES_VERSION);
    if installed.is_dir() {
        return Ok(());
    }
    let bytes = archive_bytes().await?;
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
    std::fs::rename(&partial, &installed).map_err(|source| EmbeddedError::Io {
        path: installed.display().to_string(),
        source,
        help: "set `dir` under [db_embedded] to a directory this process can write",
    })
}

/// The archive: the file [`ARCHIVE_ENV`] names, the cache, or a download.
async fn archive_bytes() -> Result<Vec<u8>, EmbeddedError> {
    if let Some(file) = std::env::var_os(ARCHIVE_ENV) {
        let file = PathBuf::from(file);
        return std::fs::read(&file).map_err(|e| {
            install_error(
                format!("{ARCHIVE_ENV}={}: {e}", file.display()),
                format!("set {ARCHIVE_ENV} to the path of {}", archive_name()),
            )
        });
    }
    let cache = cached_archive();
    if let Some(bytes) = cache.as_deref().and_then(|file| std::fs::read(file).ok()) {
        return Ok(bytes);
    }
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
    if let Some(file) = cache {
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
    fn a_redacted_url_keeps_all_but_the_password() {
        assert_eq!(
            redact("postgresql://postgres:secret@localhost:5432/app"),
            "postgresql://postgres:<redacted>@localhost:5432/app"
        );
    }

    #[test]
    fn the_port_is_the_fourth_line_of_postmaster_pid() {
        let dir = std::env::temp_dir().join(format!("brocanth-pid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
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
}
