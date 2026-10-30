//! What `dbshell` and `check` both read: the project's marker and its
//! configuration, as the files and the environment give them.
//!
//! No build and no project binary. The configuration is read by the same
//! `brocanth_config::Loader` the application runs, with the same name, so
//! the files, the layering and the `<APP>_SECTION__KEY` variables agree with
//! what the application would see at startup.

use std::path::{Path, PathBuf};

use brocanth_config::{AppName, Config, Loader};

use crate::ui::Diagnostic;

/// A project on disk.
pub struct Project {
    /// The directory holding `brocanth.toml`.
    pub root: PathBuf,
    /// `[project] name`, which is also the configuration's application name.
    pub name: String,
    /// `[modules] enabled`, empty for a marker from before the list.
    pub modules: Vec<String>,
}

impl Project {
    /// The project at or above `dir`, or the working directory.
    pub fn find(dir: Option<&Path>) -> Result<Self, Vec<Diagnostic>> {
        let start = match dir {
            Some(dir) => dir.to_path_buf(),
            None => std::env::current_dir().map_err(|e| {
                vec![Diagnostic::error(
                    "cwd",
                    format!("cannot read the current directory: {e}"),
                )]
            })?,
        };
        let root = super::delegate::project_root(&start)?;
        let marker = root.join(super::delegate::MARKER);
        let text = std::fs::read_to_string(&marker).map_err(|e| {
            vec![
                Diagnostic::error("marker", format!("cannot read the marker: {e}"))
                    .at(marker.display().to_string()),
            ]
        })?;
        let doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| {
            vec![
                Diagnostic::error("marker", format!("the marker is not TOML: {e}"))
                    .at(marker.display().to_string())
                    .help("`brocanth add` writes this file; restore it from version control"),
            ]
        })?;
        let name = doc
            .get("project")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .map(str::to_owned)
            .or_else(|| root.file_name().and_then(|n| n.to_str()).map(str::to_owned))
            .unwrap_or_default();
        let modules = doc
            .get("modules")
            .and_then(|m| m.get("enabled"))
            .and_then(|e| e.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            root,
            name,
            modules,
        })
    }

    /// Whether `brocanth.toml` lists this module.
    pub fn has(&self, module: &str) -> bool {
        self.modules.iter().any(|m| m == module)
    }

    /// The application name the loader and the variables use.
    pub fn app(&self) -> AppName {
        AppName::new(&self.name)
    }

    /// The configuration of `env`, or of `$<APP>_ENV`, or of `development`.
    pub fn config(&self, env: Option<&str>) -> Result<Config, Vec<Diagnostic>> {
        let mut loader = Loader::new(self.app()).cwd(&self.root);
        if let Some(env) = env {
            loader = loader.env(env);
        }
        loader.load().map_err(|e| {
            vec![
                Diagnostic::error("config", format!("the configuration does not load: {e}")).help(
                    "fix the file it names; the application refuses the same file at startup",
                ),
            ]
        })
    }

    /// The file a key of `env` belongs in, for a `help:` line.
    pub fn config_file(&self, env: &str) -> String {
        format!("config/{env}.toml")
    }

    /// The variable that sets `section.key`, for a `help:` line.
    pub fn variable(&self, section: &str, key: &str) -> String {
        self.app().env_var(&format!(
            "{}__{}",
            section.to_ascii_uppercase(),
            key.to_ascii_uppercase()
        ))
    }
}
