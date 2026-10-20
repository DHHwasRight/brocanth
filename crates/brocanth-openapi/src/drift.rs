//! Emitting the document, and failing the build when the committed one no
//! longer matches.
//!
//! # Why this is the feature that justifies the crate
//!
//! Generating a document is useful. Generating it and *checking it in* is
//! what makes it a contract: the file is reviewable, diffable, and consumable
//! by a client generator in another repository that cannot run this one's
//! build. The cost of a checked-in generated file is that it goes stale, and
//! a stale API document is worse than none because it is believed.
//!
//! [`check_file`] closes that: run it in CI, and a route added without
//! regenerating the document is a red build on the pull request that added
//! it, naming the route. "The spec is out of date" stops being a discovery
//! somebody makes in an incident and becomes a test failure with a line
//! number.
//!
//! # What counts as a difference
//!
//! For JSON, the committed text is parsed and compared *structurally*.
//! Reformatting is not drift, reordering keys is not drift, and `1` against
//! `1.0` is not drift. Only content is. The report names each difference by
//! JSON pointer, so the failure says `/paths/~1posts~1{id}/get/operationId`
//! rather than "the files differ".
//!
//! For YAML the comparison is textual, because parsing YAML would mean taking
//! on a YAML parser and this crate's whole argument is that it adds nothing
//! to an application's dependency tree. Emit JSON as the committed artefact
//! and YAML as a convenience, which is the usual arrangement anyway.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::document::Document;
use crate::json::{self, Json, JsonError};

/// How a document is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// Indented JSON. The default, and the one to commit.
    #[default]
    Json,
    /// Block-style YAML.
    Yaml,
}

impl Format {
    /// The format a file extension implies, or [`None`] for anything else.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension().and_then(|e| e.to_str()) {
            Some("json") => Some(Self::Json),
            Some("yaml" | "yml") => Some(Self::Yaml),
            _ => None,
        }
    }

    /// The media type to serve this format as.
    #[must_use]
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Json => "application/json",
            // RFC 9512 registered this in 2024; `text/yaml` was never
            // registered and is still what most tools send.
            Self::Yaml => "application/yaml",
        }
    }
}

/// Renders the document, with a trailing newline.
#[must_use]
pub fn render(document: &Document, format: Format) -> String {
    match format {
        Format::Json => document.to_json_string(),
        Format::Yaml => document.to_yaml_string(),
    }
}

/// Writes the document to `path`, creating parent directories.
///
/// The format comes from the extension, and an unknown extension is JSON.
/// This is the function a CLI calls.
///
/// # Errors
///
/// Any I/O error from creating the directory or writing the file.
pub fn write(document: &Document, path: impl AsRef<Path>) -> io::Result<()> {
    let path = path.as_ref();
    let format = Format::from_path(path).unwrap_or_default();
    write_as(document, path, format)
}

/// Writes the document to `path` in a named format.
///
/// # Errors
///
/// Any I/O error from creating the directory or writing the file.
pub fn write_as(document: &Document, path: impl AsRef<Path>, format: Format) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, render(document, format))
}

// ---------------------------------------------------------------------------
// Drift
// ---------------------------------------------------------------------------

/// How one place in the document differs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The generated document has it; the committed one does not.
    Added,
    /// The committed document has it; the generated one does not.
    Removed,
    /// Both have it and they disagree.
    Changed {
        /// What the committed document says.
        committed: String,
        /// What the generated document says.
        generated: String,
    },
}

/// One difference, located by JSON pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// RFC 6901 pointer into the document.
    pub pointer: String,
    /// What kind of difference it is.
    pub change: Change,
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.change {
            Change::Added => write!(f, "{}: added", self.pointer),
            Change::Removed => write!(f, "{}: removed", self.pointer),
            Change::Changed {
                committed,
                generated,
            } => write!(
                f,
                "{}: committed `{committed}`, generated `{generated}`",
                self.pointer
            ),
        }
    }
}

/// Everything that differs between the committed document and the generated
/// one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Drift {
    /// Where the committed document is.
    pub path: Option<PathBuf>,
    /// The differences, in document order, at most [`Drift::LIMIT`] of them.
    pub differences: Vec<Difference>,
    /// How many differences there were in total.
    pub total: usize,
}

impl Drift {
    /// How many differences are reported before the list is cut short.
    ///
    /// A route removed from the table produces one difference per field under
    /// it; printing four hundred of those buries the one line that says which
    /// route.
    pub const LIMIT: usize = 20;
}

impl std::fmt::Display for Drift {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.path {
            Some(path) => writeln!(
                f,
                "{} is out of date: {} {} against the generated document.",
                path.display(),
                self.total,
                if self.total == 1 {
                    "difference"
                } else {
                    "differences"
                }
            )?,
            None => writeln!(
                f,
                "the committed document is out of date: {} {} against the generated document.",
                self.total,
                if self.total == 1 {
                    "difference"
                } else {
                    "differences"
                }
            )?,
        }
        for difference in &self.differences {
            writeln!(f, "  - {difference}")?;
        }
        if self.total > self.differences.len() {
            writeln!(f, "  ... and {} more", self.total - self.differences.len())?;
        }
        write!(f, "Regenerate it and commit the result.")
    }
}

impl std::error::Error for Drift {}

/// Why a drift check could not conclude, or did.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DriftError {
    /// The document has drifted. This is the expected failure.
    #[error(transparent)]
    Drifted(#[from] Drift),

    /// The committed file is not there.
    ///
    /// Treated as a failure rather than as a reason to write it: a check
    /// that silently creates what it was asked to verify checks nothing.
    #[error("{path} does not exist; generate it and commit it")]
    Missing {
        /// Where it should have been.
        path: PathBuf,
    },

    /// The committed file could not be read.
    #[error("{path} could not be read: {source}")]
    Unreadable {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: io::Error,
    },

    /// The committed file is not JSON.
    #[error("{path} is not valid JSON: {source}")]
    Unparseable {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: JsonError,
    },
}

/// Compares a committed document against the generated one.
///
/// JSON is compared structurally; YAML is compared as text.
///
/// # Errors
///
/// [`DriftError::Drifted`] when the two disagree, and
/// [`DriftError::Unparseable`] when the committed JSON does not parse.
pub fn check(document: &Document, committed: &str, format: Format) -> Result<(), DriftError> {
    check_located(document, committed, format, None)
}

/// Compares the document against a committed file.
///
/// The format comes from the file extension. This is what runs in CI.
///
/// # Errors
///
/// [`DriftError::Missing`] when the file is not there, [`DriftError::Drifted`]
/// when it is out of date, and the other variants when it cannot be read or
/// parsed.
pub fn check_file(document: &Document, path: impl AsRef<Path>) -> Result<(), DriftError> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(DriftError::Missing {
            path: path.to_path_buf(),
        });
    }
    let committed = fs::read_to_string(path).map_err(|source| DriftError::Unreadable {
        path: path.to_path_buf(),
        source,
    })?;
    let format = Format::from_path(path).unwrap_or_default();
    check_located(document, &committed, format, Some(path.to_path_buf()))
}

fn check_located(
    document: &Document,
    committed: &str,
    format: Format,
    path: Option<PathBuf>,
) -> Result<(), DriftError> {
    match format {
        Format::Json => {
            let parsed = json::parse(committed).map_err(|source| DriftError::Unparseable {
                path: path.clone().unwrap_or_else(|| PathBuf::from("<input>")),
                source,
            })?;
            let generated = document.to_json();
            let mut differences = Vec::new();
            diff(&parsed, &generated, "", &mut differences);
            if differences.is_empty() {
                return Ok(());
            }
            let total = differences.len();
            differences.truncate(Drift::LIMIT);
            Err(Drift {
                path,
                differences,
                total,
            }
            .into())
        }
        Format::Yaml => {
            let generated = document.to_yaml_string();
            if committed == generated {
                return Ok(());
            }
            let differences = vec![Difference {
                pointer: String::new(),
                change: Change::Changed {
                    committed: format!("{} bytes of YAML", committed.len()),
                    generated: format!("{} bytes of YAML", generated.len()),
                },
            }];
            Err(Drift {
                path,
                differences,
                total: 1,
            }
            .into())
        }
    }
}

/// Walks two values together and records where they disagree.
fn diff(committed: &Json, generated: &Json, pointer: &str, out: &mut Vec<Difference>) {
    match (committed, generated) {
        (Json::Object(left), Json::Object(right)) => {
            for (key, value) in left {
                let child = push(pointer, key);
                match right.iter().find(|(k, _)| k == key) {
                    Some((_, other)) => diff(value, other, &child, out),
                    None => out.push(Difference {
                        pointer: child,
                        change: Change::Removed,
                    }),
                }
            }
            for (key, _) in right {
                if !left.iter().any(|(k, _)| k == key) {
                    out.push(Difference {
                        pointer: push(pointer, key),
                        change: Change::Added,
                    });
                }
            }
        }
        (Json::Array(left), Json::Array(right)) => {
            for (index, value) in left.iter().enumerate() {
                let child = push(pointer, &index.to_string());
                match right.get(index) {
                    Some(other) => diff(value, other, &child, out),
                    None => out.push(Difference {
                        pointer: child,
                        change: Change::Removed,
                    }),
                }
            }
            for index in left.len()..right.len() {
                out.push(Difference {
                    pointer: push(pointer, &index.to_string()),
                    change: Change::Added,
                });
            }
        }
        (left, right) if left == right => {}
        (left, right) => out.push(Difference {
            pointer: if pointer.is_empty() {
                "/".to_owned()
            } else {
                pointer.to_owned()
            },
            change: Change::Changed {
                committed: summary(left),
                generated: summary(right),
            },
        }),
    }
}

/// RFC 6901 escaping: `~` becomes `~0` and `/` becomes `~1`, which matters
/// here because every path key contains slashes.
fn push(pointer: &str, token: &str) -> String {
    let escaped = token.replace('~', "~0").replace('/', "~1");
    format!("{pointer}/{escaped}")
}

fn summary(value: &Json) -> String {
    match value {
        Json::Str(text) if text.chars().count() > 40 => {
            let short: String = text.chars().take(37).collect();
            format!("{short}...")
        }
        Json::Str(text) => text.clone(),
        Json::Array(items) => format!("an array of {}", items.len()),
        Json::Object(entries) => format!("an object of {}", entries.len()),
        other => other.to_json(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointers_escape_the_slashes_in_a_path_key() {
        assert_eq!(push("/paths", "/posts/{id}"), "/paths/~1posts~1{id}");
        assert_eq!(push("", "a~b"), "/a~0b");
    }

    #[test]
    fn reformatting_is_not_drift() {
        let document = Document::new("Test", "1.0.0");
        let compact = document.to_json().to_json().replace("\n  ", "\n ");
        check(&document, &compact, Format::Json).unwrap();
    }

    #[test]
    fn a_changed_value_is_located_by_pointer() {
        let document = Document::new("Test", "1.0.0");
        let committed = document.to_json_string().replace("1.0.0", "0.9.0");
        let error = check(&document, &committed, Format::Json).unwrap_err();
        let DriftError::Drifted(drift) = error else {
            panic!("expected drift");
        };
        assert_eq!(drift.total, 1);
        assert_eq!(drift.differences[0].pointer, "/info/version");
    }

    #[test]
    fn a_format_comes_from_the_extension() {
        assert_eq!(
            Format::from_path(Path::new("openapi.json")),
            Some(Format::Json)
        );
        assert_eq!(
            Format::from_path(Path::new("openapi.yaml")),
            Some(Format::Yaml)
        );
        assert_eq!(Format::from_path(Path::new("openapi")), None);
    }
}
