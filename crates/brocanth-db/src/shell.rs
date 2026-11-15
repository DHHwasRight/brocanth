//! `dbshell`: the database's own client, on the application's database.
//!
//! Django's `dbshell`, Rails' `dbconsole`. A generated project with `db`
//! and `commands` has a `dbshell` command: it reads `[db] url` the way the
//! application does, and calls [`run`], which replaces the process with
//! `psql` on that database.
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
use std::path::Path;

/// The client to run, worked out from a URL.
#[derive(Debug, PartialEq, Eq)]
pub struct Client {
    /// `psql`.
    pub program: &'static str,
    /// The first argument: the URL without its password.
    pub target: String,
    /// The password, passed in `PGPASSWORD` and never on the command line,
    /// where `ps` would show it to every user of the machine.
    pub password: Option<String>,
}

/// Why the client does not start, and what to do.
#[derive(Debug)]
pub struct ShellError {
    /// The rule, such as `dbshell/scheme`.
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
/// `root` is the project root. Postgres is the only database, so nothing
/// reads it now; the generated `dbshell` command still passes it.
/// `rest` goes to the client unchanged. On Unix this returns only on
/// failure, because the process becomes the client.
pub fn run(url: &str, root: &Path, print: bool, rest: &[OsString]) -> Result<(), ShellError> {
    let _ = root;
    let client = client_for(url)?;
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

/// `psql` for a Postgres URL, as the pool reads it: an upper-case scheme
/// and a SQLAlchemy `+driver` suffix work too.
pub fn client_for(url: &str) -> Result<Client, ShellError> {
    let url = crate::config::normalize_url(url).map_err(|_| {
        let scheme = url.split(':').next().unwrap_or(url);
        ShellError::new(
            "dbshell/scheme",
            format!("dbshell has no client for a `{scheme}` URL"),
        )
        .help("set `[db] url` to a postgres:// URL")
    })?;
    let (target, password) = without_password(&url);
    Ok(Client {
        program: "psql",
        target,
        password,
    })
}

/// The URL with no password, and the password. `user:password@` becomes
/// `user@`, and a `password=` query parameter goes away: psql reads both,
/// and `ps` shows both to every user of the machine.
fn without_password(url: &str) -> (String, Option<String>) {
    let Some((scheme, rest)) = url.split_once("://") else {
        return (url.to_owned(), None);
    };
    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let (authority, mut password) = match authority.rsplit_once('@') {
        Some((userinfo, host)) => match userinfo.split_once(':') {
            Some((user, password)) => (format!("{user}@{host}"), Some(percent_decode(password))),
            None => (authority.to_owned(), None),
        },
        None => (authority.to_owned(), None),
    };
    let tail = match tail.split_once('?') {
        Some((path, query)) => {
            let (query, fragment) = match query.split_once('#') {
                Some((query, fragment)) => (query, Some(fragment)),
                None => (query, None),
            };
            let mut kept = Vec::new();
            for pair in query.split('&') {
                match pair.split_once('=') {
                    Some(("password", value)) => {
                        password = Some(percent_decode(value));
                    }
                    _ => kept.push(pair),
                }
            }
            let mut out = path.to_owned();
            if !kept.is_empty() {
                out.push('?');
                out.push_str(&kept.join("&"));
            }
            if let Some(fragment) = fragment {
                out.push('#');
                out.push_str(fragment);
            }
            out
        }
        None => tail.to_owned(),
    };
    (format!("{scheme}://{authority}{tail}"), password)
}

/// `%40` back to `@`: a URL password with a reserved character is encoded,
/// and `PGPASSWORD` wants the password itself. A `%` not followed by two
/// hex digits stays as it is.
fn percent_decode(raw: &str) -> String {
    fn hex(b: u8) -> Option<u8> {
        match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        }
    }
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' && at + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[at + 1]), hex(bytes[at + 2])) {
                out.push(hi << 4 | lo);
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
    let install =
        "install the PostgreSQL client (`brew install libpq`, `apt install postgresql-client`)";
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
        let c =
            client_for("postgres://app:s%40cret@db.internal:5432/blog?sslmode=require").unwrap();
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
        let c = client_for("postgresql://localhost/dev").unwrap();
        assert_eq!(c.target, "postgresql://localhost/dev");
        assert_eq!(c.password, None);
    }

    #[test]
    fn an_unknown_scheme_says_why() {
        let e = client_for("sqlite://db/dev.sqlite").unwrap_err();
        assert_eq!(e.id, "dbshell/scheme");
        let e = client_for("mysql://localhost/x").unwrap_err();
        assert_eq!(e.id, "dbshell/scheme");
        assert!(e.to_string().contains("help:"));
    }

    #[test]
    fn a_password_query_parameter_leaves_the_target() {
        let c = client_for("postgres://app@db/blog?sslmode=require&password=s%40cret").unwrap();
        assert_eq!(c.target, "postgres://app@db/blog?sslmode=require");
        assert_eq!(c.password.as_deref(), Some("s@cret"));
        let c = client_for("postgres://db/blog?password=x").unwrap();
        assert_eq!(c.target, "postgres://db/blog");
        assert_eq!(c.password.as_deref(), Some("x"));
    }

    #[test]
    fn the_schemes_the_pool_takes_work_here_too() {
        let c = client_for("POSTGRES://localhost/dev").unwrap();
        assert_eq!(c.target, "postgres://localhost/dev");
        let c = client_for("postgresql+psycopg2://u:p@h/d").unwrap();
        assert_eq!(c.target, "postgresql://u@h/d");
        assert_eq!(c.password.as_deref(), Some("p"));
    }

    #[test]
    fn a_bad_escape_stays_as_written() {
        assert_eq!(percent_decode("%aé"), "%aé");
        assert_eq!(percent_decode("%+1"), "%+1");
        assert_eq!(percent_decode("a%4"), "a%4");
        assert_eq!(percent_decode("%41%4a"), "AJ");
        assert_eq!(percent_decode("é%"), "é%");
    }

    #[test]
    fn quoting_leaves_plain_words_alone() {
        assert_eq!(shell_quote("postgres://a@b/c"), "postgres://a@b/c");
        assert_eq!(shell_quote("it's here"), r"'it'\''s here'");
    }
}
