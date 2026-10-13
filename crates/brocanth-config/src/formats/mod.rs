//! One parser per format, each behind its own cargo feature.
//!
//! PLAN.md §8.2: *"Feature flags gate compilation, not behaviour. A feature
//! that is off must mean the code is absent, not that a branch returns early.
//! If turning a feature off leaves its dependencies in `Cargo.lock`, the split
//! is wrong."* So each submodule is `#[cfg]`-gated with its dependency, and
//! turning `yaml` off removes the YAML parser from the tree, not just from the
//! call.
//!
//! What stays when every feature is off is this file: the [`Format`] enum, the
//! extension table, and an error that names the feature to turn on. That costs
//! no dependency and it is the difference between a useful message and
//! "unknown extension".
//!
//! # What every parser must deliver
//!
//! A [`Value`] tree in which **every node carries the line it was written on**.
//! That requirement chose all three crates: a parser that returns only a value
//! cannot support this crate's headline feature, which is why neither
//! `serde_json` nor any of the `serde_yaml` forks is used here.

use std::sync::Arc;

use crate::config::Source;
#[cfg(any(feature = "toml", feature = "json", feature = "yaml"))]
use crate::value::Format;
use crate::value::Value;

#[cfg(feature = "json")]
mod json;
#[cfg(feature = "toml")]
mod toml;
#[cfg(feature = "yaml")]
mod yaml;

/// A file this build cannot parse, or one it can parse and could not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// 1-indexed line, when the parser reported one.
    pub line: Option<u32>,
    /// 1-indexed column, when the parser reported one.
    pub column: Option<u32>,
    /// What the parser said, or what feature is missing.
    pub message: String,
}

/// Every item below is used by at least one parser, and a build with every
/// format off has no parser. The crate still compiles then — that is the §8.2
/// claim — so the unused warnings in that one configuration are expected.
#[cfg_attr(
    not(any(feature = "toml", feature = "json", feature = "yaml")),
    allow(dead_code)
)]
impl ParseError {
    pub(crate) fn at(line: u32, column: u32, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            column: Some(column),
            message: message.into(),
        }
    }

    pub(crate) fn whole_file(message: impl Into<String>) -> Self {
        Self {
            line: None,
            column: None,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.line, self.column) {
            (Some(l), Some(c)) => write!(f, "line {l}, column {c}: {}", self.message),
            (Some(l), None) => write!(f, "line {l}: {}", self.message),
            _ => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parses one document into the model, with a provenance on every node.
///
/// # Errors
///
/// [`ParseError`] when the text is not valid in `file.format`, or when that
/// format's feature is off in this build.
#[cfg_attr(
    not(any(feature = "toml", feature = "json", feature = "yaml")),
    allow(unused_variables)
)]
pub fn parse(text: &str, file: &Arc<Source>) -> Result<Value, ParseError> {
    match file.format {
        #[cfg(feature = "toml")]
        Format::Toml => toml::parse(text, file),
        #[cfg(feature = "json")]
        Format::Json => json::parse(text, file),
        #[cfg(feature = "yaml")]
        Format::Yaml => yaml::parse(text, file),
        #[allow(unreachable_patterns)]
        other => Err(ParseError::whole_file(format!(
            "this build cannot read {other}; add `features = [\"{}\"]` to the \
             brocanth-config dependency",
            other.name()
        ))),
    }
}
