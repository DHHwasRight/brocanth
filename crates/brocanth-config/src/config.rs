//! Configuration: find it, layer it, and hand typed sections to plugins.
//!
//! None of this is in the core, and that is the point. PLAN.md §8: *"Config is
//! a plugin too. The core takes already-parsed values. TOML, the
//! environment-scoped file resolution, and the merge order are a crate an
//! application opts into — a service configured entirely from its environment
//! should not link a TOML parser."* So `serde` and the parsers are dependencies
//! of this crate and of nothing below it — and, since this crate grew a second
//! and a third format, each parser is a feature of *this* crate too, so an
//! application that reads YAML links no TOML parser either.
//!
//! Two decisions from the Crystal original survive the port, and one bug does
//! not.
//!
//! Survives: configuration is *enriched by plugins*, not pre-declared by the
//! framework. [`Config`] keeps every section as a raw [`Value`] and knows the
//! name of none of them. A plugin declares a `Deserialize` struct and the
//! framework routes the table whose name matches the plugin instance into it.
//! So `[logger] level = "warn"` becomes a `LoggerConfig` without this file ever
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
//! spelling the application was named with. The environment layer in
//! [`crate::env`] follows the same rule, so `MY_APP_SERVER__PORT` is visibly
//! the same family as `MY_APP_ENV`.
//!
//! Restored from the original's reference application, which its framework
//! dropped: multi-file merge. A deployment layers a small file over the
//! committed one instead of copying it.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brocanth::suggest::nearest_name;
use serde::de::DeserializeOwned;
use thiserror::Error;

use crate::de::{from_value, DeError};
use crate::dump::Dump;
use crate::env::EnvError;
use crate::formats::{self, ParseError};
use crate::plugin::SectionClaim;
use crate::value::{self, Format, Provenance, Table, Value, ValueKind};

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
    /// Stated here, used everywhere, tested once. The configuration layer
    /// spells a key `<SNAKE_UPPER>_<SECTION>__<KEY>`, which is this rule with a
    /// path for a suffix.
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

/// Everything about one key that no registered plugin claims.
///
/// Its own type because [`ConfigError::UnknownKey`] boxes it, and its own
/// `Display` because that is where the message lives.
#[derive(Debug)]
pub struct UnknownKey {
    /// The section the key is in.
    pub section: String,
    /// The plugin that owns the section.
    pub plugin: String,
    /// The key's dotted path.
    pub path: String,
    /// Where the key was set.
    pub from: Option<Provenance>,
    /// The nearest key that does exist.
    pub suggestion: Option<String>,
    /// Every key the plugin has at that level.
    pub known: Vec<String>,
}

impl fmt::Display for UnknownKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "`{}` is set{}, and `{}`, which owns `[{}]`, has no such key{}; it has: {}",
            self.path,
            at_hint(&self.from),
            self.plugin,
            self.section,
            suggestion_hint_key(&self.suggestion),
            self.known.join(", ")
        )
    }
}

impl std::error::Error for UnknownKey {}

/// Everything that can go wrong between a file on disk and a typed section.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// `$<APP>_CONFIG` named a file that is not there. Never ignored: an
    /// operator who sets the variable means it.
    #[error("{var} is set to {} but that file does not exist", .path.display())]
    MissingConfigFile {
        /// The variable that named it.
        var: String,
        /// The path it named.
        path: PathBuf,
    },

    /// `$<APP>_CONFIG` named a file this build has no parser for.
    #[error(
        "{var} is set to {}, and this build cannot read a `.{extension}` file; it reads: {}",
        .path.display(),
        .supported.join(", ")
    )]
    UnreadableConfigFile {
        /// The variable that named it.
        var: String,
        /// The path it named.
        path: PathBuf,
        /// The extension that named no enabled format.
        extension: String,
        /// The extensions this build does read.
        supported: Vec<String>,
    },

    /// Two files fill one slot, in two formats. Which one wins is not a
    /// question this crate will answer silently.
    #[error(
        "`{}` and `{}` are the same source written twice; delete one",
        .first.display(),
        .second.display()
    )]
    AmbiguousSource {
        /// The file found first, in probe order.
        first: PathBuf,
        /// The other one.
        second: PathBuf,
    },

    /// A file that is there and cannot be read.
    #[error("cannot read {}: {source}", .path.display())]
    Io {
        /// The file.
        path: PathBuf,
        /// What the filesystem said.
        #[source]
        source: std::io::Error,
    },

    /// A file that is there and does not parse.
    #[error("cannot parse {} as {format}: {source}", .path.display())]
    Parse {
        /// The file.
        path: PathBuf,
        /// The parser that was used.
        format: Format,
        /// What the parser said, with a line.
        #[source]
        source: ParseError,
    },

    /// A variable in the environment layer that cannot become a value.
    #[error("{source}")]
    Env {
        /// Which variable, and why.
        #[source]
        source: EnvError,
    },

    /// A top-level key holding something that is not a section.
    #[error("`{name}` is a top-level value, but only sections (tables) are allowed there")]
    NotASection {
        /// The offending key.
        name: String,
    },

    /// A section no registered plugin claims.
    #[error(
        "unknown configuration section `[{name}]`{}; known sections: {}",
        suggestion_hint(.suggestion),
        .known.join(", ")
    )]
    UnknownSection {
        /// The offending section.
        name: String,
        /// The nearest claimed name, when there is one close enough.
        suggestion: Option<String>,
        /// Every claimed name.
        known: Vec<String>,
    },

    /// A key inside a claimed section that its plugin has not got. The same
    /// failure class as an unknown section, one level down.
    ///
    /// Boxed: it carries six fields and it is the largest thing that can go
    /// wrong here, so leaving it inline would make every `Result` in the crate
    /// the size of this one error.
    #[error("{0}")]
    UnknownKey(Box<UnknownKey>),

    /// Two plugins both claim one section name.
    #[error(
        "`{plugin}` claims the configuration section `[{name}]`, and `{first}` already claims it; \
         one section belongs to one plugin, so register one of them under another name with \
         `plugin_as_with_defaults`"
    )]
    DuplicateSection {
        /// The contested section.
        name: String,
        /// The plugin that claimed it first.
        first: String,
        /// The plugin that claimed it second.
        plugin: String,
    },

    /// A section that does not fit the type its plugin declared.
    #[error("section `[{name}]` does not match the type the plugin declared: {source}")]
    Section {
        /// The section.
        name: String,
        /// What did not fit, and where that value came from.
        #[source]
        source: DeError,
    },

    /// A plugin's own defaults cannot be put in the model. A bug in that
    /// plugin, caught before anything else runs.
    #[error("the defaults of `{name}` cannot be read as configuration: {message}")]
    Defaults {
        /// The section the plugin claimed.
        name: String,
        /// What went wrong.
        message: String,
    },

    /// A section was asked for with no configuration installed.
    #[error(
        "`[{name}]` was asked for before a configuration was installed; call \
         `.with_config(Loader::new(\"my_app\").load()?)` on the builder first"
    )]
    NoConfigInstalled {
        /// The section that was asked for.
        name: String,
    },
}

fn suggestion_hint(suggestion: &Option<String>) -> String {
    match suggestion {
        Some(s) => format!("; did you mean `[{s}]`?"),
        None => String::new(),
    }
}

fn suggestion_hint_key(suggestion: &Option<String>) -> String {
    match suggestion {
        Some(s) => format!("; did you mean `{s}`?"),
        None => String::new(),
    }
}

fn at_hint(from: &Option<Provenance>) -> String {
    match from {
        Some(p) => format!(" at {p}"),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------

/// Where a layer of configuration came from.
///
/// The four file slots, in precedence order. The environment layer sits above
/// all four and is not a file, so it is not one of these; see [`Provenance`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    /// `$<APP>_CONFIG`.
    EnvVar,
    /// `/etc/<app>/<env>.<ext>`.
    SystemDir,
    /// `./<app>_<env>.<ext>`.
    WorkingDir,
    /// `./config/<env>.<ext>`.
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Source {
    /// Where the file is.
    pub path: PathBuf,
    /// Which of the four slots it fills.
    pub origin: Origin,
    /// Which parser read it.
    pub format: Format,
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

type EnvLookup = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;
type EnvVars = Box<dyn Fn() -> Vec<(String, String)> + Send + Sync>;

/// Finds and layers the configuration of one application.
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
    env_vars: Option<EnvVars>,
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
            env_vars: None,
        }
    }

    /// Forces the environment, instead of reading `$<APP>_ENV`.
    #[must_use]
    pub fn env(mut self, env: impl Into<String>) -> Self {
        self.env = Some(env.into());
        self
    }

    /// Replaces the filesystem root that `/etc/<app>/` resolves against.
    #[must_use]
    pub fn root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = root.into();
        self
    }

    /// Replaces the working directory that `./` resolves against.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self
    }

    /// Replaces the environment-variable lookup. Tests use this.
    ///
    /// It replaces the process environment *entirely*, so a loader given one
    /// scans no variables for the configuration layer either until
    /// [`Loader::env_vars`] supplies them. A test that injected a lookup and
    /// then read the real `MY_APP_SERVER__PORT` of whoever ran it would not be
    /// a test.
    #[must_use]
    pub fn env_lookup<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) -> Option<String> + Send + Sync + 'static,
    {
        self.env_lookup = Box::new(f);
        if self.env_vars.is_none() {
            self.env_vars = Some(Box::new(Vec::new));
        }
        self
    }

    /// Replaces the variables the configuration layer scans.
    ///
    /// The default is the process environment. [`Loader::env_lookup`] answers
    /// the two variables the loader itself reads; this answers the many a
    /// configuration can set.
    #[must_use]
    pub fn env_vars<F>(mut self, f: F) -> Self
    where
        F: Fn() -> Vec<(String, String)> + Send + Sync + 'static,
    {
        self.env_vars = Some(Box::new(f));
        self
    }

    /// The environment this loader resolves to.
    pub fn resolved_env(&self) -> String {
        self.env
            .clone()
            .or_else(|| (self.env_lookup)(&self.app.env_var_for_env()))
            .unwrap_or_else(|| DEFAULT_ENV.to_string())
    }

    /// The four file sources, highest precedence first, each spelled with the
    /// preferred extension of this build.
    ///
    /// Order: `$<APP>_CONFIG`, `/etc/<app>/<env>`, `./<app>_<env>`,
    /// `./config/<env>`. [`Loader::candidate_files`] lists every spelling the
    /// loader probes; this lists one per slot, which is what an error message
    /// and a `--help` want.
    pub fn candidates(&self) -> Vec<Source> {
        let preferred = Format::enabled().first().copied().unwrap_or(Format::Toml);
        self.slots()
            .into_iter()
            .map(|(path, origin, explicit)| match explicit {
                Some(format) => Source {
                    path,
                    origin,
                    format,
                },
                None => Source {
                    path: with_extension(&path, preferred.extensions()[0]),
                    origin,
                    format: preferred,
                },
            })
            .collect()
    }

    /// Every file this build probes, highest precedence first.
    ///
    /// One entry per slot and extension: with `toml` and `yaml` on, the config
    /// directory contributes `config/<env>.toml`, `config/<env>.yaml` and
    /// `config/<env>.yml`. Finding two of them is an error, not a precedence
    /// question; see [`ConfigError::AmbiguousSource`].
    pub fn candidate_files(&self) -> Vec<Source> {
        let mut out = Vec::new();
        for (path, origin, explicit) in self.slots() {
            match explicit {
                Some(format) => out.push(Source {
                    path,
                    origin,
                    format,
                }),
                None => {
                    for format in Format::enabled() {
                        for ext in format.extensions() {
                            out.push(Source {
                                path: with_extension(&path, ext),
                                origin,
                                format,
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// The slots: a path without an extension, the slot, and the format when
    /// the slot already names a whole path.
    fn slots(&self) -> Vec<(PathBuf, Origin, Option<Format>)> {
        let env = self.resolved_env();
        let app = self.app.as_str();
        let mut out: Vec<(PathBuf, Origin, Option<Format>)> = Vec::with_capacity(4);
        if let Some(p) = (self.env_lookup)(&self.app.env_var_for_config()) {
            let path = PathBuf::from(p);
            let format = path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(Format::from_extension)
                .unwrap_or(Format::Toml);
            out.push((path, Origin::EnvVar, Some(format)));
        }
        out.push((
            self.root.join("etc").join(app).join(&env),
            Origin::SystemDir,
            None,
        ));
        out.push((
            self.cwd.join(format!("{app}_{env}")),
            Origin::WorkingDir,
            None,
        ));
        out.push((self.cwd.join("config").join(&env), Origin::ConfigDir, None));
        out
    }

    /// Loads and merges every source that exists.
    ///
    /// Sources are listed highest precedence first and merged lowest first, so
    /// a key set in the environment wins over the same key in `$<APP>_CONFIG`,
    /// which wins over the same key in `./config/<env>.toml`. Merging is deep
    /// for tables; scalars and arrays are replaced whole. When no source exists
    /// the configuration is empty, which is not an error — plugins fall back to
    /// their defaults.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] when a named file is missing, two files fill one slot, a
    /// file does not parse, or a variable names an impossible path.
    pub fn load(&self) -> Result<Config, ConfigError> {
        let present = self.resolve_files()?;

        let mut root = Value::new(
            ValueKind::Table(Table::open()),
            Provenance::inline("nothing"),
        );
        for source in present.iter().rev() {
            let file = Arc::new(source.clone());
            let text = std::fs::read_to_string(&source.path).map_err(|e| ConfigError::Io {
                path: source.path.clone(),
                source: e,
            })?;
            let layer = formats::parse(&text, &file).map_err(|e| ConfigError::Parse {
                path: source.path.clone(),
                format: source.format,
                source: e,
            })?;
            value::merge(&mut root, layer);
        }

        let vars = match &self.env_vars {
            Some(f) => f(),
            None => std::env::vars().collect(),
        };
        let (env_layer, used) =
            crate::env::layer(&self.app, &vars).map_err(|source| ConfigError::Env { source })?;
        value::merge(&mut root, env_layer);

        Ok(Config {
            app: self.app.clone(),
            env: self.resolved_env(),
            root,
            sources: present,
            env_vars: used,
        })
    }

    /// The files that exist, highest precedence first, at most one per slot.
    fn resolve_files(&self) -> Result<Vec<Source>, ConfigError> {
        // An explicitly pointed-at file that is absent is always a mistake, and
        // it is reported before anything about its extension: an operator who
        // mistyped the path wants to hear about the path.
        if let Some(s) = self
            .candidate_files()
            .into_iter()
            .find(|s| s.origin == Origin::EnvVar)
        {
            if !s.path.is_file() {
                return Err(ConfigError::MissingConfigFile {
                    var: self.app.env_var_for_config(),
                    path: s.path,
                });
            }
            let extension = s
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_string();
            if !Format::from_extension(&extension).is_some_and(Format::is_enabled) {
                return Err(ConfigError::UnreadableConfigFile {
                    var: self.app.env_var_for_config(),
                    path: s.path,
                    extension,
                    supported: Format::enabled_extensions()
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                });
            }
        }

        let mut present: Vec<Source> = Vec::new();
        for source in self.candidate_files() {
            if !source.path.is_file() {
                continue;
            }
            if let Some(already) = present.iter().find(|s| s.origin == source.origin) {
                return Err(ConfigError::AmbiguousSource {
                    first: already.path.clone(),
                    second: source.path,
                });
            }
            present.push(source);
        }
        Ok(present)
    }
}

fn with_extension(stem: &Path, ext: &str) -> PathBuf {
    let mut name = stem
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    name.push('.');
    name.push_str(ext);
    stem.with_file_name(name)
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// The loaded configuration of one application.
///
/// It holds raw sections and knows what none of them mean. Meaning arrives when
/// a plugin instance claims a section name and declares the type it
/// deserialises into.
///
/// Every value in it carries its [`Provenance`]. That is not a debugging extra:
/// it is what lets [`Config::dump`] print the whole effective configuration
/// with the origin of every key, which is the condition PLAN.md §8 puts on a
/// framework supplying any default at all.
#[derive(Clone, Debug)]
pub struct Config {
    app: AppName,
    env: String,
    root: Value,
    sources: Vec<Source>,
    env_vars: Vec<String>,
}

impl Config {
    /// Builds a configuration straight from TOML text. Useful in tests and for
    /// applications that embed their configuration.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Parse`] when the text is not TOML.
    #[cfg(feature = "toml")]
    pub fn from_toml(
        app: impl Into<AppName>,
        env: impl Into<String>,
        text: &str,
    ) -> Result<Self, ConfigError> {
        Self::from_text(app, env, Format::Toml, text)
    }

    /// Builds a configuration from text in any format this build can read.
    ///
    /// Every value's provenance is the pseudo-file `<inline toml>` and the line
    /// it was written on, so a dump of an embedded configuration is as useful
    /// as a dump of one on disk.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Parse`] when the text is not valid in `format`, or when
    /// this build has no parser for it.
    pub fn from_text(
        app: impl Into<AppName>,
        env: impl Into<String>,
        format: Format,
        text: &str,
    ) -> Result<Self, ConfigError> {
        let path = PathBuf::from(format!("<inline {}>", format.name()));
        let file = Arc::new(Source {
            path: path.clone(),
            origin: Origin::WorkingDir,
            format,
        });
        let root = formats::parse(text, &file).map_err(|source| ConfigError::Parse {
            path,
            format,
            source,
        })?;
        Ok(Self {
            app: app.into(),
            env: env.into(),
            root,
            sources: Vec::new(),
            env_vars: Vec::new(),
        })
    }

    /// An empty configuration.
    pub fn empty(app: impl Into<AppName>, env: impl Into<String>) -> Self {
        Self {
            app: app.into(),
            env: env.into(),
            root: Value::new(
                ValueKind::Table(Table::open()),
                Provenance::inline("nothing"),
            ),
            sources: Vec::new(),
            env_vars: Vec::new(),
        }
    }

    /// The application this belongs to.
    pub fn app(&self) -> &AppName {
        &self.app
    }

    /// The environment it was loaded for.
    pub fn env(&self) -> &str {
        &self.env
    }

    /// The files that contributed, highest precedence first.
    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// The environment variables that contributed, in name order.
    pub fn env_vars(&self) -> &[String] {
        &self.env_vars
    }

    /// Every top-level key, in name order.
    pub fn section_names(&self) -> impl Iterator<Item = &str> {
        self.root.as_table().into_iter().flat_map(Table::keys)
    }

    /// The raw table of one section.
    pub fn raw_section(&self, name: &str) -> Option<&Table> {
        self.root.as_table()?.get(name)?.as_table()
    }

    /// The whole document, for tooling.
    pub fn raw(&self) -> &Table {
        self.root.as_table().expect("the root is always a table")
    }

    /// The whole document as a value, so a caller can walk the provenance.
    pub fn root(&self) -> &Value {
        &self.root
    }

    /// Where one key came from. `cfg.provenance("server.pool.max_size")`.
    pub fn provenance(&self, path: &str) -> Option<&Provenance> {
        self.root.at(path).map(Value::provenance)
    }

    /// The configuration, printed for a human.
    ///
    /// This one has no plugin defaults in it, because a bare [`Config`] has met
    /// no plugins. [`Effective::dump`](crate::Effective::dump) is the one an
    /// application wants after `build()`.
    pub fn dump(&self) -> Dump<'_> {
        Dump {
            config: self,
            claims: &[],
            history: true,
            redaction: crate::redact::Redaction::standard(),
        }
    }

    pub(crate) fn dump_with<'a>(&'a self, claims: &'a [SectionClaim]) -> Dump<'a> {
        Dump {
            config: self,
            claims,
            history: true,
            redaction: crate::redact::Redaction::standard(),
        }
    }

    /// Puts `lower` underneath everything already here, keeping provenance on
    /// both sides. This is how a plugin's defaults become the bottom layer.
    pub(crate) fn merge_under(&mut self, lower: Value) {
        let mut base = lower;
        value::merge(&mut base, self.root.clone());
        self.root = base;
    }

    /// Deserialises one section into the type a plugin declared.
    ///
    /// A missing section gives `T::default()`, so `[logger]` stays optional.
    /// Unknown keys *inside* the section are the plugin's business when the
    /// plugin declared no defaults: declare `#[serde(deny_unknown_fields)]` to
    /// reject them, or leave it off to accept them. A plugin registered through
    /// [`plugin_with_defaults`](crate::AppBuilderConfigExt::plugin_with_defaults)
    /// gets the check without the attribute, and with a better error.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Section`] when the section does not fit `T`, and
    /// [`ConfigError::NotASection`] when the name holds a scalar.
    pub fn section<T: DeserializeOwned + Default>(&self, name: &str) -> Result<T, ConfigError> {
        match self.root.as_table().and_then(|t| t.get(name)) {
            None => Ok(T::default()),
            Some(v) if v.is_table() => self.deserialize_section(name, v.clone()),
            Some(_) => Err(ConfigError::NotASection {
                name: name.to_string(),
            }),
        }
    }

    pub(crate) fn deserialize_section<T: DeserializeOwned>(
        &self,
        name: &str,
        value: Value,
    ) -> Result<T, ConfigError> {
        from_value(value).map_err(|source| ConfigError::Section {
            name: name.to_string(),
            source,
        })
    }

    /// Rejects any top-level key that no plugin instance claimed.
    ///
    /// `claimed` is the set of plugin instance names; [`CORE_SECTIONS`] is
    /// added automatically. The error names the offending section and suggests
    /// the nearest claimed one, because the overwhelmingly common cause is a
    /// typo and a silent build is the worst possible answer to it.
    ///
    /// # Errors
    ///
    /// [`ConfigError::UnknownSection`] or [`ConfigError::NotASection`].
    pub fn validate_sections<S: AsRef<str>>(&self, claimed: &[S]) -> Result<(), ConfigError> {
        let mut known: BTreeSet<String> = CORE_SECTIONS.iter().map(|s| s.to_string()).collect();
        known.extend(claimed.iter().map(|s| s.as_ref().to_string()));

        let table = self.root.as_table().expect("the root is always a table");
        for (name, value) in table.iter() {
            if !value.is_table() {
                return Err(ConfigError::NotASection { name: name.clone() });
            }
            if known.contains(name) {
                continue;
            }
            let known: Vec<String> = known.into_iter().collect();
            return Err(ConfigError::UnknownSection {
                name: name.clone(),
                suggestion: nearest_name(name, &known),
                known,
            });
        }
        Ok(())
    }

    /// Rejects any key inside a claimed section that the owning plugin has not
    /// got.
    ///
    /// The key set comes from the plugin's own defaults, through serde's
    /// distinction between a struct and a map; see [`crate::ser`]. A key inside
    /// a map is never unclaimed, because a map exists to take keys the plugin
    /// author did not write down.
    ///
    /// # Errors
    ///
    /// [`ConfigError::UnknownKey`], naming the key, the plugin, where the key
    /// was set, and the nearest key that does exist.
    pub fn validate_keys(&self, claims: &[SectionClaim]) -> Result<(), ConfigError> {
        for claim in claims {
            let Some(found) = self.raw_section(&claim.section) else {
                continue;
            };
            let Some(declared) = claim.defaults.as_table() else {
                continue;
            };
            check_keys(claim, &claim.section, found, declared)?;
        }
        Ok(())
    }
}

fn check_keys(
    claim: &SectionClaim,
    path: &str,
    found: &Table,
    declared: &Table,
) -> Result<(), ConfigError> {
    if declared.is_open() {
        return Ok(());
    }
    for (key, value) in found.iter() {
        let here = format!("{path}.{key}");
        match declared.get(key) {
            None => {
                let known: Vec<String> = declared.keys().map(str::to_string).collect();
                return Err(ConfigError::UnknownKey(Box::new(UnknownKey {
                    section: claim.section.clone(),
                    plugin: claim.plugin.clone(),
                    path: here,
                    from: Some(value.provenance().clone()),
                    suggestion: nearest_name(key, &known),
                    known,
                })));
            }
            Some(declared_value) => {
                if let (Some(found_table), Some(declared_table)) =
                    (value.as_table(), declared_value.as_table())
                {
                    check_keys(claim, &here, found_table, declared_table)?;
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
}

#[cfg(all(test, feature = "toml"))]
mod toml_tests {
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
        assert!(
            msg.contains("known sections:"),
            "lists what is valid: {msg}"
        );
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
