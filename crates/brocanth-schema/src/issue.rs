//! Where a value failed, what was expected, and what arrived.
//!
//! Three rules shape this module.
//!
//! **Every issue names its place.** The path is dotted and carries array
//! indices — `notes[0].kind` — because "kind is invalid" is useless when the
//! form has four of them.
//!
//! **Every validation reports every issue.** [`Issues`] is a list, never an
//! early return. A form that reports one error at a time is how a user ends up
//! submitting six times.
//!
//! **Every issue is readable twice.** [`Issue::message`] is the sentence that
//! goes under the input; [`Issue::expected`] and [`Issue::received`] are the
//! machine-readable halves a JSON client wants. See [`Issues::form_report`] and
//! [`Issues::to_json`].

use core::fmt;
use std::borrow::Cow;

// ---------------------------------------------------------------------------
// Path
// ---------------------------------------------------------------------------

/// One step on the way to a value.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Segment {
    /// A named field, rendered `.name` (or `name` at the head of the path).
    Field(Cow<'static, str>),
    /// An array index, rendered `[0]`.
    Index(usize),
}

/// The place a value sits inside the thing being validated.
///
/// A validator carries one `Path` down the tree and pushes and pops as it
/// descends, so a deep structure costs one allocation rather than one per
/// field. Use [`Path::with_field`] and [`Path::with_index`] and the pop cannot
/// be forgotten.
///
/// ```
/// use brocanth_schema::Path;
///
/// let mut at = Path::root();
/// assert_eq!(at.dotted(), "");
///
/// at.with_field("notes", |at| {
///     at.with_index(0, |at| {
///         at.with_field("kind", |at| assert_eq!(at.dotted(), "notes[0].kind"));
///     });
/// });
/// assert_eq!(at.dotted(), "");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Path {
    segments: Vec<Segment>,
}

impl Path {
    /// The empty path: the value being validated, not a part of it.
    #[must_use]
    pub fn root() -> Self {
        Self::default()
    }

    /// `true` when this is the value itself rather than a part of it.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// Every step, outermost first.
    #[must_use]
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    /// Descend into a named field.
    pub fn push_field(&mut self, name: impl Into<Cow<'static, str>>) {
        self.segments.push(Segment::Field(name.into()));
    }

    /// Descend into an array element.
    pub fn push_index(&mut self, index: usize) {
        self.segments.push(Segment::Index(index));
    }

    /// Climb back one step. Does nothing at the root.
    pub fn pop(&mut self) {
        self.segments.pop();
    }

    /// Run `f` one field deeper, then climb back.
    pub fn with_field<R>(
        &mut self,
        name: impl Into<Cow<'static, str>>,
        f: impl FnOnce(&mut Path) -> R,
    ) -> R {
        self.push_field(name);
        let out = f(self);
        self.pop();
        out
    }

    /// Run `f` one element deeper, then climb back.
    pub fn with_index<R>(&mut self, index: usize, f: impl FnOnce(&mut Path) -> R) -> R {
        self.push_index(index);
        let out = f(self);
        self.pop();
        out
    }

    /// The dotted rendering: `notes[0].kind`, or `""` at the root.
    #[must_use]
    pub fn dotted(&self) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            match segment {
                Segment::Field(name) => {
                    if !out.is_empty() {
                        out.push('.');
                    }
                    out.push_str(name);
                }
                Segment::Index(i) => {
                    out.push('[');
                    out.push_str(itoa(*i).as_str());
                    out.push(']');
                }
            }
        }
        out
    }
}

fn itoa(n: usize) -> String {
    n.to_string()
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.dotted())
    }
}

// ---------------------------------------------------------------------------
// Issue codes
// ---------------------------------------------------------------------------

/// Which constraint failed.
///
/// A client switches on this rather than on a message, so the message stays
/// free to change and to be translated.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum IssueCode {
    /// The value is not of the kind the schema describes at all.
    WrongType,
    /// A required field was absent.
    Missing,
    /// A key the object schema does not declare, under `UnknownKeys::Reject`.
    UnknownKey,
    /// Shorter than `min_length` / fewer than `min_items`.
    TooShort,
    /// Longer than `max_length` / more than `max_items`.
    TooLong,
    /// Not the exact length the schema demands.
    WrongLength,
    /// The regular expression did not match.
    Pattern,
    /// Not an email address, URL or UUID, as the schema demanded.
    Format,
    /// Missing the required prefix.
    StartsWith,
    /// Missing the required suffix.
    EndsWith,
    /// Missing the required substring.
    Contains,
    /// Below `min` or at or below `exclusive_min`.
    TooSmall,
    /// Above `max` or at or above `exclusive_max`.
    TooLarge,
    /// Not a multiple of the step.
    NotMultipleOf,
    /// A whole number was required and the value has a fractional part.
    NotInteger,
    /// `NaN` or an infinity where a finite number was required.
    NotFinite,
    /// The array repeats an element.
    NotUnique,
    /// Not one of the declared variants.
    NotInEnum,
    /// Not the declared literal.
    NotLiteral,
    /// No branch of the union accepted the value.
    NoUnionVariant,
    /// A [`refine`](crate::Schema::refine) predicate said no.
    Refused(Cow<'static, str>),
}

impl IssueCode {
    /// The wire form: `too_short`, `not_in_enum`, or the custom code a
    /// `refine` was given.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            IssueCode::WrongType => "wrong_type",
            IssueCode::Missing => "missing",
            IssueCode::UnknownKey => "unknown_key",
            IssueCode::TooShort => "too_short",
            IssueCode::TooLong => "too_long",
            IssueCode::WrongLength => "wrong_length",
            IssueCode::Pattern => "pattern",
            IssueCode::Format => "format",
            IssueCode::StartsWith => "starts_with",
            IssueCode::EndsWith => "ends_with",
            IssueCode::Contains => "contains",
            IssueCode::TooSmall => "too_small",
            IssueCode::TooLarge => "too_large",
            IssueCode::NotMultipleOf => "not_multiple_of",
            IssueCode::NotInteger => "not_integer",
            IssueCode::NotFinite => "not_finite",
            IssueCode::NotUnique => "not_unique",
            IssueCode::NotInEnum => "not_in_enum",
            IssueCode::NotLiteral => "not_literal",
            IssueCode::NoUnionVariant => "no_union_variant",
            IssueCode::Refused(code) => code,
        }
    }
}

impl fmt::Display for IssueCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Issue
// ---------------------------------------------------------------------------

/// One failure, at one place.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Issue {
    /// Dotted path including array indices, `""` for the value itself.
    pub path: String,
    /// The constraint that failed.
    pub code: IssueCode,
    /// What the schema demanded, as a noun phrase: `at least 3 characters`.
    pub expected: String,
    /// What arrived, rendered short: `2 characters`, `"nope"`, `nothing`.
    pub received: String,
    /// The sentence to print under the input: `must be at least 3 characters`.
    pub message: String,
}

impl Issue {
    /// Start an issue at `at`. Fill it with [`Issue::expected`] and
    /// [`Issue::received`]; the message defaults to `must be {expected}`.
    #[must_use]
    pub fn at(at: &Path, code: IssueCode) -> Self {
        Self {
            path: at.dotted(),
            code,
            expected: String::new(),
            received: String::new(),
            message: String::new(),
        }
    }

    /// What the schema demanded.
    #[must_use]
    pub fn expected(mut self, expected: impl Into<String>) -> Self {
        self.expected = expected.into();
        self
    }

    /// What arrived.
    #[must_use]
    pub fn received(mut self, received: impl Into<String>) -> Self {
        self.received = received.into();
        self
    }

    /// Override the default `must be {expected}` sentence.
    #[must_use]
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// Fill in the default message if nobody set one.
    #[must_use]
    pub fn finish(mut self) -> Self {
        if self.message.is_empty() {
            self.message = format!("must be {}", self.expected);
        }
        self
    }

    /// `expected a, received b` — the long form, for a log line.
    #[must_use]
    pub fn detail(&self) -> String {
        format!("expected {}, received {}", self.expected, self.received)
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

// ---------------------------------------------------------------------------
// Issues
// ---------------------------------------------------------------------------

/// Everything one validation found, in the order the schema walked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Issues {
    items: Vec<Issue>,
}

impl Issues {
    /// An empty report.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a failure. Call [`Issue::finish`] first, or use
    /// [`Issues::report`].
    pub fn push(&mut self, issue: Issue) {
        self.items.push(issue.finish());
    }

    /// Record a failure built in place.
    pub fn report(&mut self, at: &Path, code: IssueCode, build: impl FnOnce(Issue) -> Issue) {
        self.push(build(Issue::at(at, code)));
    }

    /// `true` when nothing failed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many failures.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Every failure, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, Issue> {
        self.items.iter()
    }

    /// Append another report, keeping both orders.
    pub fn merge(&mut self, other: Issues) {
        self.items.extend(other.items);
    }

    /// Every failure at exactly `path`.
    ///
    /// This is the call a form builder makes per input: `notes[0].kind`, not
    /// `notes`.
    #[must_use]
    pub fn at(&self, path: &str) -> Vec<&Issue> {
        self.items.iter().filter(|i| i.path == path).collect()
    }

    /// The sentences to print under the input at `path`. Empty when it passed.
    #[must_use]
    pub fn messages_at(&self, path: &str) -> Vec<&str> {
        self.items
            .iter()
            .filter(|i| i.path == path)
            .map(|i| i.message.as_str())
            .collect()
    }

    /// `true` when something failed at `path`.
    #[must_use]
    pub fn has(&self, path: &str) -> bool {
        self.items.iter().any(|i| i.path == path)
    }

    /// `true` when `path` failed with this code.
    #[must_use]
    pub fn has_code(&self, path: &str, code: &IssueCode) -> bool {
        self.items
            .iter()
            .any(|i| i.path == path && &i.code == code)
    }

    /// Every path that failed, first-seen order, no repeats.
    #[must_use]
    pub fn paths(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for issue in &self.items {
            if !out.contains(&issue.path.as_str()) {
                out.push(&issue.path);
            }
        }
        out
    }

    /// Grouped by path, first-seen order — a form renderer walks this once and
    /// prints each group under its input.
    ///
    /// ```
    /// use brocanth_schema::{object, string, Schema, Value};
    ///
    /// let schema = object()
    ///     .field("name", string().min_length(2))
    ///     .field("email", string().email());
    ///
    /// let bad = Value::object([("name", Value::text("a")), ("email", Value::text("nope"))]);
    /// let issues = schema.validate(&bad).unwrap_err();
    ///
    /// assert_eq!(
    ///     issues.form_report(),
    ///     vec![
    ///         ("name", vec!["must be at least 2 characters"]),
    ///         ("email", vec!["must be an email address"]),
    ///     ]
    /// );
    /// ```
    #[must_use]
    pub fn form_report(&self) -> Vec<(&str, Vec<&str>)> {
        self.paths()
            .into_iter()
            .map(|path| (path, self.messages_at(path)))
            .collect()
    }

    /// The JSON body of a 422, built without a serialiser.
    ///
    /// ```text
    /// {"error":"validation_failed","issues":[
    ///   {"path":"notes[0].kind","code":"not_in_enum",
    ///    "expected":"one of \"todo\", \"done\"","received":"\"maybe\"",
    ///    "message":"must be one of \"todo\", \"done\""}]}
    /// ```
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut out = String::from("{\"error\":\"validation_failed\",\"issues\":[");
        for (n, issue) in self.items.iter().enumerate() {
            if n > 0 {
                out.push(',');
            }
            out.push_str("{\"path\":");
            push_json_string(&mut out, &issue.path);
            out.push_str(",\"code\":");
            push_json_string(&mut out, issue.code.as_str());
            out.push_str(",\"expected\":");
            push_json_string(&mut out, &issue.expected);
            out.push_str(",\"received\":");
            push_json_string(&mut out, &issue.received);
            out.push_str(",\"message\":");
            push_json_string(&mut out, &issue.message);
            out.push('}');
        }
        out.push_str("]}");
        out
    }
}

impl<'a> IntoIterator for &'a Issues {
    type Item = &'a Issue;
    type IntoIter = std::slice::Iter<'a, Issue>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

impl IntoIterator for Issues {
    type Item = Issue;
    type IntoIter = std::vec::IntoIter<Issue>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

impl FromIterator<Issue> for Issues {
    fn from_iter<T: IntoIterator<Item = Issue>>(iter: T) -> Self {
        Self {
            items: iter.into_iter().map(Issue::finish).collect(),
        }
    }
}

impl fmt::Display for Issues {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, issue) in self.items.iter().enumerate() {
            if n > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Issues {}

// ---------------------------------------------------------------------------
// Rendering helpers, shared by every schema
// ---------------------------------------------------------------------------

/// Append `value` to `out` as a quoted, escaped JSON string.
pub fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A string as it appears in an issue: quoted, and cut at 40 characters so a
/// rejected 2 MB body does not become a 2 MB error.
#[must_use]
pub fn quote_received(value: &str) -> String {
    const LIMIT: usize = 40;
    let truncated: String = value.chars().take(LIMIT).collect();
    if truncated.chars().count() < value.chars().count() {
        format!("\"{truncated}…\"")
    } else {
        format!("\"{truncated}\"")
    }
}

/// `1 character` / `3 characters`, so messages do not say `1 characters`.
#[must_use]
pub fn count(n: usize, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// A number as it appears in an issue, with no trailing `.0` noise.
#[must_use]
pub fn render_f64(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotted_path_carries_indices() {
        let mut at = Path::root();
        at.push_field("notes");
        at.push_index(2);
        at.push_field("kind");
        assert_eq!(at.dotted(), "notes[2].kind");
        at.pop();
        assert_eq!(at.dotted(), "notes[2]");
    }

    #[test]
    fn an_index_at_the_head_still_renders() {
        let mut at = Path::root();
        at.push_index(0);
        at.push_field("name");
        assert_eq!(at.dotted(), "[0].name");
    }

    #[test]
    fn json_escapes_what_a_user_typed() {
        let mut issues = Issues::new();
        issues.report(&Path::root(), IssueCode::Format, |i| {
            i.expected("an email address")
                .received(quote_received("a\"b\nc"))
        });
        let json = issues.to_json();
        assert!(json.contains(r#""received":"\"a\"b\nc\"""#), "{json}");
    }

    #[test]
    fn received_is_truncated() {
        let long = "x".repeat(100);
        let rendered = quote_received(&long);
        assert!(rendered.ends_with("…\""));
        assert_eq!(rendered.chars().count(), 43);
    }
}
