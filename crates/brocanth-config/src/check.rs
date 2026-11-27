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
//! | `check/bindings-missing` | warning | `brocanth.toml` lists modules and has no `[bindings]` table, or a table older than a Kind a check reads; the checks read module names instead |
//! | `check/db-url` | error | `[db] url` is not a `postgres://` or `postgresql://` URL |
//! | `deploy/env` | warning | `--deploy` on `development` or `test` |
//! | `deploy/no-config` | warning | no file and no variable configures this environment |
//! | `deploy/cookie-insecure` | error | `[sessions] secure` or `[auth] secure` is `false` |
//! | `deploy/cookie-prefix` | warning | `[auth] cookie_name` has no `__Host-` prefix |
//! | `deploy/signing-key` | error | `[auth] signing_key_env` names a variable that is unset or short |
//! | `deploy/default-secret` | error | `[storage] secret` is the scaffold's development string, or short |
//! | `deploy/secret-in-file` | warning | a key named like a secret has its value in a file |
//! | `deploy/debug-logging` | warning | `[logging] level` is `debug` or `trace` |
//! | `deploy/migrate` | warning | a module's `migrate` is true, or unset where the module contributed the check (it defaults to true) |
//! | `deploy/metrics-public` | warning | `/metrics` is on the main listener with no token |
//! | `deploy/drain-delay` | warning | `/readyz` is on and `drain_delay_secs` is 0 |
//! | `deploy/storage-local` | warning | the object store is `storage-fs` (or, before the Kind, `storage` with no `db`), so its files are on one machine's disk |
//! | `deploy/storage-redirect-http` | warning | `[storage_routes] downloads` is `redirect` and `[storage] endpoint` is `http://`, so the signed URL travels in clear text |
//! | `deploy/storage-signed-ttl` | warning | `[storage_routes] downloads` is `redirect` and `signed_ttl` is above 3600 seconds |
//! | `deploy/storage-sibling-domain` | warning | `[storage_routes] downloads` is `redirect` and `[storage] endpoint` shares its registrable domain with the site's own address (`[meta] base_url`, `[sitemap] base_url` or `[feed] site_url`) |
//! | `deploy/csrf-off` | error | `[bindings]` binds `SessionCookie` and no `Csrf` hook is registered |
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

/// A check a module contributes to `check --deploy` (the `ConfigCheck`
/// Kind). The generator writes one per contributing module into
/// `config_checks` in the project's `src/commands/mod.rs`, and the
/// project's `check` command passes them to [`run_with`]. So the list is the
/// code the binary is built from, and a module that adds one edits no list
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    what: Contributed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Contributed {
    /// `deploy/migrate`: `[section] migrate` defaults to true.
    Migrates {
        section: &'static str,
        schema: &'static str,
    },
}

impl Entry {
    /// `deploy/migrate` for a module whose `[section] migrate` defaults to
    /// true in this project. `schema` names the statements to put in a
    /// migration instead: `` "`Queue::schema_sql`" ``.
    ///
    /// The module contributes it only when its default creates tables: the
    /// queue in memory (`jobs-memory`) has none, so it contributes nothing.
    pub fn migrates(section: &'static str, schema: &'static str) -> Self {
        Self {
            what: Contributed::Migrates { section, schema },
        }
    }
}

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
    run_with(app, root, options, &[])
}

/// [`run`], with the checks the project's modules contribute: the generated
/// `config_checks()` in `src/commands/mod.rs`.
pub fn run_with(
    app: impl Into<AppName>,
    root: &Path,
    options: &Options,
    checks: &[Entry],
) -> Result<Report, ConfigError> {
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
        findings.extend(deploy(&project, &config, &sources, &lookup, checks));
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
    /// The Kinds `[bindings]` in `brocanth.toml` binds to a provider:
    /// `PgPool` when the project has its one shared pool, `SessionCookie`
    /// when it has a session cookie. `None` when the file has no
    /// `[bindings]` table: a project written before the generator recorded
    /// them, read through [`BEFORE_BINDINGS`].
    bound: Option<Vec<String>>,
}

/// The modules that provided each Kind a check reads, before the generator
/// recorded the provider in `[bindings]`.
///
/// Read for a `brocanth.toml` with no `[bindings]` table, and for a table
/// written before a Kind existed (stage 1 recorded `PgPool` only), so
/// such a project does not lose `deploy/csrf-off` in silence.
/// `check/bindings-missing` says how to record the bindings. A table the
/// current generator wrote binds the Kind whenever one of these modules is
/// chosen, so for it this list changes nothing.
const BEFORE_BINDINGS: &[(&str, &[&str])] = &[
    ("PgPool", &["db"]),
    ("SessionCookie", &["sessions", "auth"]),
];

impl Project {
    fn read(app: AppName, root: &Path) -> Self {
        let marker = std::fs::read_to_string(root.join("brocanth.toml"))
            .ok()
            .and_then(|text| Config::from_toml(app.clone(), "marker", &text).ok());
        let modules = marker
            .as_ref()
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
        let bound = marker.as_ref().and_then(|marker| {
            let bindings = marker.root().at("bindings")?.as_table()?;
            Some(bindings.keys().map(|k| k.to_owned()).collect())
        });
        Self {
            root: root.to_path_buf(),
            app,
            modules,
            bound,
        }
    }

    fn has(&self, module: &str) -> bool {
        self.modules.iter().any(|m| m == module)
    }

    /// Whether `[bindings]` binds the Kind `kind`: the generator's record of
    /// which module provides it, so a check names the thing and not the
    /// module that gives it.
    ///
    /// When the table is missing or does not record `kind`, a module that
    /// provided `kind` before the bindings were recorded answers too
    /// ([`BEFORE_BINDINGS`]).
    fn binds(&self, kind: &str) -> bool {
        self.recorded(kind) || self.provided_before_bindings(kind)
    }

    fn recorded(&self, kind: &str) -> bool {
        self.bound
            .as_ref()
            .is_some_and(|bound| bound.iter().any(|k| k == kind))
    }

    fn provided_before_bindings(&self, kind: &str) -> bool {
        BEFORE_BINDINGS
            .iter()
            .filter(|(k, _)| *k == kind)
            .flat_map(|(_, keys)| keys.iter())
            .any(|key| self.has(key))
    }

    /// The Kinds a check reads that a chosen module provides and the table
    /// does not record: the table is missing, or older than the Kind.
    fn unrecorded(&self) -> Vec<&'static str> {
        BEFORE_BINDINGS
            .iter()
            .map(|(kind, _)| *kind)
            .filter(|kind| self.provided_before_bindings(kind) && !self.recorded(kind))
            .collect()
    }

    /// Whether a file under `deploy/` names the variable `var`: the
    /// generated Kubernetes manifest sets some of them.
    fn deploy_sets(&self, var: &str) -> bool {
        let Ok(entries) = std::fs::read_dir(self.root.join("deploy")) else {
            return false;
        };
        entries
            .filter_map(Result::ok)
            .any(|entry| std::fs::read_to_string(entry.path()).is_ok_and(|text| text.contains(var)))
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
    // Postgres only (PLAN.md ADR 5): any other URL is a database brocanth
    // does not speak to.
    if let Some(url) = get(config, "db.url") {
        if let Some(text) = url.as_str().filter(|u| !u.is_empty() && !is_postgres(u)) {
            let scheme = text.split_once("://").map_or(text, |(s, _)| s);
            out.push(
                Finding::error(
                    "check/db-url",
                    format!("`[db] url` is a `{scheme}` URL, and brocanth runs on Postgres only"),
                )
                .at(place(project, url))
                .help(format!(
                    "set `url` under `[db]` to a `postgres://` URL, or ${}",
                    project.variable("db", "url")
                )),
            );
        }
    }
    // A marker with no `[bindings]` table, or one older than a Kind a
    // check reads: written before the generator recorded them. The checks
    // fall back to module names, and this says how to stop that.
    let unrecorded = project.unrecorded();
    let missing = project.bound.is_none() && !project.modules.is_empty();
    if missing || !unrecorded.is_empty() {
        let what = if project.bound.is_none() {
            "brocanth.toml has no `[bindings]` table, so the checks fall back to module names"
                .to_owned()
        } else {
            format!(
                "`[bindings]` in brocanth.toml does not record {}, so the checks fall back to module names",
                unrecorded.join(", ")
            )
        };
        out.push(
            Finding::warning("check/bindings-missing", what)
            .at("brocanth.toml")
            .note("the generator records which module provides each Kind there, and the checks read it")
            .help("run `brocanth add` or `brocanth remove` once with this CLI: it rewrites brocanth.toml with `[bindings]`"),
        );
    }
    out
}

fn is_postgres(url: &str) -> bool {
    url.starts_with("postgres://") || url.starts_with("postgresql://")
}

/// The production checks.
fn deploy(
    project: &Project,
    config: &Config,
    sources: &Sources,
    lookup: &dyn Fn(&str) -> Option<String>,
    checks: &[Entry],
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
    out.extend(migrate(project, &file, config, checks));
    out.extend(observability(project, config, &file));
    // `storage-fs` is the object store, or a project from before the
    // `ObjectStore` Kind has `storage` with no pool to keep the files in.
    let on_disk = project.has("storage-fs")
        || (project.has("storage")
            && !project.recorded("ObjectStore")
            && !["storage-db", "storage-s3", "storage-gcs"]
                .iter()
                .any(|k| project.has(k))
            && !project.binds("PgPool"));
    if on_disk {
        let mut d = Finding::warning(
            "deploy/storage-local",
            "file storage is a directory on this machine's disk",
        )
        .note("a second instance does not see the files the first one stored, and a new machine has none")
        .help("`brocanth add storage-s3` (or `storage-gcs`, `storage-db`) so every instance sees the files, or run one instance on a disk that outlives it");
        if let Some(v) = get(config, "storage.root") {
            d = d.at(place(project, v));
        }
        out.push(d);
    }
    out.extend(storage_redirect(project, config, &file));
    // CSRF, and whether hooks run at all.
    // A marker with no module list says nothing, so assume cookies.
    let has_cookies = project.modules.is_empty() || project.binds("SessionCookie");
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

/// `deploy/migrate`: a module that creates its tables at startup.
///
/// Set in a file, or unset where the module contributed the check because
/// its setting defaults to true.
fn migrate(project: &Project, file: &str, config: &Config, checks: &[Entry]) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for entry in checks {
        let Contributed::Migrates { section, schema } = entry.what;
        if seen.contains(&section) {
            continue;
        }
        seen.push(section);
        let value = get(config, &format!("{section}.migrate"));
        if value.is_none_or(|v| v.as_bool() == Some(true)) {
            out.push(migrate_finding(project, section, schema, value, file));
        }
    }
    // A section nobody contributed, whose file still says `migrate = true`.
    for (path, value) in config.root().leaves() {
        let Some(section) = path.strip_suffix(".migrate") else {
            continue;
        };
        if section.contains('.') || seen.contains(&section) || value.as_bool() != Some(true) {
            continue;
        }
        out.push(migrate_finding(
            project,
            section,
            "the module's schema statements",
            Some(value),
            file,
        ));
    }
    out
}

fn migrate_finding(
    project: &Project,
    section: &str,
    schema: &str,
    value: Option<&Value>,
    file: &str,
) -> Finding {
    let said = if value.is_some() { "is" } else { "defaults to" };
    let d = Finding::warning(
        "deploy/migrate",
        format!("`[{section}] migrate` {said} true, so the application creates tables at startup"),
    )
    .help(format!(
        "set `migrate = false` under `[{section}]` in {file} (or ${}=false) and put {schema} in a migration",
        project.variable(section, "migrate")
    ));
    match value {
        Some(v) => d.at(place(project, v)),
        None => d,
    }
}

/// `deploy/metrics-public` and `deploy/drain-delay`: the `[observability]`
/// settings that are fine on a laptop and wrong behind a load balancer.
fn observability(project: &Project, config: &Config, file: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let text = |key: &str, default: &str| {
        get(config, &format!("observability.{key}")).map_or_else(
            || default.to_owned(),
            |v| v.as_str().unwrap_or_default().trim().to_owned(),
        )
    };
    if project.has("observability-metrics") || get(config, "observability.metrics_path").is_some() {
        let path = text("metrics_path", "/metrics");
        if !path.is_empty()
            && text("metrics_token", "").is_empty()
            && text("metrics_bind", "").is_empty()
        {
            out.push(
                Finding::warning(
                    "deploy/metrics-public",
                    format!("`{path}` is served on the main listener to anybody who asks"),
                )
                .note("each scrape runs every refresher, and the series name routes and queues")
                .help(format!(
                    "set ${} to a long random string, or `metrics_bind` under `[observability]` in {file} to a port the load balancer does not expose",
                    project.variable("observability", "metrics_token")
                )),
            );
        }
    }
    if project.has("observability-health") || get(config, "observability.ready_path").is_some() {
        let ready = text("ready_path", "/readyz");
        let variable = project.variable("observability", "drain_delay_secs");
        let delay = get(config, "observability.drain_delay_secs");
        let zero = delay.is_none_or(|v| v.as_integer() == Some(0));
        if !ready.is_empty() && zero && !project.deploy_sets(&variable) {
            let mut d = Finding::warning(
                "deploy/drain-delay",
                "`[observability] drain_delay_secs` is 0, so a stopping pod closes its listener while the load balancer still sends to it",
            )
            .help(format!(
                "set ${variable}=5, or `drain_delay_secs = 5` under `[observability]` in {file}"
            ));
            if let Some(v) = delay {
                d = d.at(place(project, v));
            }
            out.push(d);
        }
    }
    out
}

/// The longest `signed_ttl` a redirect may have before `check --deploy`
/// says so: an hour.
const REDIRECT_TTL_WARNING: i64 = 3600;

/// `deploy/storage-redirect-http`, `deploy/storage-signed-ttl` and
/// `deploy/storage-sibling-domain`: a download route that answers with a
/// signed URL of the bucket (`docs/object-storage.md` §9.3, §9.4). The URL
/// is a bearer credential nobody can revoke.
fn storage_redirect(project: &Project, config: &Config, file: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let Some(downloads) = get(config, "storage_routes.downloads") else {
        return out;
    };
    if downloads.as_str().map(str::trim) != Some("redirect") {
        return out;
    }
    let endpoint = get(config, "storage.endpoint")
        .and_then(|v| Some((v, v.as_str()?.trim())))
        .filter(|(_, url)| !url.is_empty());
    if let Some((v, url)) = endpoint {
        if url.starts_with("http://") {
            out.push(
                Finding::warning(
                    "deploy/storage-redirect-http",
                    "`[storage_routes] downloads` is `redirect` and `[storage] endpoint` is plain HTTP",
                )
                .at(place(project, v))
                .note("the signed URL is a bearer credential, and it travels in clear text to the browser and the bucket")
                .help(format!(
                    "set `endpoint` under `[storage]` in {file} to the server's https:// URL, or `downloads = \"proxy\"` under `[storage_routes]`"
                )),
            );
        }
    }
    if let Some(v) = get(config, "storage_routes.signed_ttl") {
        if let Some(ttl) = v.as_integer().filter(|ttl| *ttl > REDIRECT_TTL_WARNING) {
            out.push(
                Finding::warning(
                    "deploy/storage-signed-ttl",
                    format!("`[storage_routes] signed_ttl` is {ttl} seconds"),
                )
                .at(place(project, v))
                .note("a signed URL cannot be revoked: whoever holds a copy reads the file until it expires")
                .help(format!(
                    "set `signed_ttl` under `[storage_routes]` in {file} to at most {REDIRECT_TTL_WARNING}, such as 300"
                )),
            );
        }
    }
    let site = ["meta.base_url", "sitemap.base_url", "feed.site_url"]
        .iter()
        .filter_map(|path| get(config, path)?.as_str().and_then(url_host))
        .next();
    if let (Some((v, url)), Some(site)) = (endpoint, site) {
        if let Some(host) = url_host(url) {
            let (bucket, own) = (registrable(&host), registrable(&site));
            if bucket.is_some() && bucket == own {
                let domain = own.unwrap_or_default();
                out.push(
                    Finding::warning(
                        "deploy/storage-sibling-domain",
                        format!("`[storage] endpoint` {host} is on {domain}, the site's own domain"),
                    )
                    .at(place(project, v))
                    .note("a redirect sends the browser there; a page rendered on a sibling host can set cookies for the whole domain")
                    .note("every non-image goes as an attachment, so nothing renders by design; keep the bucket on a domain of its own all the same")
                    .help(format!(
                        "serve the bucket from a domain the site does not share, such as the provider's own host, or set `downloads = \"proxy\"` under `[storage_routes]` in {file}"
                    )),
                );
            }
        }
    }
    out
}

/// The host of an `http://` or `https://` URL, lowercased, without a port.
fn url_host(url: &str) -> Option<String> {
    let rest = url
        .trim()
        .strip_prefix("https://")
        .or_else(|| url.trim().strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?.trim_end_matches('.');
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// The registrable domain of a host, by a rule short of the public suffix
/// list: the last two labels, or three when the second-last is a common
/// second-level label under a country code (`example.co.uk`). `None` for
/// an address, `localhost` or a single label. A miss under a suffix the
/// rule does not know is a warning too many, never one too few for a
/// shared `example.com`.
fn registrable(host: &str) -> Option<String> {
    if host.parse::<std::net::IpAddr>().is_ok() || host.starts_with('[') {
        return None;
    }
    let labels: Vec<&str> = host.split('.').filter(|l| !l.is_empty()).collect();
    if labels.len() < 2 {
        return None;
    }
    let n = labels.len();
    let country = labels[n - 1].len() == 2;
    let second = ["co", "com", "net", "org", "ac", "gov", "edu"].contains(&labels[n - 2]);
    let take = if country && second && n >= 3 { 3 } else { 2 };
    Some(labels[n - take..].join("."))
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
            // The generator binds the pool exactly when `db` is chosen,
            // and the session cookie when `sessions` or `auth` is.
            bound: Some(
                [
                    ("PgPool", modules.contains(&"db")),
                    (
                        "SessionCookie",
                        modules.contains(&"sessions") || modules.contains(&"auth"),
                    ),
                ]
                .into_iter()
                .filter(|(_, bound)| *bound)
                .map(|(kind, _)| kind.to_owned())
                .collect(),
            ),
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

[storage]
secret = "development only: replace before anything is uploaded in anger"
"#;

    #[test]
    fn a_redirecting_route_warns_about_clear_text_a_long_ttl_and_a_sibling_domain() {
        let src = sources(&[]);
        let picks = project(&["storage", "storage-s3", "storage-routes"]);
        let run = |toml: &str| {
            let config = Config::from_toml("shop", "production", toml).unwrap();
            ids(&deploy(&picks, &config, &src, &|_| None, &[]))
                .into_iter()
                .filter(|id| id.starts_with("deploy/storage-"))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        let all = r#"
[storage]
endpoint = "http://files.shop.example.co.uk"

[storage_routes]
downloads = "redirect"
signed_ttl = 7200

[meta]
base_url = "https://www.shop.example.co.uk/"
"#;
        assert_eq!(
            run(all),
            [
                "deploy/storage-redirect-http",
                "deploy/storage-signed-ttl",
                "deploy/storage-sibling-domain"
            ]
        );
        // Proxy: none of them. The URL never leaves the process.
        assert!(run(&all.replace("\"redirect\"", "\"proxy\"")).is_empty());
        // HTTPS on the provider's host, an hour, and another domain: none.
        let fine = r#"
[storage]
endpoint = "https://acct.r2.cloudflarestorage.com"

[storage_routes]
downloads = "redirect"
signed_ttl = 3600

[sitemap]
base_url = "https://shop.example"
"#;
        assert!(run(fine).is_empty(), "{:?}", run(fine));
        // No endpoint (AWS) and no site address: nothing to compare.
        assert!(run("[storage_routes]\ndownloads = \"redirect\"\n").is_empty());
    }

    #[test]
    fn a_registrable_domain_is_the_last_two_labels_or_three_under_a_country() {
        assert_eq!(
            registrable("files.example.com").as_deref(),
            Some("example.com")
        );
        assert_eq!(
            registrable("a.b.example.co.uk").as_deref(),
            Some("example.co.uk")
        );
        assert_eq!(registrable("example.de").as_deref(), Some("example.de"));
        assert_eq!(registrable("localhost"), None);
        assert_eq!(registrable("127.0.0.1"), None);
        assert_eq!(
            url_host("https://U:P@Files.Example.com:9000/x").as_deref(),
            Some("files.example.com")
        );
        assert_eq!(url_host("ftp://x"), None);
    }

    #[test]
    fn storage_on_a_local_disk_is_a_warning_and_storage_on_db_is_not() {
        let config = Config::from_toml("shop", "production", "").unwrap();
        let src = sources(&[]);
        let local = deploy(&project(&["storage"]), &config, &src, &|_| None, &[]);
        assert!(ids(&local).contains(&"deploy/storage-local"));
        let db = deploy(&project(&["storage", "db"]), &config, &src, &|_| None, &[]);
        assert!(!ids(&db).contains(&"deploy/storage-local"));
        // With the `ObjectStore` Kind the backend is a module of its own,
        // and only the directory is one machine's.
        for (picks, local) in [
            (&["storage", "storage-fs"][..], true),
            (&["storage", "storage-fs", "db"][..], true),
            (&["storage", "storage-s3"][..], false),
            (&["storage", "storage-gcs"][..], false),
            (&["storage", "storage-db", "db"][..], false),
        ] {
            let found = deploy(&project(picks), &config, &src, &|_| None, &[]);
            assert_eq!(
                ids(&found).contains(&"deploy/storage-local"),
                local,
                "{picks:?}"
            );
        }
    }

    #[test]
    fn the_scaffold_s_development_config_fails_every_check_it_should() {
        let config = Config::from_toml("shop", "production", SCAFFOLD).unwrap();
        let p = project(&["sessions", "auth", "middleware-csrf"]);
        let src = sources(&[("src/plugins/mod.rs", "        // let builder = builder.plugin_hooking(\n        //     <brocanth_middleware::Csrf as Plugin>::new(\n")]);
        let mut found = always(&p, &config);
        found.extend(deploy(&p, &config, &src, &|_| None, &[]));
        let ids = ids(&found);
        for id in [
            "deploy/cookie-insecure",
            "deploy/cookie-prefix",
            "deploy/signing-key",
            "deploy/default-secret",
            "deploy/debug-logging",
            "deploy/migrate",
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
[flags]
migrate = false
[observability]
metrics_bind = "127.0.0.1:9100"
drain_delay_secs = 5
"#;
        let config = Config::from_toml("shop", "production", text).unwrap();
        let p = project(&[
            "auth",
            "flags",
            "observability-metrics",
            "observability-health",
        ]);
        let src = sources(&[
            ("src/plugins/mod.rs", "let builder = builder.plugin_hooking(\n <brocanth_middleware::Csrf as Plugin>::new(c));"),
            ("src/app.rs", "brocanth_server::serve(addr, app, router)"),
            ("src/hsts.rs", "\"Strict-Transport-Security\""),
        ]);
        let key = "k".repeat(43);
        let mut found = always(&p, &config);
        let checks = [Entry::migrates(
            "flags",
            "`brocanth_flags::PgStore::SCHEMA`",
        )];
        found.extend(deploy(&p, &config, &src, &|_| Some(key.clone()), &checks));
        assert!(found.is_empty(), "{:#?}", ids(&found));
    }

    #[test]
    fn the_security_headers_hook_answers_hsts_only_when_it_can_see_tls() {
        let hsts = |config: &str, files: &[(&str, &str)]| {
            let config = Config::from_toml("shop", "production", config).unwrap();
            let found = deploy(&project(&[]), &config, &sources(files), &|_| None, &[]);
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
        let found = deploy(&project(&["auth"]), &config, &src, &|_| None, &[]);
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
        let found = deploy(&project(&[]), &config, &sources(&[]), &|_| None, &[]);
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
    fn a_database_url_that_is_not_postgres_is_an_error() {
        let config = Config::from_toml(
            "shop",
            "development",
            "[db]\nurl = \"mysql://localhost/shop\"\n",
        )
        .unwrap();
        let found = always(&project(&["db"]), &config);
        assert_eq!(ids(&found), ["check/db-url"]);
        assert!(found[0].to_string().contains("help: set `url`"));
        for url in ["postgres://localhost/shop", "postgresql://db/shop"] {
            let text = format!("[db]\nurl = \"{url}\"\n");
            let config = Config::from_toml("shop", "development", &text).unwrap();
            assert!(always(&project(&["db"]), &config).is_empty(), "{url}");
        }
    }

    #[test]
    fn a_module_that_migrates_by_default_is_a_warning_with_its_schema() {
        let flags = [Entry::migrates(
            "flags",
            "`brocanth_flags::PgStore::SCHEMA`",
        )];
        let deploy_ids = |checks: &[Entry], text: &str| {
            let config = Config::from_toml("shop", "production", text).unwrap();
            deploy(&project(&[]), &config, &sources(&[]), &|_| None, checks)
                .into_iter()
                .filter(|d| d.id == "deploy/migrate")
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
        };
        // No file: the flags module contributed the check, because its
        // default is true.
        let found = deploy_ids(&flags, "");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("`[flags] migrate` defaults to true"),
            "{}",
            found[0]
        );
        assert!(
            found[0].contains("brocanth_flags::PgStore::SCHEMA"),
            "{}",
            found[0]
        );
        assert!(
            found[0].contains("$SHOP_FLAGS__MIGRATE=false"),
            "{}",
            found[0]
        );
        // Turned off.
        assert!(deploy_ids(&flags, "[flags]\nmigrate = false\n").is_empty());
        // A module that contributed nothing (jobs in memory has no
        // tables) is not reported.
        assert!(deploy_ids(&[], "").is_empty());
        // Set in the file, and contributed too: reported once, at the line.
        let found = deploy_ids(&flags, "[flags]\nmigrate = true\n");
        assert_eq!(found.len(), 1, "{found:?}");
        // A section nobody contributed, set in the file.
        let found = deploy_ids(&[], "[ledger]\nmigrate = true\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(
            found[0].contains("`[ledger] migrate` is true"),
            "{}",
            found[0]
        );
    }

    #[test]
    fn public_metrics_and_no_drain_delay_are_warnings() {
        let found = |text: &str| {
            let config = Config::from_toml("shop", "production", text).unwrap();
            let p = project(&["observability-metrics", "observability-health"]);
            ids(&deploy(&p, &config, &sources(&[]), &|_| None, &[]))
                .into_iter()
                .filter(|i| i.starts_with("deploy/metrics") || i.starts_with("deploy/drain"))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };
        assert_eq!(found(""), ["deploy/metrics-public", "deploy/drain-delay"]);
        assert!(found(
            "[observability]\nmetrics_bind = \"127.0.0.1:9100\"\ndrain_delay_secs = 5\n"
        )
        .is_empty());
        assert!(found("[observability]\nmetrics_path = \"\"\nready_path = \"\"\n").is_empty());
    }

    #[test]
    fn a_marker_with_no_bindings_falls_back_to_module_names_and_says_so() {
        let config = Config::from_toml("shop", "production", "").unwrap();
        let src = sources(&[]);
        let mut old = project(&["config", "sessions"]);
        old.bound = None;
        let found = always(&old, &config);
        assert_eq!(ids(&found), ["check/bindings-missing"]);
        let said = found[0].to_string();
        assert!(
            said.contains("help: run `brocanth add` or `brocanth remove`"),
            "{said}"
        );
        let found = deploy(&old, &config, &src, &|_| None, &[]);
        assert!(
            ids(&found).contains(&"deploy/csrf-off"),
            "{:?}",
            ids(&found)
        );
        // Storage without `db`, read from the module names too.
        let mut old = project(&["storage", "db"]);
        old.bound = None;
        let found = deploy(&old, &config, &src, &|_| None, &[]);
        assert!(!ids(&found).contains(&"deploy/storage-local"));

        // A table from stage 1, before `SessionCookie`: the same fallback.
        let mut stage1 = project(&["config", "db", "sessions"]);
        stage1.bound = Some(vec!["PgPool".to_owned()]);
        let said = always(&stage1, &config)[0].to_string();
        assert!(said.contains("does not record SessionCookie"), "{said}");
        let found = deploy(&stage1, &config, &src, &|_| None, &[]);
        assert!(
            ids(&found).contains(&"deploy/csrf-off"),
            "{:?}",
            ids(&found)
        );

        // A table that records the Kind: the binding answers, whichever
        // module provides it, and nothing is said.
        let mut new = project(&["config", "cookie-module"]);
        new.bound = Some(vec!["SessionCookie".to_owned()]);
        assert!(always(&new, &config).is_empty());
        let found = deploy(&new, &config, &src, &|_| None, &[]);
        assert!(ids(&found).contains(&"deploy/csrf-off"));
        new.bound = Some(Vec::new());
        assert!(always(&new, &config).is_empty());
        let found = deploy(&new, &config, &src, &|_| None, &[]);
        assert!(
            !ids(&found).contains(&"deploy/csrf-off"),
            "{:?}",
            ids(&found)
        );
    }

    #[test]
    fn a_run_reads_the_marker_with_and_without_bindings() {
        let root = std::env::temp_dir().join(format!("brocanth-check-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        let options = Options {
            deploy: true,
            env: None,
            allow: Vec::new(),
        };
        let run_ids = |marker: &str| {
            std::fs::write(root.join("brocanth.toml"), marker).unwrap();
            let report = run("shop", &root, &options).unwrap();
            report.findings.iter().map(|f| f.id).collect::<Vec<_>>()
        };
        let modules = "[project]\nname = \"shop\"\n\n[modules]\nenabled = [\"config\", \"auth\"]\n";
        // Old style: no table. The CSRF check still runs, from `auth`.
        let old = run_ids(modules);
        assert!(old.contains(&"deploy/csrf-off"), "{old:?}");
        assert!(old.contains(&"check/bindings-missing"), "{old:?}");
        // New style: the table records the session cookie.
        let bound = run_ids(&format!(
            "{modules}\n[bindings]\nSessionCookie = {{ provider = \"auth\", why = \"chosen\" }}\n"
        ));
        assert!(bound.contains(&"deploy/csrf-off"), "{bound:?}");
        assert!(!bound.contains(&"check/bindings-missing"), "{bound:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_run_reads_the_marker_the_files_and_the_allow_list() {
        let root = std::env::temp_dir().join(format!("brocanth-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("brocanth.toml"),
            "[project]\nname = \"shop\"\n\n[modules]\nenabled = [\"config\", \"sessions\"]\n\n\
             [bindings]\nSessionCookie = { provider = \"sessions\", why = \"chosen\" }\n",
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
            "a bound session cookie means cookies: {ids:?}"
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
