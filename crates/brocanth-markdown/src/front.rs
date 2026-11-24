//! Front matter: TOML between two `+++` lines, read into a serde type.
//!
//! **Why TOML and not YAML.** `serde_yaml` was deprecated by its author in
//! March 2024, and its forks publish rarely. `toml` is maintained, already
//! in this workspace, and has a date type of its own, so `date =
//! 2026-11-22` is a date and not a string to parse. Hugo and Zola write
//! TOML front matter between `+++` lines; this crate reads the same shape.

use std::fmt;

use serde::de::DeserializeOwned;

/// The fence that opens and closes the front matter.
const FENCE: &str = "+++";

/// A file, read: its front matter as `T`, and the Markdown after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Document<'a, T> {
    /// The front matter, typed.
    pub data: T,
    /// The Markdown after the closing `+++` line.
    pub body: &'a str,
    /// The line the body starts on, counted from 1, for an error that
    /// points into the body.
    pub body_line: usize,
}

/// Why a file's front matter was refused.
///
/// Its `Display` is one line with the line number when there is one, then
/// a `help:` line that says what to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontMatterError {
    /// What is wrong.
    pub message: String,
    /// The line in the file, counted from 1, when the error has one.
    pub line: Option<usize>,
    /// What to do.
    pub help: String,
}

impl fmt::Display for FrontMatterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message)?,
            None => f.write_str(&self.message)?,
        }
        write!(f, "\nhelp: {}", self.help)
    }
}

impl std::error::Error for FrontMatterError {}

/// The front matter's text, the body, and the line the body starts on,
/// unparsed.
///
/// `None` for the front matter when the file does not start with a `+++`
/// line. The body then is the whole file, from line 1.
///
/// # Errors
///
/// A file that opens a `+++` block and never closes it, and a file that
/// starts with `---`, which is YAML front matter.
pub fn split(text: &str) -> Result<(Option<&str>, &str, usize), FrontMatterError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first = text.lines().next().unwrap_or("");
    if first.trim_end() == "---" {
        return Err(FrontMatterError {
            message: "the front matter is between `---` lines, which is YAML".to_string(),
            line: Some(1),
            help: "write it as TOML between `+++` lines: `title = \"Hello\"`".to_string(),
        });
    }
    if first.trim_end() != FENCE {
        return Ok((None, text, 1));
    }
    // After the opening line, find the closing one.
    let start = first.len() + line_end(&text[first.len()..]);
    let mut offset = start;
    // The front matter starts on line 2.
    for (line, raw) in (2..).zip(text[start..].split_inclusive('\n')) {
        if raw.trim_end() == FENCE {
            let front = &text[start..offset];
            let body = &text[offset + raw.len()..];
            return Ok((Some(front), body, line + 1));
        }
        offset += raw.len();
    }
    Err(FrontMatterError {
        message: "the front matter opens with `+++` and never closes".to_string(),
        line: Some(1),
        help: "end the front matter with a line that holds only `+++`".to_string(),
    })
}

/// The length of the line break at the start of `rest`: 0, 1 or 2.
fn line_end(rest: &str) -> usize {
    if rest.starts_with("\r\n") {
        2
    } else if rest.starts_with('\n') {
        1
    } else {
        0
    }
}

/// Reads `text`: the front matter into `T`, and the body after it.
///
/// A file with no front matter reads as an empty table, so a `T` whose
/// fields all have defaults takes it; any other `T` fails on its first
/// required field, and the help says how to start the front matter.
///
/// An unknown key fails only when `T` says `#[serde(deny_unknown_fields)]`.
/// Say it: a misspelt `titel` is then an error and not a missing title.
///
/// # Errors
///
/// [`FrontMatterError`] with the line of the bad key or value.
pub fn front_matter<T: DeserializeOwned>(text: &str) -> Result<Document<'_, T>, FrontMatterError> {
    let (front, body, body_line) = split(text)?;
    let source = front.unwrap_or("");
    match toml::from_str::<T>(source) {
        Ok(data) => Ok(Document {
            data,
            body,
            body_line,
        }),
        Err(e) => {
            // The front matter starts on line 2: line 1 is the fence.
            let line = e
                .span()
                .map(|span| source[..span.start.min(source.len())].matches('\n').count() + 2);
            let message = e.message().trim_end().to_string();
            let help = if front.is_none() {
                "start the file with a `+++` line, the TOML fields, and a closing `+++` line"
                    .to_string()
            } else if let Some(field) = message.strip_prefix("missing field ") {
                format!(
                    "add {field} to the front matter, or make it an `Option` or \
                     `#[serde(default)]` in the type"
                )
            } else if message.starts_with("unknown field") {
                "remove the key, or correct its spelling to one of the fields listed".to_string()
            } else {
                "correct the value to the type the field declares".to_string()
            };
            // A missing field has no line of its own: toml points at the
            // whole table.
            let line = if front.is_none() || message.starts_with("missing field") {
                None
            } else {
                line
            };
            Err(FrontMatterError {
                message,
                line,
                help,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize, Debug, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Post {
        title: String,
        #[serde(default)]
        tags: Vec<String>,
    }

    #[test]
    fn the_front_matter_is_typed_and_the_body_follows() {
        let doc =
            front_matter::<Post>("+++\ntitle = \"Hi\"\ntags = [\"a\"]\n+++\n# Body\n").unwrap();
        assert_eq!(doc.data.title, "Hi");
        assert_eq!(doc.data.tags, ["a"]);
        assert_eq!(doc.body, "# Body\n");
        assert_eq!(doc.body_line, 5);
    }

    #[test]
    fn crlf_line_ends_are_read_too() {
        let doc = front_matter::<Post>("+++\r\ntitle = \"Hi\"\r\n+++\r\nBody\r\n").unwrap();
        assert_eq!(doc.data.title, "Hi");
        assert_eq!(doc.body, "Body\r\n");
    }

    #[test]
    fn a_missing_field_names_the_field_and_says_what_to_do() {
        let e = front_matter::<Post>("+++\ntags = []\n+++\n").unwrap_err();
        let text = e.to_string();
        assert!(text.contains("missing field `title`"), "{text}");
        assert!(text.contains("\nhelp: "), "{text}");
    }

    #[test]
    fn a_misspelt_key_points_at_its_line() {
        let e = front_matter::<Post>("+++\ntitle = \"x\"\ntitel = \"y\"\n+++\n").unwrap_err();
        assert_eq!(e.line, Some(3), "{e}");
        assert!(e.message.contains("unknown field"), "{e}");
    }

    #[test]
    fn a_value_of_the_wrong_type_points_at_its_line() {
        let e = front_matter::<Post>("+++\ntitle = 7\n+++\n").unwrap_err();
        assert_eq!(e.line, Some(2), "{e}");
    }

    #[test]
    fn yaml_front_matter_is_refused_with_the_toml_spelling() {
        let e = front_matter::<Post>("---\ntitle: x\n---\n").unwrap_err();
        assert!(e.to_string().contains("help: write it as TOML"), "{e}");
    }

    #[test]
    fn an_unclosed_block_is_refused() {
        let e = front_matter::<Post>("+++\ntitle = \"x\"\n").unwrap_err();
        assert!(e.message.contains("never closes"), "{e}");
    }

    #[test]
    fn no_front_matter_says_how_to_start_one() {
        let e = front_matter::<Post>("# Just a body\n").unwrap_err();
        assert!(e.help.contains("`+++`"), "{e}");
        assert_eq!(e.line, None);
    }
}
