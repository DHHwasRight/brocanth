//! Strings: length, pattern, the three named formats, and the three
//! substring checks.
//!
//! Lengths count **characters, not bytes**, because a form that says "at most
//! 20 characters" and then rejects twenty accented ones is wrong in the way a
//! user cannot diagnose.

use crate::issue::{count, quote_received, Issues, Path};
use crate::node::{SchemaNode, StringFormat, StringNode};
use crate::schema::Schema;
use crate::IssueCode;

/// A string schema. Built by [`string`].
#[derive(Clone, Debug, Default)]
pub struct StringSchema {
    node: StringNode,
    #[cfg(feature = "pattern")]
    regex: Option<regex::Regex>,
}

/// A string schema with no constraints yet.
///
/// ```
/// use brocanth_schema::{string, Schema};
///
/// let username = string().min_length(3).max_length(32);
/// assert!(username.validate("ada").is_ok());
/// assert!(username.validate("ad").is_err());
/// ```
#[must_use]
pub fn string() -> StringSchema {
    StringSchema::default()
}

impl StringSchema {
    /// At least `n` characters.
    #[must_use]
    pub fn min_length(mut self, n: usize) -> Self {
        self.node.min_length = Some(n);
        self
    }

    /// At most `n` characters.
    #[must_use]
    pub fn max_length(mut self, n: usize) -> Self {
        self.node.max_length = Some(n);
        self
    }

    /// Exactly `n` characters.
    #[must_use]
    pub fn length(mut self, n: usize) -> Self {
        self.node.exact_length = Some(n);
        self
    }

    /// At least one character. The same as `min_length(1)`, said the way a
    /// form means it.
    #[must_use]
    pub fn non_empty(self) -> Self {
        self.min_length(1)
    }

    /// The whole value must match `re`.
    ///
    /// Needs the `pattern` feature, which is on by default. Without it this
    /// method does not exist and the compiler says so, rather than the regex
    /// being accepted and never run.
    ///
    /// # Panics
    ///
    /// If `re` is not a valid regular expression. A schema is built once, at
    /// startup, from a literal the author wrote; this is the same class of
    /// failure as a malformed route template. Use [`StringSchema::try_pattern`]
    /// when the pattern comes from data.
    #[cfg(feature = "pattern")]
    #[must_use]
    pub fn pattern(self, re: &str) -> Self {
        match self.try_pattern(re) {
            Ok(schema) => schema,
            Err(err) => panic!("invalid pattern `{re}` in a string schema: {err}"),
        }
    }

    /// [`StringSchema::pattern`], for a pattern that is not a literal.
    ///
    /// # Errors
    ///
    /// The regex engine's own compilation error.
    #[cfg(feature = "pattern")]
    pub fn try_pattern(mut self, re: &str) -> Result<Self, regex::Error> {
        let anchored = anchor(re);
        let compiled = regex::Regex::new(&anchored)?;
        self.node.pattern = Some(re.to_string());
        self.regex = Some(compiled);
        Ok(self)
    }

    /// An email address.
    ///
    /// The rule is deliberately the pragmatic one — one `@`, a local part, a
    /// dotted domain — and not RFC 5322. An address this rejects is one a
    /// mail provider would reject too; an address this accepts still has to
    /// receive the confirmation email, which is the only real check there is.
    #[must_use]
    pub fn email(mut self) -> Self {
        self.node.format = Some(StringFormat::Email);
        self
    }

    /// An absolute URL: a scheme, `://`, and a host.
    #[must_use]
    pub fn url(mut self) -> Self {
        self.node.format = Some(StringFormat::Url);
        self
    }

    /// A hyphenated UUID, any version.
    #[must_use]
    pub fn uuid(mut self) -> Self {
        self.node.format = Some(StringFormat::Uuid);
        self
    }

    /// Must begin with `prefix`.
    #[must_use]
    pub fn starts_with(mut self, prefix: impl Into<String>) -> Self {
        self.node.starts_with = Some(prefix.into());
        self
    }

    /// Must end with `suffix`.
    #[must_use]
    pub fn ends_with(mut self, suffix: impl Into<String>) -> Self {
        self.node.ends_with = Some(suffix.into());
        self
    }

    /// Must contain `needle` somewhere.
    #[must_use]
    pub fn contains(mut self, needle: impl Into<String>) -> Self {
        self.node.contains = Some(needle.into());
        self
    }
}

impl Schema for StringSchema {
    type Input = str;
    type Output = String;

    fn describe(&self) -> SchemaNode {
        SchemaNode::String(self.node.clone())
    }

    fn check(&self, input: &str, at: &mut Path, issues: &mut Issues) {
        let len = input.chars().count();

        if let Some(min) = self.node.min_length {
            if len < min {
                issues.report(at, IssueCode::TooShort, |i| {
                    i.expected(format!("at least {}", count(min, "character", "characters")))
                        .received(count(len, "character", "characters"))
                });
            }
        }
        if let Some(max) = self.node.max_length {
            if len > max {
                issues.report(at, IssueCode::TooLong, |i| {
                    i.expected(format!("at most {}", count(max, "character", "characters")))
                        .received(count(len, "character", "characters"))
                });
            }
        }
        if let Some(exact) = self.node.exact_length {
            if len != exact {
                issues.report(at, IssueCode::WrongLength, |i| {
                    i.expected(format!("exactly {}", count(exact, "character", "characters")))
                        .received(count(len, "character", "characters"))
                });
            }
        }

        #[cfg(feature = "pattern")]
        if let (Some(re), Some(source)) = (self.regex.as_ref(), self.node.pattern.as_ref()) {
            if !re.is_match(input) {
                issues.report(at, IssueCode::Pattern, |i| {
                    i.expected(format!("a value matching {source}"))
                        .received(quote_received(input))
                        .message(format!("must match {source}"))
                });
            }
        }

        if let Some(format) = self.node.format {
            let ok = match format {
                StringFormat::Email => is_email(input),
                StringFormat::Url => is_url(input),
                StringFormat::Uuid => is_uuid(input),
            };
            if !ok {
                let expected = match format {
                    StringFormat::Email => "an email address",
                    StringFormat::Url => "an absolute URL",
                    StringFormat::Uuid => "a UUID",
                };
                issues.report(at, IssueCode::Format, |i| {
                    i.expected(expected).received(quote_received(input))
                });
            }
        }

        if let Some(prefix) = &self.node.starts_with {
            if !input.starts_with(prefix.as_str()) {
                issues.report(at, IssueCode::StartsWith, |i| {
                    i.expected(format!("a value starting with {prefix:?}"))
                        .received(quote_received(input))
                        .message(format!("must start with {prefix:?}"))
                });
            }
        }
        if let Some(suffix) = &self.node.ends_with {
            if !input.ends_with(suffix.as_str()) {
                issues.report(at, IssueCode::EndsWith, |i| {
                    i.expected(format!("a value ending with {suffix:?}"))
                        .received(quote_received(input))
                        .message(format!("must end with {suffix:?}"))
                });
            }
        }
        if let Some(needle) = &self.node.contains {
            if !input.contains(needle.as_str()) {
                issues.report(at, IssueCode::Contains, |i| {
                    i.expected(format!("a value containing {needle:?}"))
                        .received(quote_received(input))
                        .message(format!("must contain {needle:?}"))
                });
            }
        }
    }

    fn parse_at(&self, input: &str, at: &mut Path, issues: &mut Issues) -> Option<String> {
        let before = issues.len();
        self.check(input, at, issues);
        (issues.len() == before).then(|| input.to_string())
    }
}

// ---------------------------------------------------------------------------
// The formats
// ---------------------------------------------------------------------------

/// Wrap a pattern so it must match the whole value.
///
/// zod's `.regex()` matches anywhere in the string, which means
/// `/^[a-z]+$/`-shaped intent written without the anchors silently accepts
/// `"ab!"`. Anchoring here is the safer default and the one a form author
/// means; write `.*` explicitly for a partial match.
#[cfg(feature = "pattern")]
fn anchor(re: &str) -> String {
    let body = re.strip_prefix('^').unwrap_or(re);
    let body = body.strip_suffix('$').unwrap_or(body);
    format!("^(?:{body})$")
}

fn is_email(value: &str) -> bool {
    if value.chars().any(char::is_whitespace) || value.len() > 254 {
        return false;
    }
    let mut parts = value.split('@');
    let (Some(local), Some(domain), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if local.is_empty() || local.len() > 64 || domain.is_empty() {
        return false;
    }
    if domain.starts_with(['.', '-']) || domain.ends_with(['.', '-']) || domain.contains("..") {
        return false;
    }
    let labels: Vec<&str> = domain.split('.').collect();
    if labels.len() < 2 {
        return false;
    }
    labels.last().is_some_and(|tld| {
        tld.len() >= 2 && tld.chars().all(|c| c.is_ascii_alphabetic())
    }) && labels
        .iter()
        .all(|l| !l.is_empty() && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

fn is_url(value: &str) -> bool {
    if value.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    if scheme.is_empty() || !scheme.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return false;
    }
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return false;
    }
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    !host.is_empty()
}

fn is_uuid(value: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let mut parts = value.split('-');
    for expected in groups {
        match parts.next() {
            Some(part)
                if part.len() == expected && part.chars().all(|c| c.is_ascii_hexdigit()) => {}
            _ => return false,
        }
    }
    parts.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_counts_characters_not_bytes() {
        let schema = string().max_length(3);
        assert!(schema.validate("éàü").is_ok());
        assert!(schema.validate("éàüx").is_err());
    }

    #[test]
    fn emails_the_pragmatic_rule_accepts_and_rejects() {
        let schema = string().email();
        for good in ["a@b.co", "first.last+tag@sub.example.com"] {
            assert!(schema.validate(good).is_ok(), "rejected {good}");
        }
        for bad in ["", "a@b", "a@@b.co", "a b@c.co", "@b.co", "a@.co", "a@b..co"] {
            assert!(schema.validate(bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn urls_need_a_scheme_and_a_host() {
        let schema = string().url();
        for good in ["https://example.com", "ftp://h/p?q#f", "https://u@h/x"] {
            assert!(schema.validate(good).is_ok(), "rejected {good}");
        }
        for bad in ["example.com", "https://", "://h", "1http://h", "http://h i"] {
            assert!(schema.validate(bad).is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn uuids_are_hyphenated_hex() {
        let schema = string().uuid();
        assert!(schema.validate("3f2504e0-4f89-11d3-9a0c-0305e82c3301").is_ok());
        assert!(schema.validate("3f2504e04f8911d39a0c0305e82c3301").is_err());
        assert!(schema
            .validate("3f2504e0-4f89-11d3-9a0c-0305e82c3301-x")
            .is_err());
    }

    #[cfg(feature = "pattern")]
    #[test]
    fn patterns_match_the_whole_value() {
        let schema = string().pattern("[a-z]+");
        assert!(schema.validate("abc").is_ok());
        assert!(schema.validate("ab!").is_err());
    }
}
