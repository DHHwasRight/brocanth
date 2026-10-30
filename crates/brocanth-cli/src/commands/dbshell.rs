//! `brocanth dbshell` -- the database's own client, on the project's database.
//!
//! Django's `dbshell`, Rails' `dbconsole`. It reads `[model] url` from the
//! configuration of one environment and replaces itself with `psql` or
//! `sqlite3` on that database.
//!
//! # Why not a Rust shell
//!
//! `rails console` and `manage.py shell` evaluate the language the
//! application is written in, against the application's loaded classes.
//! Rust has no interpreter, and a project's types exist only inside its
//! compiled binary. A REPL such as `evcxr` compiles each line as a new
//! crate: it cannot see a project's private modules, it cannot hold the
//! application's running state, and it rebuilds the dependency tree on the
//! first line. Calling that a console would promise a thing it cannot do.
//!
//! What a console is *for* splits in two, and each half has an honest tool:
//!
//! * **Look at the data.** This command: the database's own client, which
//!   is a better data shell than anything written here.
//! * **Run application code once.** A command in the project
//!   (`src/commands/<name>.rs`, run as `brocanth <name>`). It is compiled
//!   against the project, so it sees every type, and it is a file, so it can
//!   be reviewed and run again.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;

use crate::commands::project::Project;
use crate::commands::Outcome;
use crate::ui::{paint, Diagnostic, Ui, ACCENT, DIM};

#[derive(Args)]
pub struct DbshellArgs {
    /// The environment whose `[model] url` to open [default: $<APP>_ENV, or development]
    #[arg(long, short, value_name = "ENV")]
    pub env: Option<String>,
    /// Print the command line instead of running it. The password is not printed
    #[arg(long)]
    pub print: bool,
    /// Passed to psql or sqlite3 unchanged, after `--`
    #[arg(last = true, value_name = "CLIENT ARGS")]
    pub rest: Vec<OsString>,
}

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

pub fn run(ui: &Ui, dir: Option<&Path>, args: DbshellArgs) -> Outcome {
    let project = Project::find(dir)?;
    // An empty list is a marker from before the list, which says nothing.
    if !project.modules.is_empty() && !project.has("model") {
        return Err(vec![Diagnostic::error(
            "dbshell/no-model",
            "this project has no model layer, so it names no database",
        )
        .help(
            "`brocanth add model` gives it one, with `[model] url` in the config",
        )]);
    }
    let config = project.config(args.env.as_deref())?;
    let env = config.env().to_owned();

    let Some(url) = config.root().at("model.url").and_then(|v| v.as_str()) else {
        return Err(vec![Diagnostic::error(
            "dbshell/no-url",
            format!("the `{env}` configuration has no `[model] url`"),
        )
        .help(format!(
            "set `url` under `[model]` in {}, or set ${}",
            project.config_file(&env),
            project.variable("model", "url")
        ))]);
    };
    let from = config
        .provenance("model.url")
        .map(ToString::to_string)
        .unwrap_or_default();

    let client = client_for(url, &project.root)
        .map_err(|d| vec![d.note(format!("`[model] url` is set at {from}"))])?;

    if args.print {
        let mut line = String::new();
        if client.password.is_some() {
            line.push_str("PGPASSWORD=... ");
        }
        line.push_str(client.program);
        line.push(' ');
        line.push_str(&shell_quote(&client.target));
        for arg in &args.rest {
            line.push(' ');
            line.push_str(&shell_quote(&arg.to_string_lossy()));
        }
        ui.result(line);
        return Ok(ExitCode::SUCCESS);
    }

    ui.note(format!(
        "{} {} {}",
        paint(ACCENT, client.program),
        client.target,
        paint(DIM, format!("({env}, from {from})"))
    ));
    handoff(&client, &args.rest)
}

/// `psql` for a Postgres URL, `sqlite3` for a SQLite one.
pub fn client_for(url: &str, root: &Path) -> Result<Client, Box<Diagnostic>> {
    if let Some(rest) = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
    {
        let path = rest.split('?').next().unwrap_or(rest);
        if path.is_empty() || path == ":memory:" || rest.contains("mode=memory") {
            return Err(Box::new(Diagnostic::error(
                "dbshell/memory",
                "the database is in memory, so no other process can open it",
            )
            .help(
                "point `[model] url` at a file, such as sqlite://db/development.sqlite?mode=rwc",
            )));
        }
        let file = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            root.join(path)
        };
        if !file.is_file() {
            // `sqlite3` would create an empty file here and open it without a
            // word, which looks exactly like a database whose tables vanished.
            return Err(Box::new(
                Diagnostic::error(
                    "dbshell/no-file",
                    format!("the database file {} does not exist", file.display()),
                )
                .help("start the application or run its migrations once; that creates the file"),
            ));
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
    Err(Box::new(
        Diagnostic::error(
            "dbshell/scheme",
            format!("brocanth has no client for a `{scheme}` URL"),
        )
        .help(
            "`[model] url` is a postgres:// or a sqlite:// URL; brocanth-model supports these two",
        ),
    ))
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

fn not_found(client: &Client) -> Diagnostic {
    let install = match client.program {
        "psql" => {
            "install the PostgreSQL client (`brew install libpq`, `apt install postgresql-client`)"
        }
        _ => "install SQLite's shell (`brew install sqlite`, `apt install sqlite3`)",
    };
    Diagnostic::error(
        "dbshell/no-client",
        format!("`{}` is not on PATH", client.program),
    )
    .help(format!("{install}, or use --print to see the command line"))
}

#[cfg(unix)]
fn handoff(client: &Client, rest: &[OsString]) -> Outcome {
    use std::os::unix::process::CommandExt;

    let mut command = std::process::Command::new(client.program);
    command.arg(&client.target).args(rest);
    if let Some(password) = &client.password {
        command.env("PGPASSWORD", password);
    }
    let error = command.exec();
    if error.kind() == std::io::ErrorKind::NotFound {
        return Err(vec![not_found(client)]);
    }
    Err(vec![Diagnostic::error(
        "dbshell/exec",
        format!("cannot run `{}`: {error}", client.program),
    )])
}

#[cfg(not(unix))]
fn handoff(client: &Client, rest: &[OsString]) -> Outcome {
    let mut command = std::process::Command::new(client.program);
    command.arg(&client.target).args(rest);
    if let Some(password) = &client.password {
        command.env("PGPASSWORD", password);
    }
    match command.status() {
        Ok(status) => Ok(ExitCode::from(
            status
                .code()
                .and_then(|c| u8::try_from(c).ok())
                .unwrap_or(1),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(vec![not_found(client)]),
        Err(e) => Err(vec![Diagnostic::error(
            "dbshell/exec",
            format!("cannot run `{}`: {e}", client.program),
        )]),
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
        let d = client_for("sqlite://db/absent.sqlite", Path::new("/nonexistent")).unwrap_err();
        assert_eq!(d.code.as_deref(), Some("dbshell/no-file"));
        assert!(d.render().contains("help:"), "{}", d.render());
    }

    #[test]
    fn memory_and_unknown_schemes_say_why() {
        let d = client_for("sqlite::memory:", Path::new("/")).unwrap_err();
        assert_eq!(d.code.as_deref(), Some("dbshell/memory"));
        let d = client_for("mysql://localhost/x", Path::new("/")).unwrap_err();
        assert_eq!(d.code.as_deref(), Some("dbshell/scheme"));
        assert!(d.render().contains("help:"));
    }

    #[test]
    fn quoting_leaves_plain_words_alone() {
        assert_eq!(shell_quote("postgres://a@b/c"), "postgres://a@b/c");
        assert_eq!(shell_quote("it's here"), r"'it'\''s here'");
    }
}
