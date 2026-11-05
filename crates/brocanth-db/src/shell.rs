//! `dbshell`: the database's own client, on the application's database.
//!
//! Django's `dbshell`, Rails' `dbconsole`. A generated project with `db`
//! and `commands` has a `dbshell` command: it reads `[db] url` the way the
//! application does, and calls [`run`], which replaces the process with
//! `psql` or `sqlite3` on that database.
//!
//! It is a command of the project and not of the `brocanth` CLI because
//! only the project knows how its configuration is read.
//!
//! # Why not a Rust shell
//!
//! `rails console` and `manage.py shell` evaluate the language the
//! application is written in, against the application's loaded classes.
//! Rust has no interpreter, and a project's types exist only inside its
//! compiled binary. What a console is *for* splits in two, and each half
//! has an honest tool: the database's own client to look at the data, and
//! a command of the project (`src/commands/<name>.rs`) to run application
//! code once.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// The client to run, worked out from a URL.
#[derive(Debug, PartialEq, Eq)]
pub struct Client {
    /// `psql` or `sqlite3`.
    pub program: &'static str,
    /// The first argument: a URL without its password, or a file path.
    pub target: String,
    /// The password, passed in `PGPASSWORD` and never on the command line,
    /// where `ps` would show it to every user of the machine.
    pub password: Option<String>,
}

/// Why the client does not start, and what to do.
#[derive(Debug)]
pub struct ShellError {
    /// The rule, such as `dbshell/memory`.
    pub id: &'static str,
    /// What is wrong.
    pub message: String,
    /// What to do about it.
    pub help: Option<String>,
}

impl ShellError {
    fn new(id: &'static str, message: impl Into<String>) -> Self {
        Self {
            id,
            message: message.into(),
            help: None,
        }
    }

    fn help(mut self, text: impl Into<String>) -> Self {
        self.help = Some(text.into());
        self
    }
}

impl fmt::Display for ShellError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error[{}]: {}", self.id, self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\nhelp: {help}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ShellError {}

/// Opens the client on `url`, or prints its command line with `print`.
///
/// `root` is the project root: a relative SQLite path is read from there.
/// `rest` goes to the client unchanged. On Unix this returns only on
/// failure, because the process becomes the client.
pub fn run(url: &str, root: &Path, print: bool, rest: &[OsString]) -> Result<(), ShellError> {
    let client = client_for(url, root)?;
    if print {
        println!("{}", command_line(&client, rest));
        return Ok(());
    }
    eprintln!("{} {}", client.program, client.target);
    handoff(&client, rest)
}

/// The command line, with the password left out.
pub fn command_line(client: &Client, rest: &[OsString]) -> String {
    let mut line = String::new();
    if client.password.is_some() {
        line.push_str("PGPASSWORD=... ");
    }
    line.push_str(client.program);
    line.push(' ');
    line.push_str(&shell_quote(&client.target));
    for arg in rest {
        line.push(' ');
        line.push_str(&shell_quote(&arg.to_string_lossy()));
    }
    line
}

/// `psql` for a Postgres URL, `sqlite3` for a SQLite one.
pub fn client_for(url: &str, root: &Path) -> Result<Client, ShellError> {
    if let Some(rest) = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
    {
        let path = rest.split('?').next().unwrap_or(rest);
        if path.is_empty() || path == ":memory:" || rest.contains("mode=memory") {
            return Err(ShellError::new(
                "dbshell/memory",
                "the database is in memory, so no other process can open it",
            )
            .help("point the URL at a file, such as sqlite://db/development.sqlite?mode=rwc"));
        }
        let file = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            root.join(path)
        };
        if !file.is_file() {
            // `sqlite3` would create an empty file here and open it without a
            // word, which looks exactly like a database whose tables vanished.
            return Err(ShellError::new(
                "dbshell/no-file",
                format!("the database file {} does not exist", file.display()),
            )
            .help("start the application or run its migrations once; that creates the file"));
        }
        return Ok(Client {
            program: "sqlite3",
            target: file.display().to_string(),
            password: None,
        });
    }

    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        let (target, password) = without_password(url);
        return Ok(Client {
            program: "psql",
            target,
            password,
        });
    }

    let scheme = url.split(':').next().unwrap_or(url);
    Err(ShellError::new(
        "dbshell/scheme",
        format!("dbshell has no client for a `{scheme}` URL"),
    )
    .help("`[db] url` is a postgres:// URL; dbshell also opens a sqlite:// file"))
}

/// The URL with `user:password@` turned into `user@`, and the password.
fn without_password(url: &str) -> (String, Option<String>) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_owned(), None);
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return (url.to_owned(), None);
    };
    match userinfo.split_once(':') {
        Some((user, password)) => (
            format!("{scheme}://{user}@{host}{tail}"),
            Some(percent_decode(password)),
        ),
        None => (url.to_owned(), None),
    }
}

/// `%40` back to `@`: a URL password with a reserved character is encoded,
/// and `PGPASSWORD` wants the password itself.
fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&raw[at + 1..at + 3], 16) {
                out.push(byte);
                at += 3;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Single quotes around anything a shell would split or expand.
fn shell_quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@=,+%".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

fn not_found(client: &Client) -> ShellError {
    let install = match client.program {
        "psql" => {
            "install the PostgreSQL client (`brew install libpq`, `apt install postgresql-client`)"
        }
        _ => "install SQLite's shell (`brew install sqlite`, `apt install sqlite3`)",
    };
    ShellError::new(
        "dbshell/no-client",
        format!("`{}` is not on PATH", client.program),
    )
    .help(format!("{install}, or use --print to see the command line"))
}

#[cfg(unix)]
fn handoff(client: &Client, rest: &[OsString]) -> Result<(), ShellError> {
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new(client.program);
    command.arg(&client.target).args(rest);
    if let Some(password) = &client.password {
        command.env("PGPASSWORD", password);
    }
    let error = command.exec();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Err(not_found(client));
    }
    Err(ShellError::new(
        "dbshell/exec",
        format!("cannot run `{}`: {error}", client.program),
    ))
}

#[cfg(not(unix))]
fn handoff(client: &Client, rest: &[OsString]) -> Result<(), ShellError> {
    let mut command = std::process::Command::new(client.program);
    command.arg(&client.target).args(rest);
    if let Some(password) = &client.password {
        command.env("PGPASSWORD", password);
    }
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(not_found(client)),
        Err(e) => Err(ShellError::new(
            "dbshell/exec",
            format!("cannot run `{}`: {e}", client.program),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_postgres_password_leaves_the_command_line() {
        let c = client_for(
            "postgres://app:s%40cret@db.internal:5432/blog?sslmode=require",
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(c.program, "psql");
        assert_eq!(
            c.target,
            "postgres://app@db.internal:5432/blog?sslmode=require"
        );
        assert_eq!(c.password.as_deref(), Some("s@cret"));
        let line = command_line(&c, &[]);
        assert!(line.starts_with("PGPASSWORD=... psql "), "{line}");
        assert!(!line.contains("s@cret"), "{line}");
    }

    #[test]
    fn a_postgres_url_with_no_password_is_passed_as_it_is() {
        let c = client_for("postgresql://localhost/dev", Path::new("/")).unwrap();
        assert_eq!(c.target, "postgresql://localhost/dev");
        assert_eq!(c.password, None);
    }

    #[test]
    fn a_relative_sqlite_path_is_read_from_the_project_root() {
        let root = std::env::temp_dir().join(format!("brocanth-dbshell-{}", std::process::id()));
        std::fs::create_dir_all(root.join("db")).unwrap();
        std::fs::write(root.join("db/dev.sqlite"), b"").unwrap();
        let c = client_for("sqlite://db/dev.sqlite?mode=rwc", &root).unwrap();
        assert_eq!(c.program, "sqlite3");
        assert_eq!(c.target, root.join("db/dev.sqlite").display().to_string());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_sqlite_file_is_refused_rather_than_created() {
        let e = client_for("sqlite://db/absent.sqlite", Path::new("/nonexistent")).unwrap_err();
        assert_eq!(e.id, "dbshell/no-file");
        assert!(e.to_string().contains("help:"), "{e}");
    }

    #[test]
    fn memory_and_unknown_schemes_say_why() {
        let e = client_for("sqlite::memory:", Path::new("/")).unwrap_err();
        assert_eq!(e.id, "dbshell/memory");
        let e = client_for("mysql://localhost/x", Path::new("/")).unwrap_err();
        assert_eq!(e.id, "dbshell/scheme");
        assert!(e.to_string().contains("help:"));
    }

    #[test]
    fn quoting_leaves_plain_words_alone() {
        assert_eq!(shell_quote("postgres://a@b/c"), "postgres://a@b/c");
        assert_eq!(shell_quote("it's here"), r"'it'\''s here'");
    }
}
