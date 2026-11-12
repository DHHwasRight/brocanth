//! `check` and `check --deploy`: what is wrong with a project before it is
//! live.
//!
//! Django's `check --deploy`. Nothing is built and nothing connects: the
//! checks load the configuration of one environment through [`Loader`],
//! read `brocanth.toml` for the module list, and read `src/` as text. So
//! they run in CI with no database, and every finding points at a line
//! somebody can change.
//!
//! A generated project with `config` and `commands` runs them as its own
//! `check` command, which calls [`run`]. They live here and not in the
//! `brocanth` CLI because they read the configuration the way the
//! application does, and only this crate knows how it does.
//!
//! | Id | Severity | When |
//! |---|---|---|
//! | `check/dialect` | error | `[policy] dialect` is not the database of `[db] url` |
//! | `deploy/env` | warning | `--deploy` on `development` or `test` |
//! | `deploy/no-config` | warning | no file and no variable configures this environment |
//! | `deploy/cookie-insecure` | error | `[sessions] secure` or `[auth] secure` is `false` |
//! | `deploy/cookie-prefix` | warning | `[auth] cookie_name` has no `__Host-` prefix |
//! | `deploy/signing-key` | error | `[auth] signing_key_env` names a variable that is unset or short |
//! | `deploy/default-secret` | error | `[storage] secret` is the scaffold's development string, or short |
//! | `deploy/secret-in-file` | warning | a key named like a secret has its value in a file |
//! | `deploy/debug-logging` | warning | `[logging] level` is `debug` or `trace` |
//! | `deploy/jobs-migrate` | warning | `[jobs] migrate = true` |
//! | `deploy/csrf-off` | error | the project has cookies and no `Csrf` hook is registered |
//! | `deploy/hooks-not-run` | error | hooks are registered and nothing in `src/` runs them |
//! | `deploy/hsts` | warning | no `SecurityHeaders` hook and no `Strict-Transport-Security` in `src/`, or the hook has HSTS off, or nothing tells it a request came over TLS |
//! | `deploy/admin-unscoped` | error | `unscoped_rows_unsafe_for_development()` is called |
//!
//! `check/recommended` (a module recommends one the project lacks) is the
//! `brocanth` CLI's: only the CLI knows every module's manifest.
//!
//! The source checks read text, not types. A registration built in a way
//! this scan does not see is reported as absent; `--allow` is the answer
//! once a person has looked.

use std::fmt;
use std::path::{Path, PathBuf};

use crate::{AppName, Config, ConfigError, Loader, Provenance, Redaction, Table, Value};

/// The scaffold's own placeholder for `[storage] secret`.
const SCAFFOLD_SECRET: &str = "development only";

/// What a run is asked to do: the command line of `check`.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Add the production checks.
    pub deploy: bool,
    /// The environment to load. `None` is `production` with `deploy`, and
    /// `$<APP>_ENV` or `development` without.
    pub env: Option<String>,
    /// Finding ids to silence.
    pub allow: Vec<String>,
}

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The project should not ship like this.
    Error,
    /// Worth a look; `--allow` records that somebody looked.
    Warning,
}

/// One thing that is wrong, where, and what to do.
#[derive(Debug, Clone)]
pub struct Finding {
    /// How bad it is.
    pub severity: Severity,
    /// The rule, such as `deploy/hsts`.
    pub id: &'static str,
    /// What is wrong.
    pub message: String,
    /// `src/file.rs:line`, `config/production.toml:4`, or a variable.
    pub location: Option<String>,
    /// `("note", ..)` and `("help", ..)`, in order.
    pub notes: Vec<(&'static str, String)>,
}

impl Finding {
    fn error(id: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Error, id, message)
    }

    fn warning(id: &'static str, message: impl Into<String>) -> Self {
        Self::new(Severity::Warning, id, message)
    }

    fn new(severity: Severity, id: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity,
            id,
            message: message.into(),
            location: None,
            notes: Vec::new(),
        }
    }

    fn at(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    fn note(mut self, text: impl Into<String>) -> Self {
        self.notes.push(("note", text.into()));
        self
    }

    fn help(mut self, text: impl Into<String>) -> Self {
        self.notes.push(("help", text.into()));
        self
    }
}

/// rustc's shape, which is also the brocanth CLI's.
impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        writeln!(f, "{label}[{}]: {}", self.id, self.message)?;
        if let Some(at) = &self.location {
            writeln!(f, "  --> {at}")?;
        }
        for (kind, text) in &self.notes {
            writeln!(f, "  = {kind}: {text}")?;
        }
        Ok(())
    }
}

/// What one run found.
#[derive(Debug, Clone)]
pub struct Report {
    /// The environment the configuration was loaded for.
    pub env: String,
    /// Whether the production checks ran.
    pub deploy: bool,
    /// What is wrong, after `--allow`.
    pub findings: Vec<Finding>,
}

impl Report {
    /// The findings that are errors.
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    /// The findings that are warnings.
    pub fn warnings(&self) -> usize {
        self.findings.len() - self.errors()
    }

    /// Whether the run fails: an error, or with `strict` a warning too.
    pub fn failed(&self, strict: bool) -> bool {
        self.errors() > 0 || (strict && self.warnings() > 0)
    }
}

/// Every finding, a blank line after each, then one summary line.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for finding in &self.findings {
            writeln!(f, "{finding}")?;
        }
        let what = if self.deploy {
            "deploy checks"
        } else {
            "checks"
        };
        if self.findings.is_empty() {
            writeln!(f, "{what} pass for `{}`", self.env)
        } else {
            writeln!(
                f,
                "{what} for `{}`: {}, {}",
                self.env,
                plural(self.errors(), "error"),
                plural(self.warnings(), "warning")
            )
        }
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Runs the checks on the project at `root`, for the application `app`.
///
/// `root` holds `brocanth.toml`; the configuration is read from there as
/// the application would read it at startup.
pub fn run(app: impl Into<AppName>, root: &Path, options: &Options) -> Result<Report, ConfigError> {
    let project = Project::read(app.into(), root);
    let env = match (&options.env, options.deploy) {
        (Some(env), _) => Some(env.clone()),
        (None, true) => Some("production".to_owned()),
        (None, false) => None,
    };
    let mut loader = Loader::new(project.app.clone()).cwd(root);
    if let Some(env) = env {
        loader = loader.env(env);
    }
    let config = loader.load()?;
    let sources = Sources::read(root);
    let lookup = |name: &str| std::env::var(name).ok();

    let mut findings = always(&project, &config);
    if options.deploy {
        findings.extend(deploy(&project, &config, &sources, &lookup));
    }
    findings.retain(|f| !options.allow.iter().any(|a| a == f.id));
    Ok(Report {
        env: config.env().to_owned(),
        deploy: options.deploy,
        findings,
    })
}

// -- what is read -------------------------------------------------------------

/// The project on disk, as the checks need it.
struct Project {
    root: PathBuf,
    app: AppName,
    /// `[modules] enabled` in `brocanth.toml`, empty when it says nothing.
    modules: Vec<String>,
}

impl Project {
    fn read(app: AppName, root: &Path) -> Self {
        let modules = std::fs::read_to_string(root.join("brocanth.toml"))
            .ok()
            .and_then(|text| Config::from_toml(app.clone(), "marker", &text).ok())
            .and_then(|marker| {
                let enabled = marker.root().at("modules.enabled")?.as_array()?;
                Some(
                    enabled
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect(),
                )
            })
            .unwrap_or_default();
        Self {
            root: root.to_path_buf(),
            app,
            modules,
        }
    }

    fn has(&self, module: &str) -> bool {
        self.modules.iter().any(|m| m == module)
    }

    fn config_file(&self, env: &str) -> String {
        format!("config/{env}.toml")
    }

    fn variable(&self, section: &str, key: &str) -> String {
        self.app.env_var(&format!(
            "{}__{}",
            section.to_ascii_uppercase(),
            key.to_ascii_uppercase()
        ))
    }
}

/// The `.rs` files under `src/`, as text.
struct Sources {
    root: PathBuf,
    files: Vec<(PathBuf, String)>,
}

impl Sources {
    fn read(root: &Path) -> Self {
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

    /// Every line, outside a `//` comment, that contains `needle`, as
    /// `src/file.rs:line`.
    ///
    /// Only whole-line comments are skipped. The scaffold writes its
    /// optional registrations as whole-line comments, and that is the case
    /// that must not count as code.
    fn find(&self, needle: &str) -> Vec<String> {
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
fn always(project: &Project, config: &Config) -> Vec<Finding> {
    let mut out = Vec::new();
    if let (Some(url), Some(dialect)) = (get(config, "db.url"), get(config, "policy.dialect")) {
        let backend = url.as_str().and_then(backend_of);
        let said = dialect.as_str().unwrap_or_default();
        if let Some(backend) = backend {
            if !said.eq_ignore_ascii_case(backend) {
                out.push(
                    Finding::error(
                        "check/dialect",
                        format!(
                            "`[policy] dialect` is `{said}`, and `[db] url` is a {backend} database"
                        ),
                    )
                    .at(place(project, dialect))
                    .note(format!("`[db] url` is set at {}", place(project, url)))
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
    if url.starts_with("postgres://") || url.starts_with("postgresql://") {
        Some("postgres")
    } else {
        None
    }
}

/// The production checks.
fn deploy(
    project: &Project,
    config: &Config,
    sources: &Sources,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let env = config.env().to_owned();
    let file = project.config_file(&env);

    if matches!(env.as_str(), "development" | "test") {
        out.push(
            Finding::warning(
                "deploy/env",
                format!("the deploy checks read the `{env}` configuration"),
            )
            .help("pass --env production, or the name of the environment you deploy"),
        );
    }
    if config.root().as_table().is_none_or(Table::is_empty) {
        out.push(
            Finding::warning(
                "deploy/no-config",
                format!("no file and no variable configures `{env}`"),
            )
            .help(format!(
                "create {file}, or point ${} at the file the deployment reads",
                project.app.env_var_for_config()
            )),
        );
    }

    // Cookies.
    for section in ["sessions", "auth"] {
        if let Some(v) = get(config, &format!("{section}.secure")) {
            if v.as_bool() == Some(false) {
                out.push(
                    Finding::error(
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
                Finding::warning(
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
                    Finding::error("deploy/signing-key", format!("the session signing key: {problem}"))
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
                Finding::error("deploy/default-secret", format!("`[storage] secret` {problem}"))
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
                Finding::warning(
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
                Finding::warning(
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
                Finding::warning(
                    "deploy/jobs-migrate",
                    "`[jobs] migrate` is true, so the application creates tables at startup",
                )
                .at(place(project, v))
                .help("set `migrate = false` and put `Queue::schema_sql` in a migration"),
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
        let mut d = Finding::error(
            "deploy/csrf-off",
            "no CSRF check is registered, and this application uses cookies",
        )
        .note(
            "any page on any site can post a form here, and the browser sends the cookies with it",
        )
        .help("register `brocanth_middleware::Csrf` with `plugin_hooking` in src/plugins/mod.rs");
        if project.has("middleware-csrf") {
            d = d.note(
                "the scaffold registered it in src/plugins/mod.rs; something removed that line",
            );
        }
        out.push(d);
    }
    let hooks = sources.find("plugin_hooking");
    let runner = sources.any(&[
        "Bridge",
        "brocanth_server::serve",
        "bridge::serve",
        "App::run",
        "begin_async(",
    ]);
    if !hooks.is_empty() && runner.is_empty() {
        out.push(
            Finding::error(
                "deploy/hooks-not-run",
                "hooks are registered, and nothing in src/ serves through the hook pipeline",
            )
            .at(hooks[0].clone())
            .note("a request that goes from the route table to the handler skips every hook, CSRF included")
            .help("serve with `brocanth_server::serve` or `brocanth_server::Bridge`, which run the hooks"),
        );
    }

    if let Some(d) = hsts(project, config, sources) {
        out.push(d);
    }

    for at in sources.find("unscoped_rows_unsafe_for_development") {
        out.push(
            Finding::error(
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

/// `deploy/hsts`: whether a response over HTTPS carries
/// `Strict-Transport-Security`.
///
/// The `SecurityHeaders` hook sends it only on a request it knows is
/// HTTPS, so a registration is not enough. It also needs `behind_tls`, or
/// a `ProxyHeaders` hook that passes on a trusted proxy's `proto=https`.
fn hsts(project: &Project, config: &Config, sources: &Sources) -> Option<Finding> {
    let named = sources.find("brocanth_middleware::SecurityHeaders");
    let registered = named.iter().any(|at| {
        sources
            .find("plugin_hooking")
            .iter()
            .any(|h| same_file(h, at))
    }) || !sources.find("SecurityHeaders::new").is_empty()
        || !sources.find("SecurityHeaders>::new").is_empty()
        || !sources
            .find("plugin_hooking_from_config::<brocanth_middleware::SecurityHeaders>")
            .is_empty();

    if !registered {
        if !sources.find("Strict-Transport-Security").is_empty() {
            return None;
        }
        return Some(
            Finding::warning(
                "deploy/hsts",
                "nothing in src/ sets `Strict-Transport-Security`",
            )
            .help("register `brocanth_middleware::SecurityHeaders` with `plugin_hooking`, or set HSTS at the TLS terminator and pass --allow deploy/hsts"),
        );
    }

    let hsts_off = !sources.find("without_hsts").is_empty()
        || get(config, "security_headers.hsts.enabled").and_then(Value::as_bool) == Some(false);
    if hsts_off {
        let mut d = Finding::warning(
            "deploy/hsts",
            "`SecurityHeaders` is registered with HSTS turned off",
        )
        .help("turn HSTS on, or set it at the TLS terminator and pass --allow deploy/hsts");
        if let Some(v) = get(config, "security_headers.hsts.enabled") {
            d = d.at(place(project, v));
        }
        return Some(d);
    }

    let told = get(config, "security_headers.behind_tls").and_then(Value::as_bool) == Some(true)
        || !sources.find(".behind_tls()").is_empty();
    let proxied = !sources.find("ProxyHeaders::new").is_empty()
        || !sources.find("ProxyHeaders>::new").is_empty();
    if told || proxied {
        return None;
    }
    Some(
        Finding::warning(
            "deploy/hsts",
            "`SecurityHeaders` is registered, and nothing tells it a request came over TLS",
        )
        .note("it sends HSTS only on a request it knows is HTTPS, and brocanth-server does not terminate TLS")
        .help("set `behind_tls = true` in `[security_headers]`, or register `ProxyHeaders` so a trusted proxy's `proto=https` reaches it"),
    )
}

fn same_file(a: &str, b: &str) -> bool {
    a.rsplit_once(':').map(|x| x.0) == b.rsplit_once(':').map(|x| x.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(modules: &[&str]) -> Project {
        Project {
            root: PathBuf::from("/p"),
            app: AppName::new("shop"),
            modules: modules.iter().map(|m| (*m).to_owned()).collect(),
        }
    }

    fn sources(files: &[(&str, &str)]) -> Sources {
        Sources {
            root: PathBuf::from("/p"),
            files: files
                .iter()
                .map(|(p, t)| (PathBuf::from("/p").join(p), (*t).to_owned()))
                .collect(),
        }
    }

    fn ids(found: &[Finding]) -> Vec<&str> {
        found.iter().map(|d| d.id).collect()
    }

    const SCAFFOLD: &str = r#"
[logging]
level = "debug"

[sessions]
secure = false

[db]
url = "postgres://localhost/shop"

[jobs]
migrate = true

[auth]
signing_key_env = "SHOP_SESSION_KEY"
cookie_name = "session"
secure = false

[policy]
dialect = "mysql"

[storage]
secret = "development only: replace before anything is uploaded in anger"
"#;

    #[test]
    fn the_scaffold_s_development_config_fails_every_check_it_should() {
        let config = Config::from_toml("shop", "production", SCAFFOLD).unwrap();
        let p = project(&["sessions", "auth", "middleware-csrf"]);
        let src = sources(&[("src/plugins/mod.rs", "        // let builder = builder.plugin_hooking(\n        //     <brocanth_middleware::Csrf as Plugin>::new(\n")]);
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
            assert!(d.notes.iter().any(|(k, _)| *k == "help"), "{d} has no help");
        }
    }

    #[test]
    fn a_production_config_passes() {
        let text = r#"
[logging]
level = "info"
[sessions]
secure = true
[db]
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
        let src = sources(&[
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
    fn the_security_headers_hook_answers_hsts_only_when_it_can_see_tls() {
        let hsts = |config: &str, files: &[(&str, &str)]| {
            let config = Config::from_toml("shop", "production", config).unwrap();
            let found = deploy(&project(&[]), &config, &sources(files), &|_| None);
            found
                .into_iter()
                .find(|d| d.id == "deploy/hsts")
                .map(|d| d.to_string())
        };
        let registered = "let builder = builder.plugin_hooking(\n    \
             <brocanth_middleware::SecurityHeaders as Plugin>::new(c),\n);";

        // Commented out, as the scaffold writes it, is not registered.
        let commented =
            "// let b = b.plugin_hooking(\n//   brocanth_middleware::SecurityHeaders::new(c));";
        let d = hsts("", &[("src/app.rs", commented)]).expect("a finding");
        assert!(d.contains("SecurityHeaders"), "{d}");

        // Registered, and nothing says the request was HTTPS.
        let d = hsts("", &[("src/app.rs", registered)]).expect("a finding");
        assert!(d.contains("over TLS") && d.contains("behind_tls"), "{d}");

        // Registered from the config file, as the scaffold now writes it.
        let from_config = "let builder = builder\n    \
             .plugin_hooking_from_config::<brocanth_middleware::SecurityHeaders>(\"security_headers\")";
        let d = hsts("", &[("src/plugins/mod.rs", from_config)]).expect("a finding");
        assert!(d.contains("over TLS"), "{d}");

        // Told by the configuration, by the builder, or by a proxy hook.
        assert_eq!(
            hsts(
                "[security_headers]\nbehind_tls = true\n",
                &[("src/app.rs", registered)]
            ),
            None
        );
        let built = "builder.plugin_hooking(SecurityHeaders::new(\
                     SecurityHeadersConfig::default().behind_tls()))";
        assert_eq!(hsts("", &[("src/app.rs", built)]), None);
        let proxied = format!("{registered}\nbuilder.plugin_hooking(ProxyHeaders::new(p));");
        assert_eq!(hsts("", &[("src/app.rs", proxied.as_str())]), None);

        // Registered with HSTS off is still a finding, at the line.
        let d = hsts(
            "[security_headers]\nbehind_tls = true\n[security_headers.hsts]\nenabled = false\n",
            &[("src/app.rs", registered)],
        )
        .expect("a finding");
        assert!(d.contains("HSTS turned off"), "{d}");
    }

    #[test]
    fn hooks_with_no_runner_and_the_unscoped_admin_are_located() {
        let config = Config::from_toml("shop", "production", "").unwrap();
        let src = sources(&[
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
            .find(|d| d.id == "deploy/hooks-not-run")
            .unwrap();
        assert_eq!(hooks.location.as_deref(), Some("src/plugins/mod.rs:2"));
        let admin = found
            .iter()
            .find(|d| d.id == "deploy/admin-unscoped")
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
        let found = deploy(&project(&[]), &config, &sources(&[]), &|_| None);
        let d = found
            .iter()
            .find(|d| d.id == "deploy/secret-in-file")
            .expect("named like a secret");
        let said = d.to_string();
        assert!(said.contains("$SHOP_MAIL__SMTP_PASSWORD"), "{said}");
        assert!(
            !said.contains("hunter2"),
            "a finding never prints the secret"
        );
    }

    #[test]
    fn a_dialect_that_is_not_the_database_is_an_error() {
        let config = Config::from_toml(
            "shop",
            "development",
            "[db]\nurl = \"postgres://localhost/shop\"\n\n[policy]\ndialect = \"mysql\"\n",
        )
        .unwrap();
        let found = always(&project(&["db", "policy"]), &config);
        assert_eq!(ids(&found), ["check/dialect"]);
        let config = Config::from_toml(
            "shop",
            "development",
            "[db]\nurl = \"postgres://localhost/shop\"\n\n[policy]\ndialect = \"postgres\"\n",
        )
        .unwrap();
        assert!(always(&project(&["db", "policy"]), &config).is_empty());
    }

    #[test]
    fn a_run_reads_the_marker_the_files_and_the_allow_list() {
        let root = std::env::temp_dir().join(format!("brocanth-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("brocanth.toml"),
            "[project]\nname = \"shop\"\n\n[modules]\nenabled = [\"config\", \"sessions\"]\n",
        )
        .unwrap();
        std::fs::write(
            root.join("config/production.toml"),
            "[sessions]\nsecure = false\n",
        )
        .unwrap();
        let options = Options {
            deploy: true,
            env: None,
            allow: vec!["deploy/hsts".to_owned()],
        };
        let report = run("shop", &root, &options).unwrap();
        assert_eq!(report.env, "production");
        let ids = ids(&report.findings);
        assert!(ids.contains(&"deploy/cookie-insecure"), "{ids:?}");
        assert!(
            ids.contains(&"deploy/csrf-off"),
            "sessions means cookies: {ids:?}"
        );
        assert!(!ids.contains(&"deploy/hsts"), "allowed: {ids:?}");
        let insecure = report
            .findings
            .iter()
            .find(|f| f.id == "deploy/cookie-insecure")
            .unwrap();
        assert_eq!(
            insecure.location.as_deref(),
            Some("config/production.toml:2")
        );
        assert!(report.failed(false));
        assert!(report
            .to_string()
            .contains("deploy checks for `production`"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
