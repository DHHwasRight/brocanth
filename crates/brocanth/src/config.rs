//! Configuration: find it, layer it, and hand typed sections to plugins.
//!
//! Two decisions from the Crystal original survive the port, and one bug does
//! not.
//!
//! Survives: configuration is *enriched by plugins*, not pre-declared by the
//! framework. [`Config`] keeps every section as raw TOML and knows the name of
//! none of them. A plugin declares a `Deserialize` struct and the framework
//! routes the table whose name matches the plugin instance into it. So
//! `[logger] level = "warn"` becomes a `LoggerConfig` without this file ever
//! mentioning a logger.
//!
//! Survives also: a section that nothing claims is an **error**. The original
//! tolerated unmapped entries; Hugo tolerates them too, and a misspelt key
//! there builds clean, logs nothing at any level, and silently drops the
//! feature. Spelling mistakes are the common case, so the error names the
//! section and suggests the nearest claimed one.
//!
//! Does not survive: the original computed two prefixes for one application —
//! `MYAPP_ENV` from the PascalCase class name and `MY_APP_CONFIG` from the
//! snake_case one — while its README documented a third. [`AppName`] states the
//! convention once: every variable is `<SNAKE_UPPER>_<SUFFIX>`, whatever
//! spelling the application was named with.
//!
//! Restored from the original's reference application, which its framework
//! dropped: multi-file merge. A deployment layers a small file over the
//! committed one instead of copying it.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use thiserror::Error;
use toml::{Table, Value};

/// The environment used when nothing names one.
pub const DEFAULT_ENV: &str = "development";

/// Top-level sections the framework itself owns. Everything else must be
/// claimed by a registered plugin instance.
pub const CORE_SECTIONS: &[&str] = &["app", "server"];

// ---------------------------------------------------------------------------
// Application name
// ---------------------------------------------------------------------------

/// The canonical name of an application.
///
/// One spelling, derived once. `MyApp`, `my_app`, `my-app` and `My App` are the
/// same application, and all four produce `MY_APP_ENV` and `MY_APP_CONFIG`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AppName {
    snake: String,
}

impl AppName {
    /// Normalises any spelling to `snake_case`.
    pub fn new(raw: &str) -> Self {
        Self {
            snake: to_snake_case(raw),
        }
    }

    /// The canonical `snake_case` name. File names use this.
    pub fn as_str(&self) -> &str {
        &self.snake
    }

    /// The one environment-variable convention: `<SNAKE_UPPER>_<SUFFIX>`.
    ///
    /// Stated here, used everywhere, tested once.
    pub fn env_var(&self, suffix: &str) -> String {
        format!("{}_{}", self.snake.to_ascii_uppercase(), suffix)
    }

    /// Name of the variable that selects the environment.
    pub fn env_var_for_env(&self) -> String {
        self.env_var("ENV")
    }

    /// Name of the variable that points at an explicit configuration file.
    pub fn env_var_for_config(&self) -> String {
        self.env_var("CONFIG")
    }
}

impl fmt::Display for AppName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.snake)
    }
}

impl From<&str> for AppName {
    fn from(raw: &str) -> Self {
        Self::new(raw)
    }
}

fn to_snake_case(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(chars.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        if c == '-' || c == ' ' || c == '.' || c == '_' {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            continue;
        }
        if c.is_ascii_uppercase() && i > 0 {
            let prev = chars[i - 1];
            let next_is_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            let boundary = prev.is_ascii_lowercase()
                || prev.is_ascii_digit()
                || (prev.is_ascii_uppercase() && next_is_lower);
            if boundary && !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Everything that can go wrong between a file on disk and a typed section.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// `$<APP>_CONFIG` named a file that is not there. Never ignored: an
    /// operator who sets the variable means it.
    #[error("{var} is set to {} but that file does not exist", .path.display())]
    MissingConfigFile { var: String, path: PathBuf },

    #[error("cannot read {}: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot parse {}: {source}", .path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    #[error("`{name}` is a top-level value, but only sections (TOML tables) are allowed there")]
    NotASection { name: String },

    #[error(
        "unknown configuration section `[{name}]`{}; known sections: {}",
        suggestion_hint(.suggestion),
        .known.join(", ")
    )]
    UnknownSection {
        name: String,
        suggestion: Option<String>,
        known: Vec<String>,
    },

    #[error("section `[{name}]` does not match the type the plugin declared: {source}")]
    Section {
        name: String,
        #[source]
        source: toml::de::Error,
    },
}

fn suggestion_hint(suggestion: &Option<String>) -> String {
    match suggestion {
        Some(s) => format!("; did you mean `[{s}]`?"),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// Where a layer of configuration came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// `$<APP>_CONFIG`.
    EnvVar,
    /// `/etc/<app>/<env>.toml`.
    SystemDir,
    /// `./<app>_<env>.toml`.
    WorkingDir,
    /// `./config/<env>.toml`.
    ConfigDir,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Origin::EnvVar => "env var",
            Origin::SystemDir => "system dir",
            Origin::WorkingDir => "working dir",
            Origin::ConfigDir => "config dir",
        };
        f.write_str(s)
    }
}

/// One file that contributed to the loaded configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub origin: Origin,
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

type EnvLookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Finds and layers the configuration files of one application.
///
/// The loader reads the process environment and the real filesystem by
/// default, and both are injectable. Tests inject, which is why no test in this
/// crate mutates process-wide state.
pub struct Loader {
    app: AppName,
    env: Option<String>,
    root: PathBuf,
    cwd: PathBuf,
    env_lookup: EnvLookup,
}

impl Loader {
    /// Starts a loader for `app`.
    pub fn new(app: impl Into<AppName>) -> Self {
        Self {
            app: app.into(),
            env: None,
            root: PathBuf::from("/"),
            cwd: PathBuf::from("."),
            env_lookup: Box::new(|k| std::env::var(k).ok()),
        }
    }

    /// Forces the environment, instead of reading `$<APP>_ENV`.
    pub fn env(mut self, env: impl Into<String>) -> Self {
        self.env = Some(env.into());
        self
    }

    /// Replaces the filesystem root that `/etc/<app>/` resolves against.
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = root.into();
        self
    }

    /// Replaces the working directory that `./` resolves against.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self
    }

    /// Replaces the environment-variable lookup. Tests use this.
    pub fn env_lookup<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        self.env_lookup = Box::new(f);
        self
    }

    /// The environment this loader resolves to.
    pub fn resolved_env(&self) -> String {
        self.env
            .clone()
            .or_else(|| (self.env_lookup)(&self.app.env_var_for_env()))
            .unwrap_or_else(|| DEFAULT_ENV.to_string())
    }

    /// The candidate files, highest precedence first.
    ///
    /// Order: `$<APP>_CONFIG`, `/etc/<app>/<env>.toml`, `./<app>_<env>.toml`,
    /// `./config/<env>.toml`.
    pub fn candidates(&self) -> Vec<Source> {
        let env = self.resolved_env();
        let app = self.app.as_str();
        let mut out = Vec::with_capacity(4);
        if let Some(p) = (self.env_lookup)(&self.app.env_var_for_config()) {
            out.push(Source {
                path: PathBuf::from(p),
                origin: Origin::EnvVar,
            });
        }
        out.push(Source {
            path: self.root.join("etc").join(app).join(format!("{env}.toml")),
            origin: Origin::SystemDir,
        });
        out.push(Source {
            path: self.cwd.join(format!("{app}_{env}.toml")),
            origin: Origin::WorkingDir,
        });
        out.push(Source {
            path: self.cwd.join("config").join(format!("{env}.toml")),
            origin: Origin::ConfigDir,
        });
        out
    }

    /// Loads and merges every candidate that exists.
    ///
    /// Candidates are listed highest precedence first and merged lowest first,
    /// so a key set in `$<APP>_CONFIG` wins over the same key in
    /// `./config/<env>.toml`. Merging is deep for tables; scalars and arrays
    /// are replaced whole. When no candidate exists the configuration is empty,
    /// which is not an error — plugins fall back to their `Default`.
    pub fn load(&self) -> Result<Config, ConfigError> {
        let candidates = self.candidates();

        // An explicitly pointed-at file that is absent is always a mistake.
        if let Some(s) = candidates.iter().find(|s| s.origin == Origin::EnvVar) {
            if !s.path.is_file() {
                return Err(ConfigError::MissingConfigFile {
                    var: self.app.env_var_for_config(),
                    path: s.path.clone(),
                });
            }
        }

        let present: Vec<Source> = candidates
            .into_iter()
            .filter(|s| s.path.is_file())
            .collect();

        let mut table = Table::new();
        for source in present.iter().rev() {
            let layer = read_table(&source.path)?;
            merge_tables(&mut table, layer);
        }

        Ok(Config {
            app: self.app.clone(),
            env: self.resolved_env(),
            table,
            sources: present,
        })
    }
}

fn read_table(path: &Path) -> Result<Table, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    text.parse::<Table>().map_err(|source| ConfigError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

/// Deep merge: `over` wins, table values recurse, everything else is replaced.
fn merge_tables(base: &mut Table, over: Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(base_tbl)), Value::Table(over_tbl)) => {
                merge_tables(base_tbl, over_tbl);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// The loaded configuration of one application.
///
/// It holds raw sections and knows what none of them mean. Meaning arrives when
/// a plugin instance claims a section name and declares the type it
/// deserialises into.
#[derive(Clone, Debug)]
pub struct Config {
    app: AppName,
    env: String,
    table: Table,
    sources: Vec<Source>,
}

impl Config {
    /// Builds a configuration straight from TOML text. Useful in tests and for
    /// applications that embed their configuration.
    pub fn from_toml(
        app: impl Into<AppName>,
        env: impl Into<String>,
        text: &str,
    ) -> Result<Self, ConfigError> {
        let table = text.parse::<Table>().map_err(|source| ConfigError::Parse {
            path: PathBuf::from("<inline>"),
            source,
        })?;
        Ok(Self {
            app: app.into(),
            env: env.into(),
            table,
            sources: Vec::new(),
        })
    }

    /// An empty configuration.
    pub fn empty(app: impl Into<AppName>, env: impl Into<String>) -> Self {
        Self {
            app: app.into(),
            env: env.into(),
            table: Table::new(),
            sources: Vec::new(),
        }
    }

    pub fn app(&self) -> &AppName {
        &self.app
    }

    pub fn env(&self) -> &str {
        &self.env
    }

    /// The files that contributed, highest precedence first.
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// Every top-level key, in TOML order.
    pub fn section_names(&self) -> impl Iterator<Item = &str> {
        self.table.keys().map(|k| k.as_str())
    }

    /// The raw table of one section.
    pub fn raw_section(&self, name: &str) -> Option<&Table> {
        match self.table.get(name) {
            Some(Value::Table(t)) => Some(t),
            _ => None,
        }
    }

    /// The whole document, for tooling.
    pub fn raw(&self) -> &Table {
        &self.table
    }

    /// Deserialises one section into the type a plugin declared.
    ///
    /// A missing section gives `T::default()`, so `[logger]` stays optional.
    /// Unknown keys *inside* the section are the plugin's business: declare
    /// `#[serde(deny_unknown_fields)]` to reject them, or leave it off to
    /// accept them.
    pub fn section<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, ConfigError> {
        match self.table.get(name) {
            None => Ok(T::default()),
            Some(Value::Table(t)) => Value::Table(t.clone())
                .try_into::<T>()
                .map_err(|source| ConfigError::Section {
                    name: name.to_string(),
                    source,
                }),
            Some(_) => Err(ConfigError::NotASection {
                name: name.to_string(),
            }),
        }
    }

    /// Rejects any top-level key that no plugin instance claimed.
    ///
    /// `claimed` is the set of plugin instance names; [`CORE_SECTIONS`] is
    /// added automatically. The error names the offending section and suggests
    /// the nearest claimed one, because the overwhelmingly common cause is a
    /// typo and a silent build is the worst possible answer to it.
    pub fn validate_sections<S: AsRef<str>>(&self, claimed: &[S]) -> Result<(), ConfigError> {
        let mut known: BTreeSet<String> = CORE_SECTIONS.iter().map(|s| s.to_string()).collect();
        known.extend(claimed.iter().map(|s| s.as_ref().to_string()));

        for (name, value) in &self.table {
            if !value.is_table() {
                return Err(ConfigError::NotASection {
                    name: name.clone(),
                });
            }
            if known.contains(name) {
                continue;
            }
            return Err(ConfigError::UnknownSection {
                name: name.clone(),
                suggestion: nearest(name, &known),
                known: known.into_iter().collect(),
            });
        }
        Ok(())
    }
}

/// The closest known name, when it is close enough to be a plausible typo.
///
/// Shared with the plugin layer, which needs the same courtesy for a misspelt
/// middleware name.
pub(crate) fn nearest_name<S: AsRef<str>>(name: &str, known: &[S]) -> Option<String> {
    let set: BTreeSet<String> = known.iter().map(|s| s.as_ref().to_string()).collect();
    nearest(name, &set)
}

fn nearest(name: &str, known: &BTreeSet<String>) -> Option<String> {
    let budget = (name.chars().count() / 3).max(2);
    known
        .iter()
        .map(|k| (levenshtein(name, k), k))
        .filter(|(d, _)| *d <= budget)
        .min_by_key(|(d, k)| (*d, (*k).clone()))
        .map(|(_, k)| k.clone())
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.is_empty() {
        return b.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::collections::HashMap;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + Send + Sync + 'static {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| map.get(k).cloned()
    }

    // -- one env-var convention ---------------------------------------------

    #[test]
    fn every_spelling_of_the_name_gives_one_env_var_prefix() {
        // The original bug: `MYAPP_ENV` from the PascalCase name,
        // `MY_APP_CONFIG` from the snake_case one, `MYAPP_CONFIG` in the
        // README. One convention now, whatever spelling you start from.
        for raw in ["MyApp", "my_app", "my-app", "My App", "myApp"] {
            let app = AppName::new(raw);
            assert_eq!(app.as_str(), "my_app", "normalising {raw}");
            assert_eq!(app.env_var_for_env(), "MY_APP_ENV", "env var for {raw}");
            assert_eq!(
                app.env_var_for_config(),
                "MY_APP_CONFIG",
                "config var for {raw}"
            );
        }
    }

    #[test]
    fn acronyms_and_single_words_normalise_too() {
        assert_eq!(AppName::new("HTTPServer").as_str(), "http_server");
        assert_eq!(AppName::new("blog").as_str(), "blog");
        assert_eq!(AppName::new("Blog").env_var("CONFIG"), "BLOG_CONFIG");
    }

    // -- resolution order ---------------------------------------------------

    #[test]
    fn candidates_are_listed_highest_precedence_first() {
        let loader = Loader::new("my_app")
            .env("production")
            .root("/fake-root")
            .cwd("/fake-cwd")
            .env_lookup(lookup(&[("MY_APP_CONFIG", "/explicit.toml")]));

        let got: Vec<(PathBuf, Origin)> = loader
            .candidates()
            .into_iter()
            .map(|s| (s.path, s.origin))
            .collect();

        assert_eq!(
            got,
            vec![
                (PathBuf::from("/explicit.toml"), Origin::EnvVar),
                (
                    PathBuf::from("/fake-root/etc/my_app/production.toml"),
                    Origin::SystemDir
                ),
                (
                    PathBuf::from("/fake-cwd/my_app_production.toml"),
                    Origin::WorkingDir
                ),
                (
                    PathBuf::from("/fake-cwd/config/production.toml"),
                    Origin::ConfigDir
                ),
            ]
        );
    }

    #[test]
    fn env_var_selects_the_environment_and_the_default_is_development() {
        let with_var = Loader::new("MyApp").env_lookup(lookup(&[("MY_APP_ENV", "staging")]));
        assert_eq!(with_var.resolved_env(), "staging");

        let without = Loader::new("MyApp").env_lookup(lookup(&[]));
        assert_eq!(without.resolved_env(), DEFAULT_ENV);

        let explicit = Loader::new("MyApp")
            .env("test")
            .env_lookup(lookup(&[("MY_APP_ENV", "staging")]));
        assert_eq!(explicit.resolved_env(), "test");
    }

    #[test]
    fn config_env_var_set_but_missing_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");

        let err = Loader::new("my_app")
            .env("production")
            .root(dir.path())
            .cwd(dir.path())
            .env_lookup(lookup(&[("MY_APP_CONFIG", missing.to_str().unwrap())]))
            .load()
            .unwrap_err();

        match &err {
            ConfigError::MissingConfigFile { var, path } => {
                assert_eq!(var, "MY_APP_CONFIG");
                assert_eq!(path, &missing);
            }
            other => panic!("expected MissingConfigFile, got {other:?}"),
        }
        let msg = err.to_string();
        assert!(msg.contains("MY_APP_CONFIG"), "{msg}");
        assert!(msg.contains("nope.toml"), "{msg}");
    }

    #[test]
    fn an_absent_candidate_that_nobody_named_is_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Loader::new("my_app")
            .env("production")
            .root(dir.path())
            .cwd(dir.path())
            .env_lookup(lookup(&[]))
            .load()
            .unwrap();
        assert!(cfg.sources().is_empty());
        assert_eq!(cfg.section_names().count(), 0);
    }

    // -- multi-file merge ---------------------------------------------------

    #[test]
    fn layers_merge_with_higher_precedence_winning() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Lowest layer: the committed defaults.
        write(
            &root.join("config/production.toml"),
            r#"
            [server]
            host = "0.0.0.0"
            port = 3000
            workers = 4

            [logger]
            level = "info"
            "#,
        );
        // Middle layer: a per-checkout override.
        write(
            &root.join("my_app_production.toml"),
            r#"
            [server]
            port = 8080

            [logger]
            level = "warn"
            "#,
        );
        // Higher layer: the machine.
        write(
            &root.join("etc/my_app/production.toml"),
            r#"
            [logger]
            level = "error"
            "#,
        );
        // Highest layer: the deployment, pointed at explicitly.
        let explicit = root.join("deploy.toml");
        write(
            &explicit,
            r#"
            [server]
            host = "127.0.0.1"
            "#,
        );

        let cfg = Loader::new("my_app")
            .env("production")
            .root(root)
            .cwd(root)
            .env_lookup(lookup(&[("MY_APP_CONFIG", explicit.to_str().unwrap())]))
            .load()
            .unwrap();

        assert_eq!(cfg.sources().len(), 4);
        assert_eq!(cfg.sources()[0].origin, Origin::EnvVar);
        assert_eq!(cfg.sources()[3].origin, Origin::ConfigDir);

        let server = cfg.raw_section("server").unwrap();
        // From the highest layer.
        assert_eq!(server["host"].as_str(), Some("127.0.0.1"));
        // From the middle layer, not overwritten by the one above it.
        assert_eq!(server["port"].as_integer(), Some(8080));
        // Only the lowest layer ever mentioned this, and it survived.
        assert_eq!(server["workers"].as_integer(), Some(4));
        // /etc beats ./ and ./config.
        assert_eq!(
            cfg.raw_section("logger").unwrap()["level"].as_str(),
            Some("error")
        );
    }

    #[test]
    fn merge_is_deep_for_tables_and_whole_for_arrays() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("config/test.toml"),
            r#"
            [db.pool]
            size = 5
            timeout = 30
            [db]
            hosts = ["a", "b", "c"]
            "#,
        );
        write(
            &root.join("my_app_test.toml"),
            r#"
            [db.pool]
            size = 20
            [db]
            hosts = ["z"]
            "#,
        );

        let cfg = Loader::new("my_app")
            .env("test")
            .root(root)
            .cwd(root)
            .env_lookup(lookup(&[]))
            .load()
            .unwrap();

        let db = cfg.raw_section("db").unwrap();
        let pool = db["pool"].as_table().unwrap();
        assert_eq!(pool["size"].as_integer(), Some(20), "overridden");
        assert_eq!(pool["timeout"].as_integer(), Some(30), "nested key kept");
        assert_eq!(
            db["hosts"].as_array().unwrap().len(),
            1,
            "arrays replace, they do not append"
        );
    }

    // -- typed sections -----------------------------------------------------

    #[derive(Debug, Default, Deserialize, PartialEq)]
    struct LoggerConfig {
        level: String,
        #[serde(default)]
        json: bool,
    }

    #[test]
    fn a_section_deserialises_into_the_type_the_plugin_declared() {
        let cfg = Config::from_toml(
            "my_app",
            "test",
            r#"
            [logger]
            level = "warn"
            json = true
            "#,
        )
        .unwrap();

        let logger: LoggerConfig = cfg.section("logger").unwrap();
        assert_eq!(
            logger,
            LoggerConfig {
                level: "warn".into(),
                json: true
            }
        );
    }

    #[test]
    fn a_missing_section_falls_back_to_default() {
        let cfg = Config::empty("my_app", "test");
        let logger: LoggerConfig = cfg.section("logger").unwrap();
        assert_eq!(logger, LoggerConfig::default());
    }

    #[test]
    fn a_section_that_does_not_match_its_type_is_an_error() {
        let cfg = Config::from_toml("my_app", "test", "[logger]\nlevel = 7\n").unwrap();
        let err = cfg.section::<LoggerConfig>("logger").unwrap_err();
        assert!(matches!(err, ConfigError::Section { .. }), "{err:?}");
        assert!(err.to_string().contains("[logger]"), "{err}");
    }

    // -- unknown sections ---------------------------------------------------

    #[test]
    fn an_unknown_top_level_section_is_rejected_and_the_nearest_name_suggested() {
        let cfg = Config::from_toml("my_app", "test", "[loggr]\nlevel = \"warn\"\n").unwrap();
        let err = cfg.validate_sections(&["logger", "db_log"]).unwrap_err();

        match &err {
            ConfigError::UnknownSection {
                name, suggestion, ..
            } => {
                assert_eq!(name, "loggr");
                assert_eq!(suggestion.as_deref(), Some("logger"));
            }
            other => panic!("expected UnknownSection, got {other:?}"),
        }

        let msg = err.to_string();
        assert!(msg.contains("`[loggr]`"), "names the section: {msg}");
        assert!(msg.contains("did you mean `[logger]`?"), "suggests: {msg}");
        assert!(msg.contains("known sections:"), "lists what is valid: {msg}");
    }

    #[test]
    fn a_claimed_section_and_the_core_sections_pass_validation() {
        let cfg = Config::from_toml(
            "my_app",
            "test",
            r#"
            [app]
            name = "my_app"
            [server]
            port = 3000
            [logger]
            level = "warn"
            [db_log]
            level = "debug"
            "#,
        )
        .unwrap();
        cfg.validate_sections(&["logger", "db_log"]).unwrap();
    }

    #[test]
    fn a_wildly_wrong_section_is_rejected_without_a_misleading_suggestion() {
        let cfg = Config::from_toml("my_app", "test", "[telemetry]\nx = 1\n").unwrap();
        let err = cfg.validate_sections(&["logger"]).unwrap_err();
        match &err {
            ConfigError::UnknownSection {
                name, suggestion, ..
            } => {
                assert_eq!(name, "telemetry");
                assert_eq!(suggestion, &None);
            }
            other => panic!("expected UnknownSection, got {other:?}"),
        }
        assert!(!err.to_string().contains("did you mean"), "{err}");
    }

    #[test]
    fn a_top_level_scalar_is_not_a_section() {
        let cfg = Config::from_toml("my_app", "test", "level = \"warn\"\n").unwrap();
        let err = cfg.validate_sections(&["logger"]).unwrap_err();
        assert!(matches!(err, ConfigError::NotASection { .. }), "{err:?}");
    }
}
