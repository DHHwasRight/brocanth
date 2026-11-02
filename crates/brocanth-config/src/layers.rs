//! Every layer of a configuration, on its own, and then the merge.
//!
//! [`Dump`](crate::Dump) answers "what is the value and where did it come
//! from". This answers the question before it: "what does each source say".
//! Each layer is printed alone, lowest precedence first, with only the keys it
//! sets, so one layer can be checked without reading the others. The merged
//! view follows, and names the layer each value came from.
//!
//! ```text
//! layers, lowest precedence first
//!
//!   1  defaults
//!        sessions.cookie  = "_my_app_session"
//!        sessions.secure  = true
//!   2  config/development.toml
//!        sessions.cookie  = "_my_app_session"   line 3
//!        sessions.secure  = false               line 5
//!   3  environment
//!        sessions.secure  = true                $MY_APP_SESSIONS__SECURE
//!   4  flags
//!        sessions.cookie  = "x"                 --cookie
//!
//! merged
//!   sessions.cookie       = "x"                 4 flags
//!   sessions.secure       = true                3 environment
//! ```
//!
//! The plugins' defaults are not in a [`Config`]: a plugin built with
//! `plugin_from_config` takes them from its own `Default` when a key is
//! missing. The caller passes them in with [`Layers::defaults`], because only
//! the caller can construct the settings type.

use std::fmt;

use crate::config::Config;
use crate::redact::Redaction;
use crate::value::{Provenance, Value};

/// A printable rendering of every layer of a configuration. Built by
/// [`Config::layers`] and printed with `{}`.
pub struct Layers<'a> {
    config: &'a Config,
    defaults: Vec<(String, String)>,
    secrets: Vec<String>,
    redaction: Redaction,
}

/// Which layer a value belongs to, in precedence order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Layer {
    Defaults,
    /// A file, by its position from the lowest-precedence one.
    File(usize, String),
    Environment,
    Flags,
    Other(String),
}

impl Layer {
    fn name(&self) -> String {
        match self {
            Layer::Defaults => "defaults".to_string(),
            Layer::File(_, path) => path.clone(),
            Layer::Environment => "environment".to_string(),
            Layer::Flags => "flags".to_string(),
            Layer::Other(label) => label.clone(),
        }
    }
}

/// One key as one layer sets it: the path, the value as shown, and the
/// detail that locates it within the layer.
struct Entry {
    path: String,
    shown: String,
    detail: String,
}

impl<'a> Layers<'a> {
    pub(crate) fn new(config: &'a Config) -> Self {
        Self {
            config,
            defaults: Vec::new(),
            secrets: Vec::new(),
            redaction: Redaction::standard(),
        }
    }

    /// The plugins' own defaults, as `(dotted path, value as shown)`.
    #[must_use]
    pub fn defaults<P: Into<String>>(mut self, defaults: Vec<(P, String)>) -> Self {
        self.defaults = defaults
            .into_iter()
            .map(|(path, shown)| (path.into(), shown))
            .collect();
        self
    }

    /// A key to mask whatever its name, in every layer.
    ///
    /// The standard [`Redaction`] already masks keys named like secrets. This
    /// is for one the application declared secret under another name.
    #[must_use]
    pub fn secret(mut self, path: impl Into<String>) -> Self {
        self.secrets.push(path.into());
        self
    }

    fn masked(&self, path: &str) -> bool {
        self.secrets.iter().any(|s| s == path) || self.redaction.hides(path)
    }

    fn show(&self, path: &str, value: &Value) -> String {
        if self.masked(path) {
            return "(secret)".to_string();
        }
        self.redaction.show(path, value).text
    }

    fn layer_of(&self, from: &Provenance) -> Layer {
        match from {
            Provenance::Default { .. } => Layer::Defaults,
            Provenance::File { file, .. } => {
                // `sources` is highest precedence first; a layer number counts
                // from the lowest.
                let sources = self.config.sources();
                let position = sources
                    .iter()
                    .position(|s| s.path == file.path)
                    .map_or(0, |i| sources.len() - i);
                Layer::File(position, file.path.display().to_string())
            }
            Provenance::EnvVar { .. } => Layer::Environment,
            Provenance::Flag { .. } => Layer::Flags,
            Provenance::Inline { label } => Layer::Other(label.to_string()),
        }
    }

    fn detail(from: &Provenance) -> String {
        match from {
            Provenance::File { line, .. } => format!("line {line}"),
            Provenance::EnvVar { var } => format!("${var}"),
            Provenance::Flag { flag } => format!("--{flag}"),
            Provenance::Default { plugin } => format!("default ({plugin})"),
            Provenance::Inline { .. } => String::new(),
        }
    }

    /// Every layer, lowest first, each with the keys it sets.
    fn collect(&self) -> Vec<(Layer, Vec<Entry>)> {
        let mut layers: Vec<(Layer, Vec<Entry>)> = vec![(
            Layer::Defaults,
            self.defaults
                .iter()
                .map(|(path, shown)| Entry {
                    path: path.clone(),
                    shown: if self.masked(path) {
                        "(secret)".to_string()
                    } else {
                        shown.clone()
                    },
                    detail: String::new(),
                })
                .collect(),
        )];
        // Every file, and the environment and the flags, even when they set
        // nothing: an empty layer is a fact worth seeing.
        let sources = self.config.sources();
        for (i, source) in sources.iter().enumerate().rev() {
            layers.push((
                Layer::File(sources.len() - i, source.path.display().to_string()),
                Vec::new(),
            ));
        }
        layers.push((Layer::Environment, Vec::new()));
        layers.push((Layer::Flags, Vec::new()));

        for (path, value) in self.config.root().leaves() {
            if path.is_empty() {
                continue;
            }
            for one in std::iter::once(value).chain(value.shadowed().iter()) {
                let layer = self.layer_of(one.provenance());
                let entry = Entry {
                    path: path.clone(),
                    shown: self.show(&path, one),
                    detail: Self::detail(one.provenance()),
                };
                match layers.iter_mut().find(|(l, _)| *l == layer) {
                    Some((_, entries)) => entries.push(entry),
                    None => layers.push((layer, vec![entry])),
                }
            }
        }
        layers.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, entries) in &mut layers {
            entries.sort_by(|a, b| a.path.cmp(&b.path));
        }
        layers
    }
}

impl fmt::Display for Layers<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let layers = self.collect();
        let width = layers
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|e| e.path.len()))
            .max()
            .unwrap_or(0);
        let shown_width = layers
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|e| e.shown.len()))
            .max()
            .unwrap_or(0);

        writeln!(f, "layers, lowest precedence first")?;
        for (n, (layer, entries)) in layers.iter().enumerate() {
            writeln!(f)?;
            writeln!(f, "  {:>2}  {}", n + 1, layer.name())?;
            if entries.is_empty() {
                writeln!(f, "        (sets nothing)")?;
            }
            for entry in entries {
                let line = format!(
                    "        {:<width$}  = {:<shown_width$}  {}",
                    entry.path, entry.shown, entry.detail
                );
                writeln!(f, "{}", line.trim_end())?;
            }
        }

        // The merged view: for each key, the highest layer that sets it.
        let mut merged: Vec<(&str, &str, usize, String)> = Vec::new();
        for (n, (layer, entries)) in layers.iter().enumerate() {
            for entry in entries {
                // A file's shadowed values are listed too; only the value a
                // later layer did not replace is the merged one, and layers
                // are in order, so the last write wins.
                match merged.iter_mut().find(|(path, ..)| *path == entry.path) {
                    Some(slot) => *slot = (&entry.path, &entry.shown, n + 1, layer.name()),
                    None => merged.push((&entry.path, &entry.shown, n + 1, layer.name())),
                }
            }
        }
        merged.sort_by(|a, b| a.0.cmp(b.0));
        writeln!(f)?;
        writeln!(f, "merged")?;
        for (path, shown, n, name) in merged {
            writeln!(
                f,
                "  {:<w$}  = {:<shown_width$}  {n} {name}",
                path,
                shown,
                w = width + 6
            )?;
        }
        Ok(())
    }
}

impl Config {
    /// Every layer of this configuration on its own, then the merge.
    ///
    /// See [`Layers`]. Secrets are masked by the standard [`Redaction`], and
    /// by [`Layers::secret`] for a key named otherwise.
    #[must_use]
    pub fn layers(&self) -> Layers<'_> {
        Layers::new(self)
    }
}

#[cfg(all(test, feature = "toml"))]
mod tests {
    use crate::Loader;

    #[test]
    fn each_layer_alone_then_the_merge() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::write(
            root.join("config/test.toml"),
            "[sessions]\ncookie = \"file\"\nsecure = false\n",
        )
        .unwrap();
        let config = Loader::new("my_app")
            .env("test")
            .root(root)
            .cwd(root)
            .env_lookup(|_| None)
            .env_vars(|| vec![("MY_APP_SESSIONS__SECURE".into(), "true".into())])
            .flag("sessions.cookie", "cookie", "x", true)
            .load()
            .unwrap();
        let text = config
            .layers()
            .defaults(vec![
                ("sessions.cookie", "\"_my_app_session\"".to_string()),
                ("sessions.secret", "\"hush\"".to_string()),
            ])
            .to_string();
        let file = root.join("config/test.toml").display().to_string();
        let expected = format!(
            "layers, lowest precedence first\n\
             \n\
             \x20  1  defaults\n\
             \x20       sessions.cookie  = \"_my_app_session\"\n\
             \x20       sessions.secret  = (secret)\n\
             \n\
             \x20  2  {file}\n\
             \x20       sessions.cookie  = \"file\"             line 2\n\
             \x20       sessions.secure  = false              line 3\n\
             \n\
             \x20  3  environment\n\
             \x20       sessions.secure  = true               $MY_APP_SESSIONS__SECURE\n\
             \n\
             \x20  4  flags\n\
             \x20       sessions.cookie  = \"x\"                --cookie\n\
             \n\
             merged\n\
             \x20 sessions.cookie        = \"x\"                4 flags\n\
             \x20 sessions.secret        = (secret)           1 defaults\n\
             \x20 sessions.secure        = true               3 environment\n"
        );
        assert_eq!(text, expected, "\n{text}");
    }
}
