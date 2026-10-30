//! `brocanth check` and `brocanth check --deploy` -- what is wrong before it
//! is live.
//!
//! Django's `check --deploy` (D23). Nothing is built and nothing connects:
//! the command reads `brocanth.toml`, loads the configuration of one
//! environment through the application's own loader, and reads `src/` as
//! text. So it runs in CI with no database, and every finding points at a
//! line somebody can change.
//!
//! The list comes from what brocanth has. Each finding has an id, a message,
//! a location when there is one, and a `help:` line:
//!
//! | Id | Severity | When |
//! |---|---|---|
//! | `check/dialect` | error | `[policy] dialect` is not the database of `[model] url` |
//! | `deploy/env` | warning | `--deploy` on `development` or `test` |
//! | `deploy/no-config` | warning | no file and no variable configures this environment |
//! | `deploy/cookie-insecure` | error | `[sessions] secure` or `[auth] secure` is `false` |
//! | `deploy/cookie-prefix` | warning | `[auth] cookie_name` has no `__Host-` prefix |
//! | `deploy/signing-key` | error | `[auth] signing_key_env` names a variable that is unset or short |
//! | `deploy/default-secret` | error | `[storage] secret` is the scaffold's development string, or short |
//! | `deploy/secret-in-file` | warning | a key named like a secret has its value in a file |
//! | `deploy/debug-logging` | warning | `[logging] level` is `debug` or `trace` |
//! | `deploy/jobs-migrate` | warning | `[jobs] migrate = true` |
//! | `deploy/database` | error | a database URL is in memory or under a temporary directory |
//! | `deploy/csrf-off` | error | the project has cookies and no `Csrf` hook is registered |
//! | `deploy/hooks-not-run` | error | hooks are registered and nothing in `src/` runs them |
//! | `deploy/hsts` | warning | nothing in `src/` sets `Strict-Transport-Security` |
//! | `deploy/admin-unscoped` | error | `unscoped_rows_unsafe_for_development()` is called |
//!
//! `--allow <ID>` silences one finding, as Django's
//! `SILENCED_SYSTEM_CHECKS` does, and it is written on the command line or in
//! CI where a reviewer sees it. The exit code is 1 when an error remains,
//! and with `--strict` when a warning remains too.
//!
//! The source checks read text, not types. A registration that is built in a
//! way this scan does not see is reported as absent; `--allow` is the answer
//! once a person has looked.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use brocanth_config::{Config, Provenance, Redaction, Value};
use clap::Args;

use crate::commands::project::Project;
use crate::commands::Outcome;
use crate::ui::{paint, Diagnostic, Severity, Ui, BAD, OK, WARN};

#[derive(Args)]
pub struct CheckArgs {
    /// Add the production checks: secrets, cookies, CSRF, the admin, the database
    #[arg(long)]
    pub deploy: bool,
    /// The environment to load [default: production with --deploy, else $<APP>_ENV or development]
    #[arg(long, short, value_name = "ENV")]
    pub env: Option<String>,
    /// Silence one finding by its id, such as deploy/hsts. Repeat for more
    #[arg(long, value_name = "ID")]
    pub allow: Vec<String>,
    /// Exit 1 on a warning too
    #[arg(long)]
    pub strict: bool,
}

/// The scaffold's own placeholder for `[storage] secret`.
const SCAFFOLD_SECRET: &str = "development only";

pub fn run(ui: &Ui, dir: Option<&Path>, args: CheckArgs) -> Outcome {
    let project = Project::find(dir)?;
    let env = match (&args.env, args.deploy) {
        (Some(env), _) => Some(env.clone()),
        (None, true) => Some("production".to_owned()),
        (None, false) => None,
    };
    let config = project.config(env.as_deref())?;
    let sources = Sources::read(&project.root);
    let lookup = |name: &str| std::env::var(name).ok();

    let mut findings = always(&project, &config);
    if args.deploy {
        findings.extend(deploy(&project, &config, &sources, &lookup));
    }
    findings.retain(|d| !d.code.as_ref().is_some_and(|c| args.allow.contains(c)));

    for d in &findings {
        ui.diagnostic(d);
    }
    let errors = findings
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = findings.len() - errors;
    let what = if args.deploy {
        "deploy checks"
    } else {
        "checks"
    };
    let env = config.env();
    if findings.is_empty() {
        ui.done(format!("{what} pass for `{env}`"));
    } else {
        ui.note(format!(
            "{what} for `{env}`: {}, {}",
            paint(if errors > 0 { BAD } else { OK }, plural(errors, "error")),
            paint(
                if warnings > 0 { WARN } else { OK },
                plural(warnings, "warning")
            ),
        ));
    }
    let failed = errors > 0 || (args.strict && warnings > 0);
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

// -- what is read -------------------------------------------------------------

/// The `.rs` files under `src/`, as text.
pub struct Sources {
    root: PathBuf,
    files: Vec<(PathBuf, String)>,
}

impl Sources {
    pub fn read(root: &Path) -> Self {
        let mut files = Vec::new();
        let mut stack = vec![root.join("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        files.push((path, text));
                    }
                }
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        Self {
            root: root.to_path_buf(),
            files,
        }
    }

    #[cfg(test)]
    pub fn of(files: &[(&str, &str)]) -> Self {
        Self {
            root: PathBuf::from("/p"),
            files: files
                .iter()
                .map(|(p, t)| (PathBuf::from("/p").join(p), (*t).to_owned()))
                .collect(),
        }
    }

    /// Every line, outside a `//` comment, that contains `needle`, as
    /// `src/file.rs:line`.
    ///
    /// Only whole-line comments are skipped. The scaffold writes its
    /// optional registrations as whole-line comments, and that is the case
    /// that must not count as code.
    pub fn find(&self, needle: &str) -> Vec<String> {
        let mut out = Vec::new();
        for (path, text) in &self.files {
            let mut in_block = false;
            for (i, line) in text.lines().enumerate() {
                let t = line.trim_start();
                if in_block {
                    if t.contains("*/") {
                        in_block = false;
                    }
                    continue;
                }
                if t.starts_with("/*") {
                    in_block = !t.contains("*/");
                    continue;
                }
                if t.starts_with("//") {
                    continue;
                }
                let code = line.split("//").next().unwrap_or(line);
                if code.contains(needle) {
                    let rel = path.strip_prefix(&self.root).unwrap_or(path);
                    out.push(format!("{}:{}", rel.display(), i + 1));
                }
            }
        }
        out
    }

    fn any(&self, needles: &[&str]) -> Vec<String> {
        needles.iter().flat_map(|n| self.find(n)).collect()
    }
}

/// Where a configuration value came from, relative to the project.
fn place(project: &Project, value: &Value) -> String {
    match value.provenance() {
        Provenance::File { file, line, .. } => {
            let rel = file.path.strip_prefix(&project.root).unwrap_or(&file.path);
            format!("{}:{line}", rel.display())
        }
        other => other.to_string(),
    }
}

fn get<'a>(config: &'a Config, path: &str) -> Option<&'a Value> {
    config.root().at(path)
}

// -- the checks ---------------------------------------------------------------

/// The checks that hold in every environment.
pub fn always(project: &Project, config: &Config) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    if let (Some(url), Some(dialect)) = (get(config, "model.url"), get(config, "policy.dialect")) {
        let backend = url.as_str().and_then(backend_of);
        let said = dialect.as_str().unwrap_or_default();
        if let Some(backend) = backend {
            if !said.eq_ignore_ascii_case(backend) {
                out.push(
                    Diagnostic::error(
                        "check/dialect",
                        format!(
                            "`[policy] dialect` is `{said}`, and `[model] url` is a {backend} database"
                        ),
                    )
                    .at(place(project, dialect))
                    .note(format!("`[model] url` is set at {}", place(project, url)))
                    .help(format!(
                        "set `dialect = \"{backend}\"`: a condition compiled for another dialect is SQL the database refuses"
                    )),
                );
            }
        }
    }
    out
}

fn backend_of(url: &str) -> Option<&'static str> {
    if url.starts_with("sqlite:") {
        Some("sqlite")
    } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Some("postgres")
    } else {
        None
    }
}

/// The production checks.
pub fn deploy(
    project: &Project,
    config: &Config,
    sources: &Sources,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let env = config.env().to_owned();
    let file = project.config_file(&env);

    if matches!(env.as_str(), "development" | "test") {
        out.push(
            Diagnostic::warning(
                "deploy/env",
                format!("the deploy checks read the `{env}` configuration"),
            )
            .help("pass --env production, or the name of the environment you deploy"),
        );
    }
    if config
        .root()
        .as_table()
        .is_none_or(brocanth_config::Table::is_empty)
    {
        out.push(
            Diagnostic::warning(
                "deploy/no-config",
                format!("no file and no variable configures `{env}`"),
            )
            .help(format!(
                "create {file}, or point ${} at the file the deployment reads",
                project.app().env_var_for_config()
            )),
        );
    }

    // Cookies.
    for section in ["sessions", "auth"] {
        if let Some(v) = get(config, &format!("{section}.secure")) {
            if v.as_bool() == Some(false) {
                out.push(
                    Diagnostic::error(
                        "deploy/cookie-insecure",
                        format!("`[{section}] secure` is false, so the cookie also travels over plain HTTP"),
                    )
                    .at(place(project, v))
                    .help(format!(
                        "set `secure = true` under `[{section}]` in {file}, or ${}=true",
                        project.variable(section, "secure")
                    )),
                );
            }
        }
    }
    if let Some(v) = get(config, "auth.cookie_name") {
        if let Some(name) = v.as_str().filter(|n| !n.starts_with("__Host-")) {
            out.push(
                Diagnostic::warning(
                    "deploy/cookie-prefix",
                    format!("the session cookie `{name}` has no `__Host-` prefix"),
                )
                .at(place(project, v))
                .help("name it `__Host-session`: the browser then refuses it from a subdomain or over HTTP"),
            );
        }
    }

    // Secrets.
    if let Some(v) = get(config, "auth.signing_key_env") {
        if let Some(var) = v.as_str() {
            let problem = match lookup(var) {
                None => Some(format!("${var} is not set in this environment")),
                Some(key) if key.trim_end_matches('=').len() < 43 => {
                    Some(format!("${var} is shorter than 32 bytes of base64url"))
                }
                Some(_) => None,
            };
            if let Some(problem) = problem {
                out.push(
                    Diagnostic::error("deploy/signing-key", format!("the session signing key: {problem}"))
                        .at(place(project, v))
                        .note("the application refuses to start without it")
                        .help(format!(
                            "export {var}=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '='), and run this check where the deployment's environment is"
                        )),
                );
            }
        }
    }
    let mut reported: Vec<String> = Vec::new();
    if let Some(v) = get(config, "storage.secret") {
        let secret = v.as_str().unwrap_or_default();
        let problem = if secret.contains(SCAFFOLD_SECRET) {
            Some("is the scaffold's development value, and the repository shows it to anybody")
        } else if secret.len() < 32 {
            Some("is shorter than 32 characters")
        } else {
            None
        };
        if let Some(problem) = problem {
            reported.push("storage.secret".to_owned());
            out.push(
                Diagnostic::error("deploy/default-secret", format!("`[storage] secret` {problem}"))
                    .at(place(project, v))
                    .note("it signs file tokens and keys the address of each stored object")
                    .help(format!(
                        "set ${} to 32 random bytes in the deployment, and remove the value from {file}",
                        project.variable("storage", "secret")
                    )),
            );
        }
    }
    let redaction = Redaction::standard();
    for (path, value) in config.root().leaves() {
        if reported.contains(&path) || !redaction.hides(&path) {
            continue;
        }
        if value.as_str().is_none_or(str::is_empty) {
            continue;
        }
        if let Provenance::File { .. } = value.provenance() {
            let (section, key) = path.split_once('.').unwrap_or(("", path.as_str()));
            out.push(
                Diagnostic::warning(
                    "deploy/secret-in-file",
                    format!("`{path}` is named like a secret, and its value is in a file"),
                )
                .at(place(project, value))
                .help(format!(
                    "set ${} in the deployment and keep the value out of the repository",
                    project.variable(section, &key.replace('.', "__"))
                )),
            );
        }
    }

    // Development settings.
    if let Some(v) = get(config, "logging.level") {
        if let Some(level) = v
            .as_str()
            .filter(|l| l.eq_ignore_ascii_case("debug") || l.eq_ignore_ascii_case("trace"))
        {
            out.push(
                Diagnostic::warning(
                    "deploy/debug-logging",
                    format!("`[logging] level` is `{level}`"),
                )
                .at(place(project, v))
                .help(format!("set `level = \"info\"` under `[logging]` in {file}: a debug log is slow and can hold request data")),
            );
        }
    }
    if let Some(v) = get(config, "jobs.migrate") {
        if v.as_bool() == Some(true) {
            out.push(
                Diagnostic::warning(
                    "deploy/jobs-migrate",
                    "`[jobs] migrate` is true, so the application creates tables at startup",
                )
                .at(place(project, v))
                .help("set `migrate = false` and put `Queue::schema_sql` in a migration"),
            );
        }
    }
    for section in ["model", "cache", "jobs"] {
        let Some(v) = get(config, &format!("{section}.url")) else {
            continue;
        };
        let Some(url) = v.as_str() else { continue };
        if let Some(problem) = temporary_database(url) {
            out.push(
                Diagnostic::error(
                    "deploy/database",
                    format!(
                        "`[{section}] url` {problem}, so the data does not outlive the process"
                    ),
                )
                .at(place(project, v))
                .help(format!(
                    "point ${} at a Postgres server, or at a SQLite file on a persistent disk",
                    project.variable(section, "url")
                )),
            );
        }
    }

    // CSRF, and whether hooks run at all.
    let has_cookies = project.modules.is_empty() || project.has("sessions") || project.has("auth");
    let csrf = sources.find("brocanth_middleware::Csrf");
    let csrf_registered = csrf.iter().any(|at| {
        sources
            .find("plugin_hooking")
            .iter()
            .any(|h| same_file(h, at))
    }) || !sources.find("Csrf::new").is_empty()
        || !sources.find("Csrf>::new").is_empty();
    if has_cookies && !csrf_registered {
        let mut d = Diagnostic::error(
            "deploy/csrf-off",
            "no CSRF check is registered, and this application uses cookies",
        )
        .note(
            "any page on any site can post a form here, and the browser sends the cookies with it",
        )
        .help("register `brocanth_middleware::Csrf` with `plugin_hooking` in src/plugins/mod.rs");
        if project.has("middleware-csrf") {
            d = d.note("the scaffold wrote the registration commented out; its comment says the two changes that make it work");
        }
        out.push(d);
    }
    let hooks = sources.find("plugin_hooking");
    let runner = sources.any(&[
        "Bridge",
        "brocanth_server::serve",
        "bridge::serve",
        "App::run",
    ]);
    if !hooks.is_empty() && runner.is_empty() {
        out.push(
            Diagnostic::error(
                "deploy/hooks-not-run",
                "hooks are registered, and nothing in src/ serves through the hook pipeline",
            )
            .at(hooks[0].clone())
            .note("a request that goes from the route table to the handler skips every hook, CSRF included")
            .help("serve with `brocanth_server::serve` or `brocanth_server::Bridge`, which run the hooks"),
        );
    }

    if sources.find("Strict-Transport-Security").is_empty() {
        out.push(
            Diagnostic::warning(
                "deploy/hsts",
                "nothing in src/ sets `Strict-Transport-Security`",
            )
            .note("brocanth has no security-headers hook yet")
            .help("set HSTS at the TLS terminator and pass --allow deploy/hsts, or add the header in a ResponseHook"),
        );
    }

    for at in sources.find("unscoped_rows_unsafe_for_development") {
        out.push(
            Diagnostic::error(
                "deploy/admin-unscoped",
                "the admin shows every row of a model that has no resource",
            )
            .at(at)
            .note("`unscoped_rows_unsafe_for_development()` turns off the row policy for those models")
            .help("remove the call and register a `Resource` with a row policy for each model the admin shows"),
        );
    }
    out
}

fn same_file(a: &str, b: &str) -> bool {
    a.rsplit_once(':').map(|x| x.0) == b.rsplit_once(':').map(|x| x.0)
}

/// Why a database URL does not outlive the process, if it does not.
fn temporary_database(url: &str) -> Option<&'static str> {
    let rest = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))?;
    let path = rest.split('?').next().unwrap_or(rest);
    if path.is_empty() || path == ":memory:" || rest.contains("mode=memory") {
        return Some("is an in-memory SQLite database");
    }
    let tmp = std::env::temp_dir();
    let under_tmp = ["/tmp/", "/var/tmp/", "/private/tmp/", "/dev/shm/"]
        .iter()
        .any(|p| path.starts_with(p))
        || Path::new(path).starts_with(&tmp);
    under_tmp.then_some("is a SQLite file under a temporary directory")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(modules: &[&str]) -> Project {
        Project {
            root: PathBuf::from("/p"),
            name: "shop".to_owned(),
            modules: modules.iter().map(|m| (*m).to_owned()).collect(),
        }
    }

    fn ids(found: &[Diagnostic]) -> Vec<&str> {
        found.iter().filter_map(|d| d.code.as_deref()).collect()
    }

    const SCAFFOLD: &str = r#"
[logging]
level = "debug"

[sessions]
secure = false

[model]
url = "sqlite::memory:"

[jobs]
migrate = true

[auth]
signing_key_env = "SHOP_SESSION_KEY"
cookie_name = "session"
secure = false

[policy]
dialect = "postgres"

[storage]
secret = "development only: replace before anything is uploaded in anger"
"#;

    #[test]
    fn the_scaffold_s_development_config_fails_every_check_it_should() {
        let config = Config::from_toml("shop", "production", SCAFFOLD).unwrap();
        let p = project(&["sessions", "auth", "middleware-csrf"]);
        let src = Sources::of(&[("src/plugins/mod.rs", "        // let builder = builder.plugin_hooking(\n        //     <brocanth_middleware::Csrf as Plugin>::new(\n")]);
        let mut found = always(&p, &config);
        found.extend(deploy(&p, &config, &src, &|_| None));
        let ids = ids(&found);
        for id in [
            "check/dialect",
            "deploy/cookie-insecure",
            "deploy/cookie-prefix",
            "deploy/signing-key",
            "deploy/default-secret",
            "deploy/debug-logging",
            "deploy/jobs-migrate",
            "deploy/database",
            "deploy/csrf-off",
            "deploy/hsts",
        ] {
            assert!(ids.contains(&id), "{id} missing from {ids:?}");
        }
        assert_eq!(
            ids.iter()
                .filter(|i| **i == "deploy/cookie-insecure")
                .count(),
            2
        );
        // The storage secret is reported once, as the default, not twice.
        assert!(!ids.contains(&"deploy/secret-in-file"), "{ids:?}");
        assert!(
            !ids.contains(&"deploy/hooks-not-run"),
            "a comment registers nothing"
        );
        for d in &found {
            assert!(
                d.notes.iter().any(|(k, _)| *k == "help"),
                "{} has no help",
                d.render()
            );
        }
    }

    #[test]
    fn a_production_config_passes() {
        let text = r#"
[logging]
level = "info"
[sessions]
secure = true
[model]
url = "postgres://app@db/shop"
[auth]
signing_key_env = "SHOP_SESSION_KEY"
cookie_name = "__Host-session"
secure = true
[policy]
dialect = "postgres"
"#;
        let config = Config::from_toml("shop", "production", text).unwrap();
        let p = project(&["sessions", "auth"]);
        let src = Sources::of(&[
            ("src/plugins/mod.rs", "let builder = builder.plugin_hooking(\n <brocanth_middleware::Csrf as Plugin>::new(c));"),
            ("src/app.rs", "brocanth_server::serve(addr, app, router)"),
            ("src/hsts.rs", "\"Strict-Transport-Security\""),
        ]);
        let key = "k".repeat(43);
        let mut found = always(&p, &config);
        found.extend(deploy(&p, &config, &src, &|_| Some(key.clone())));
        assert!(found.is_empty(), "{:#?}", ids(&found));
    }

    #[test]
    fn hooks_with_no_runner_and_the_unscoped_admin_are_located() {
        let config = Config::from_toml("shop", "production", "").unwrap();
        let src = Sources::of(&[
            (
                "src/plugins/mod.rs",
                "\nlet b = b.plugin_hooking(brocanth_middleware::Csrf::new(c));",
            ),
            (
                "src/admin.rs",
                "AdminConfig::new()\n  .unscoped_rows_unsafe_for_development()",
            ),
        ]);
        let found = deploy(&project(&["auth"]), &config, &src, &|_| None);
        let hooks = found
            .iter()
            .find(|d| d.code.as_deref() == Some("deploy/hooks-not-run"))
            .unwrap();
        assert_eq!(hooks.location.as_deref(), Some("src/plugins/mod.rs:2"));
        let admin = found
            .iter()
            .find(|d| d.code.as_deref() == Some("deploy/admin-unscoped"))
            .unwrap();
        assert_eq!(admin.location.as_deref(), Some("src/admin.rs:2"));
        assert!(!ids(&found).contains(&"deploy/csrf-off"));
    }

    #[test]
    fn a_password_in_a_file_is_a_warning_with_the_variable_to_use() {
        let config = Config::from_toml(
            "shop",
            "production",
            "[mail]\nsmtp_password = \"hunter2\"\n",
        )
        .unwrap();
        let found = deploy(&project(&[]), &config, &Sources::of(&[]), &|_| None);
        let d = found
            .iter()
            .find(|d| d.code.as_deref() == Some("deploy/secret-in-file"))
            .expect("named like a secret");
        assert!(
            d.render().contains("$SHOP_MAIL__SMTP_PASSWORD"),
            "{}",
            d.render()
        );
        assert!(
            !d.render().contains("hunter2"),
            "a finding never prints the secret"
        );
    }

    #[test]
    fn a_temporary_sqlite_file_does_not_outlive_the_process() {
        assert!(temporary_database("sqlite:///tmp/x.sqlite?mode=rwc").is_some());
        assert!(temporary_database("sqlite://db/production.sqlite").is_none());
        assert!(temporary_database("postgres://db/x").is_none());
    }
}
