//! Plain SQL migrations, for a project with `db` and no `model`.
//!
//! A migration is one file, `migrations/<YYYYMMDD_HHMMSS>_<name>.sql`. The
//! statements before a line `-- down` apply it. The statements after that
//! line undo it. A file with no `-- down` statement cannot be rolled back.
//!
//! ```sql
//! CREATE TABLE posts (id bigserial PRIMARY KEY, title text NOT NULL);
//!
//! -- down
//! DROP TABLE posts;
//! ```
//!
//! The files are applied in name order, so in the order they were written.
//! Each migration runs in its own transaction, with its row in
//! `brocanth_migrations`, so a failed statement leaves nothing behind. A
//! Postgres advisory lock keeps two `migrate up` runs from applying the
//! same file.
//!
//! Why not SeaORM's migrator: it needs Rust migrations compiled into the
//! binary, and that is the `model` module's path (`docs/migrations.md`).
//! This path reads SQL files at run time, so it needs no list to keep in
//! step and no build. The table is not SeaORM's `seaql_migrations`:
//! SeaORM refuses a row it has no file for, so a shared table would break
//! the day a project adds `model`.

use std::fmt;
use std::path::{Path, PathBuf};

use sqlx::{AssertSqlSafe, Connection, PgConnection};

use crate::{Db, DbError};

/// The table that records each applied migration.
pub const TABLE: &str = "brocanth_migrations";

/// The key of the advisory lock `up` and `down` hold. Any fixed number
/// works; this one is "brocanth" in ASCII.
const LOCK: i64 = 0x6272_6f63_616e_7468;

/// The line that separates the statements that apply a migration from the
/// statements that undo it.
pub const DOWN_MARKER: &str = "-- down";

/// One `.sql` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Migration {
    /// The file name without `.sql`: `20261105_101500_add_posts`. This is
    /// the key in `brocanth_migrations`.
    pub name: String,
    /// The statements that apply it.
    pub up: String,
    /// The statements that undo it. `None` when the file has no `-- down`
    /// statement.
    pub down: Option<String>,
}

impl Migration {
    /// Splits a file at its `-- down` line.
    pub fn parse(name: impl Into<String>, text: &str) -> Migration {
        let mut up = String::new();
        let mut down = String::new();
        let mut in_down = false;
        for line in text.split_inclusive('\n') {
            if !in_down && line.trim().eq_ignore_ascii_case(DOWN_MARKER) {
                in_down = true;
                continue;
            }
            if in_down {
                down.push_str(line);
            } else {
                up.push_str(line);
            }
        }
        Migration {
            name: name.into(),
            up,
            down: has_statement(&down).then_some(down),
        }
    }
}

/// Whether `sql` holds more than blank lines and `--` comments.
fn has_statement(sql: &str) -> bool {
    sql.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with("--")
    })
}

/// Where a migration is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// In `brocanth_migrations`.
    Applied,
    /// A file that is not applied yet.
    Pending,
    /// In `brocanth_migrations`, with no file. It cannot be rolled back.
    Missing,
}

impl State {
    /// `applied`, `pending` or `missing`, as `migrate status` prints it.
    pub fn as_str(self) -> &'static str {
        match self {
            State::Applied => "applied",
            State::Pending => "pending",
            State::Missing => "no file",
        }
    }
}

/// Something `migrate` refuses to do, or could not do.
#[derive(Debug)]
#[non_exhaustive]
pub enum MigrateError {
    /// A file in the directory could not be read or written.
    Io {
        /// The file or directory.
        path: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// A `.sql` file whose name does not start with a stamp.
    BadName(String),
    /// A name for `new` that is not snake case.
    NotSnakeCase(String),
    /// `down` reached a migration it cannot undo.
    Irreversible(String),
    /// `down` reached an applied migration whose file is gone.
    NoFile(String),
    /// A statement failed. The migration's transaction was rolled back.
    Failed {
        /// The migration.
        name: String,
        /// `up` or `down`.
        direction: &'static str,
        /// What the database said.
        source: Box<DbError>,
    },
    /// The database refused a bookkeeping statement.
    Db(Box<DbError>),
}

impl fmt::Display for MigrateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(
                f,
                "{}: {source}\nhelp: run this command from the project root, next to migrations/",
                path.display()
            ),
            Self::BadName(file) => write!(
                f,
                "migrations/{file} does not start with a stamp, so its order is unknown\n\
                 help: rename it to <YYYYMMDD_HHMMSS>_<name>.sql, or write a new file with \
                 `migrate new <name>`"
            ),
            Self::NotSnakeCase(name) => write!(
                f,
                "the name `{name}` is not snake case\n\
                 help: use lower-case letters, digits and `_`, such as `add_posts`"
            ),
            Self::Irreversible(name) => write!(
                f,
                "migrations/{name}.sql has no statement after `-- down`, so it cannot be \
                 rolled back\nhelp: write the statements that undo it after a `-- down` line, \
                 or write a new migration that undoes it"
            ),
            Self::NoFile(name) => write!(
                f,
                "{name} is applied, and migrations/{name}.sql is gone, so it cannot be \
                 rolled back\nhelp: restore the file from version control"
            ),
            Self::Failed {
                name,
                direction,
                source,
            } => write!(
                f,
                "migrations/{name}.sql ({direction}) failed, and nothing of it was kept: {source}"
            ),
            Self::Db(source) => write!(f, "{source}"),
        }
    }
}

impl std::error::Error for MigrateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Failed { source, .. } | Self::Db(source) => Some(source.as_ref()),
            _ => None,
        }
    }
}

impl From<DbError> for MigrateError {
    fn from(e: DbError) -> Self {
        Self::Db(Box::new(e))
    }
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> MigrateError + '_ {
    move |source| MigrateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Every `.sql` file in `dir`, in name order. A missing directory holds
/// none.
///
/// # Errors
/// [`MigrateError::Io`] when a file cannot be read, and
/// [`MigrateError::BadName`] for a `.sql` file with no stamp.
pub fn read_dir(dir: &Path) -> Result<Vec<Migration>, MigrateError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io(dir)(e)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(io(dir))?.path();
        let Some(file) = path.file_name().and_then(|f| f.to_str()) else {
            continue;
        };
        let Some(name) = file.strip_suffix(".sql") else {
            continue;
        };
        if !stamped(name) {
            return Err(MigrateError::BadName(file.to_string()));
        }
        let text = std::fs::read_to_string(&path).map_err(io(&path))?;
        files.push(Migration::parse(name, &text));
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// `YYYYMMDD_HHMMSS_` and a name.
fn stamped(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() > 16
        && b[..8].iter().all(u8::is_ascii_digit)
        && b[8] == b'_'
        && b[9..15].iter().all(u8::is_ascii_digit)
        && b[15] == b'_'
}

/// What `new` writes into a new file.
const TEMPLATE: &str = "-- Statements that apply this migration. They run in one transaction.\n\
\n\
\n\
-- down\n\
-- Statements that undo it, in reverse order. Without one, `migrate down`\n\
-- refuses this migration.\n\
\n";

/// Writes an empty migration, `<dir>/<UTC stamp>_<name>.sql`, and returns
/// its path. Creates `dir` when it does not exist.
///
/// # Errors
/// [`MigrateError::NotSnakeCase`] for a bad name, [`MigrateError::Io`]
/// when the file cannot be written or already exists.
pub fn new_file(dir: &Path, name: &str) -> Result<PathBuf, MigrateError> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    new_file_at(dir, name, secs)
}

fn new_file_at(dir: &Path, name: &str, secs: u64) -> Result<PathBuf, MigrateError> {
    if !snake_case(name) {
        return Err(MigrateError::NotSnakeCase(name.to_string()));
    }
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let path = dir.join(format!("{}_{name}.sql", stamp_of(secs)));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(io(&path))?;
    std::io::Write::write_all(&mut file, TEMPLATE.as_bytes()).map_err(io(&path))?;
    Ok(path)
}

fn snake_case(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// `YYYYMMDD_HHMMSS` in UTC: the stamp `brocanth-migrate` writes, so both
/// kinds of migration sort the same way. Copied, not shared:
/// `brocanth-migrate` links a Rust parser, and this crate must not.
fn stamp_of(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rest = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}_{:02}{:02}{:02}",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Howard Hinnant's `civil_from_days`: days since 1970-01-01 to a date.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The migrations on disk, applied to one database.
pub struct Migrator<'a> {
    db: &'a Db,
    files: Vec<Migration>,
}

impl<'a> Migrator<'a> {
    /// The files, in name order, against `db`.
    pub fn new(db: &'a Db, mut files: Vec<Migration>) -> Self {
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Self { db, files }
    }

    /// Each migration and its state: every file, then each applied name
    /// with no file.
    ///
    /// # Errors
    /// [`MigrateError::Db`] when the table cannot be read or created.
    pub async fn status(&self) -> Result<Vec<(String, State)>, MigrateError> {
        let mut conn = self.connection().await?;
        let applied = applied(&mut conn).await?;
        Ok(self.states(&applied))
    }

    fn states(&self, applied: &[String]) -> Vec<(String, State)> {
        let mut out: Vec<(String, State)> = self
            .files
            .iter()
            .map(|m| {
                let state = if applied.contains(&m.name) {
                    State::Applied
                } else {
                    State::Pending
                };
                (m.name.clone(), state)
            })
            .collect();
        for name in applied {
            if !self.files.iter().any(|m| &m.name == name) {
                out.push((name.clone(), State::Missing));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Applies each pending file, oldest first. Returns their names.
    ///
    /// # Errors
    /// [`MigrateError::Failed`] for the first file that fails. The files
    /// before it stay applied.
    pub async fn up(&self) -> Result<Vec<String>, MigrateError> {
        let mut conn = self.connection().await?;
        lock(&mut conn).await?;
        let result = self.up_on(&mut conn).await;
        unlock(&mut conn).await;
        result
    }

    async fn up_on(&self, conn: &mut PgConnection) -> Result<Vec<String>, MigrateError> {
        let applied = applied(conn).await?;
        let mut done = Vec::new();
        for m in self.files.iter().filter(|m| !applied.contains(&m.name)) {
            run(conn, &m.name, "up", &m.up, Record::Insert).await?;
            done.push(m.name.clone());
        }
        Ok(done)
    }

    /// Undoes the newest `n` applied migrations, newest first. Returns
    /// their names.
    ///
    /// Checks all `n` before it undoes one: a missing file or a missing
    /// `-- down` statement stops it before any change.
    ///
    /// # Errors
    /// [`MigrateError::NoFile`], [`MigrateError::Irreversible`], or
    /// [`MigrateError::Failed`] for the first that fails.
    pub async fn down(&self, n: usize) -> Result<Vec<String>, MigrateError> {
        let mut conn = self.connection().await?;
        lock(&mut conn).await?;
        let result = self.down_on(&mut conn, n).await;
        unlock(&mut conn).await;
        result
    }

    async fn down_on(
        &self,
        conn: &mut PgConnection,
        n: usize,
    ) -> Result<Vec<String>, MigrateError> {
        let applied = applied(conn).await?;
        let mut steps = Vec::new();
        for name in applied.iter().rev().take(n) {
            let file = self
                .files
                .iter()
                .find(|m| &m.name == name)
                .ok_or_else(|| MigrateError::NoFile(name.clone()))?;
            let down = file
                .down
                .as_deref()
                .ok_or_else(|| MigrateError::Irreversible(name.clone()))?;
            steps.push((name, down));
        }
        let mut done = Vec::new();
        for (name, down) in steps {
            run(conn, name, "down", down, Record::Delete).await?;
            done.push(name.clone());
        }
        Ok(done)
    }

    async fn connection(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Postgres>, MigrateError> {
        let mut conn = self.db.pool().acquire().await.map_err(DbError::connect)?;
        let create = format!(
            "CREATE TABLE IF NOT EXISTS {TABLE} (\
             name text PRIMARY KEY, \
             applied_at timestamptz NOT NULL DEFAULT now())"
        );
        sqlx::raw_sql(AssertSqlSafe(create))
            .execute(&mut *conn)
            .await
            .map_err(DbError::query)?;
        Ok(conn)
    }
}

/// The applied names, in name order.
async fn applied(conn: &mut PgConnection) -> Result<Vec<String>, MigrateError> {
    let select = format!("SELECT name FROM {TABLE} ORDER BY name");
    let names: Vec<String> = sqlx::query_scalar(AssertSqlSafe(select))
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::query)?;
    Ok(names)
}

async fn lock(conn: &mut PgConnection) -> Result<(), MigrateError> {
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(LOCK)
        .execute(&mut *conn)
        .await
        .map_err(DbError::query)?;
    Ok(())
}

/// Best effort: the lock also ends with the connection.
async fn unlock(conn: &mut PgConnection) {
    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(LOCK)
        .execute(&mut *conn)
        .await;
}

enum Record {
    Insert,
    Delete,
}

/// One migration step and its bookkeeping row, in one transaction.
async fn run(
    conn: &mut PgConnection,
    name: &str,
    direction: &'static str,
    sql: &str,
    record: Record,
) -> Result<(), MigrateError> {
    let failed = |source: sqlx::Error| MigrateError::Failed {
        name: name.to_string(),
        direction,
        source: Box::new(DbError::query(source)),
    };
    let mut tx = conn.begin().await.map_err(failed)?;
    sqlx::raw_sql(AssertSqlSafe(sql.to_string()))
        .execute(&mut *tx)
        .await
        .map_err(failed)?;
    let book = match record {
        Record::Insert => format!("INSERT INTO {TABLE} (name) VALUES ($1)"),
        Record::Delete => format!("DELETE FROM {TABLE} WHERE name = $1"),
    };
    sqlx::query(AssertSqlSafe(book))
        .bind(name)
        .execute(&mut *tx)
        .await
        .map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_splits_at_its_down_line() {
        let m = Migration::parse(
            "20261105_101500_add_posts",
            "CREATE TABLE posts (id int);\n\n-- down\nDROP TABLE posts;\n",
        );
        assert_eq!(m.up, "CREATE TABLE posts (id int);\n\n");
        assert_eq!(m.down.as_deref(), Some("DROP TABLE posts;\n"));
    }

    #[test]
    fn a_down_of_comments_alone_is_no_down() {
        let m = Migration::parse("x", TEMPLATE);
        assert_eq!(m.down, None);
        assert!(!has_statement(&m.up));
        assert_eq!(Migration::parse("x", "SELECT 1;\n").down, None);
    }

    #[test]
    fn the_stamp_is_utc_and_sorts_by_time() {
        assert_eq!(stamp_of(0), "19700101_000000");
        // 2026-11-05 10:15:00 UTC.
        assert_eq!(stamp_of(1_793_873_700), "20261105_101500");
    }

    #[test]
    fn new_writes_a_stamped_file_and_refuses_a_bad_name() {
        let dir = std::env::temp_dir().join(format!("brocanth-db-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = new_file_at(&dir, "add_posts", 1_793_873_700).unwrap();
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "20261105_101500_add_posts.sql"
        );
        assert!(matches!(
            new_file_at(&dir, "add_posts", 1_793_873_700),
            Err(MigrateError::Io { .. })
        ));
        let error = new_file_at(&dir, "Add Posts", 0).unwrap_err();
        assert!(error.to_string().contains("help:"), "{error}");
        let read = read_dir(&dir).unwrap();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].name, "20261105_101500_add_posts");
        std::fs::write(dir.join("posts.sql"), "SELECT 1;").unwrap();
        assert!(matches!(read_dir(&dir), Err(MigrateError::BadName(_))));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_directory_holds_no_migration() {
        assert!(read_dir(Path::new("/no/such/brocanth/dir"))
            .unwrap()
            .is_empty());
    }
}
