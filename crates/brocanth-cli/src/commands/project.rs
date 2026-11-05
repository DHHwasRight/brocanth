//! What `check` reads: the project's marker, as TOML text.
//!
//! The configuration is not read here. Only the project knows how it reads
//! its configuration, so the checks on it are the project's own `check`
//! command, and this tool links no configuration loader.

use std::path::{Path, PathBuf};

use crate::ui::Diagnostic;

/// A project on disk.
pub struct Project {
    /// The directory holding `brocanth.toml`.
    pub root: PathBuf,
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
        Ok(Self { root, modules })
    }

    /// Whether `brocanth.toml` lists this module.
    pub fn has(&self, module: &str) -> bool {
        self.modules.iter().any(|m| m == module)
    }
}
