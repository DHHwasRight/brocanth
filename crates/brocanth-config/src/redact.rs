//! What a configuration must not print, even to the person who may read it.
//!
//! This crate's whole value is that it can show you the effective
//! configuration with the origin of every key. That is also the one way it can
//! hurt you: a `[database] url` with a password in it, printed into a terminal,
//! a ticket, a log line or a backoffice tab, has been disclosed four times.
//! `lib.rs` already declines to manage secrets for exactly this reason. The
//! reason does not stop a deployment putting one in a file anyway, so this
//! module is the second half of that decision: a secret that reaches the
//! document does not reach a reader.
//!
//! # The rule, and why it is two rules
//!
//! **By name.** A key whose dotted path contains a segment like `password`,
//! `secret`, `token` or `credential` is hidden whole. The segments are matched
//! over the *whole* path and not only the leaf, so `[secrets] anything` is
//! covered by its section name alone.
//!
//! **By shape.** A string that is a URL with a password in its authority has
//! that password — and only that password — replaced:
//!
//! ```text
//! postgres://app:hunter2@db.internal:5432/app
//! postgres://app:[redacted]@db.internal:5432/app
//! ```
//!
//! The second rule is not a refinement of the first, it is the reason there
//! are two. The name rule does not catch `[database] url`, because the key is
//! called `url`; and the disaster this module exists to prevent is a database
//! URL with a password in it. Only the password goes: the scheme, the user,
//! the host, the port and the database are the answer to *"which database is
//! this process actually pointed at"*, which is the question the settings tab
//! exists to answer, and masking the whole string destroys it.
//!
//! # Why not a marker in the file
//!
//! `secret = true` beside the key, or a `!secret` tag, fails open. A file that
//! forgets the marker leaks, and the file that forgets it is the file somebody
//! wrote in a hurry during an incident. It also cannot mark a value that never
//! appears in a file — an environment variable, or a plugin's own default —
//! and a marker key is itself a key, so it would show up in the document it is
//! annotating.
//!
//! # Why not the plugin's type
//!
//! A `Secret<String>` newtype in a plugin's configuration, serialising to a
//! marked value, is the better long-term answer for the keys a plugin owns:
//! it is checked by the compiler and it cannot drift. It cannot reach the keys
//! nobody owns. A `[database]` section in a file, in a build where no plugin
//! has claimed `database`, has no Rust type anywhere to carry the marking, and
//! that section is exactly where a connection URL lives. The door is open: a
//! marked value would be a third rule here, not a replacement for these two.
//!
//! # It over-redacts on purpose
//!
//! `[cache] key_prefix` is not a secret and `[server] signing_key` is. The two
//! are told apart by one refinement and no more: `key` and `keys` count only
//! as the **last** segment of a path. Beyond that the list is deliberately
//! generous, because a hidden value costs a reader one question and a printed
//! one cannot be taken back. Every hidden value says which rule hid it, so the
//! answer to a false positive is to rename the key, and the reader can see
//! that is what happened.

use crate::value::{Value, ValueKind};

/// What is printed in place of something hidden.
pub const MASK: &str = "[redacted]";

/// Path segments that mean "this value is a secret", matched anywhere in a
/// key's dotted path.
const SECRET_ANYWHERE: &[&str] = &[
    "password",
    "passwords",
    "passwd",
    "passphrase",
    "pwd",
    "secret",
    "secrets",
    "token",
    "tokens",
    "credential",
    "credentials",
    "apikey",
    "privatekey",
    "salt",
    "seed",
    "signature",
    "authorization",
    "dsn",
];

/// Segments that mean "secret" only as the **last** segment of a path.
///
/// `signing_key` is a secret and `key_prefix` is a cache setting. One
/// refinement, and it is the only one: everything else on the list above is
/// a noun that is the secret rather than a noun secrets are described with.
const SECRET_AT_THE_END: &[&str] = &["key", "keys"];

/// Why a value was hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Reason {
    /// The key is named like a secret.
    KeyName,
    /// The value is a URL and its authority carried a password.
    UrlPassword,
}

impl Reason {
    /// A clause a screen or a dump can print after the value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Reason::KeyName => "hidden: the key is named like a secret",
            Reason::UrlPassword => "hidden: the URL carried a password",
        }
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One value, as it may be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shown {
    /// What to print. Never contains the hidden part.
    pub text: String,
    /// Which rule hid something, or `None` when the text is the whole value.
    pub reason: Option<Reason>,
}

impl Shown {
    /// Whether anything was taken out.
    #[must_use]
    pub fn is_redacted(&self) -> bool {
        self.reason.is_some()
    }
}

impl std::fmt::Display for Shown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

/// The rule a [`Dump`](crate::Dump) or a settings screen applies before it
/// prints a value.
///
/// [`Redaction::standard`] is what everything here uses unless it is told
/// otherwise.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Redaction {
    extra: Vec<String>,
    off: bool,
}

impl Redaction {
    /// The rule described in the module documentation.
    #[must_use]
    pub fn standard() -> Self {
        Self::default()
    }

    /// No redaction at all.
    ///
    /// For a test that asserts on the value, and for a program whose output
    /// an operator has already decided is as privileged as the files. It is
    /// spelled out rather than reached by leaving an argument off, because a
    /// dump that prints secrets should be a line somebody wrote.
    #[must_use]
    pub fn none() -> Self {
        Self {
            extra: Vec::new(),
            off: true,
        }
    }

    /// One more path segment that means "secret", for an application whose
    /// own vocabulary this list does not know: `.also("sas")`, `.also("pin")`.
    ///
    /// Matched the same way as the built-in segments: case-insensitively,
    /// anywhere in the path, on `.`, `_` and `-` boundaries.
    #[must_use]
    pub fn also(mut self, segment: impl Into<String>) -> Self {
        self.extra.push(segment.into().to_ascii_lowercase());
        self
    }

    /// Whether this rule hides a key by its name alone.
    #[must_use]
    pub fn hides(&self, path: &str) -> bool {
        if self.off {
            return false;
        }
        let segments: Vec<String> = path
            .split(['.', '_', '-'])
            .filter(|s| !s.is_empty())
            .map(str::to_ascii_lowercase)
            .collect();
        let last = segments.last().map(String::as_str);
        segments
            .iter()
            .any(|s| SECRET_ANYWHERE.contains(&s.as_str()) || self.extra.contains(s))
            || last.is_some_and(|s| SECRET_AT_THE_END.contains(&s))
    }

    /// One value, ready to print.
    ///
    /// `path` is the key's dotted path, which is what the name rule reads.
    #[must_use]
    pub fn show(&self, path: &str, value: &Value) -> Shown {
        if self.hides(path) {
            return Shown {
                text: MASK.to_string(),
                reason: Some(Reason::KeyName),
            };
        }
        if self.off {
            return Shown {
                text: value.display(),
                reason: None,
            };
        }
        self.show_kind(value.kind())
    }

    fn show_kind(&self, kind: &ValueKind) -> Shown {
        match kind {
            ValueKind::String(s) => match mask_url_password(s) {
                Some(masked) => Shown {
                    text: format!("{masked:?}"),
                    reason: Some(Reason::UrlPassword),
                },
                None => Shown {
                    text: format!("{s:?}"),
                    reason: None,
                },
            },
            // An array is one value to the merge and one value here, so it is
            // scanned element by element and rejoined. A list of connection
            // URLs is the ordinary way this arrives.
            ValueKind::Array(items) => {
                let parts: Vec<Shown> = items.iter().map(|v| self.show_kind(v.kind())).collect();
                let reason = parts.iter().find_map(|p| p.reason);
                Shown {
                    text: format!(
                        "[{}]",
                        parts
                            .iter()
                            .map(|p| p.text.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    reason,
                }
            }
            other => Shown {
                text: Value::new(other.clone(), crate::value::Provenance::inline("shown"))
                    .display(),
                reason: None,
            },
        }
    }
}

/// Replaces the password in a URL's authority, and answers `None` when there
/// is nothing to replace.
///
/// Hand-written rather than taken from the `url` crate, which this crate does
/// not depend on and should not start depending on to blank out one span. The
/// parse is deliberately shallow: it looks for `://`, takes the authority up
/// to the next `/`, `?` or `#`, and splits the userinfo at the **last** `@`
/// and the password at the **first** `:` after it — which is what RFC 3986
/// says and what every database driver does.
fn mask_url_password(text: &str) -> Option<String> {
    let scheme_end = text.find("://")?;
    let authority_start = scheme_end + 3;
    let rest = &text[authority_start..];
    let authority_len = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_len];
    let at = authority.rfind('@')?;
    let userinfo = &authority[..at];
    let colon = userinfo.find(':')?;
    if colon + 1 >= userinfo.len() {
        // `user:@host` — an empty password is not a secret, and blanking it
        // would invent one.
        return None;
    }
    Some(format!(
        "{}{}{}{MASK}{}",
        &text[..authority_start],
        &userinfo[..colon],
        ":",
        &text[authority_start + at..]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Provenance;

    fn string(s: &str) -> Value {
        Value::new(ValueKind::String(s.to_string()), Provenance::inline("test"))
    }

    fn integer(n: i64) -> Value {
        Value::new(ValueKind::Integer(n), Provenance::inline("test"))
    }

    #[test]
    fn a_key_named_like_a_secret_is_hidden_whole() {
        let r = Redaction::standard();
        for path in [
            "database.password",
            "smtp.PASSWORD",
            "auth.signing_key",
            "auth.api_key",
            "s3.secret_access_key",
            "secrets.anything_at_all",
            "stripe.token",
            "server.tls.private_key",
            "db.credentials.user",
        ] {
            assert!(r.hides(path), "`{path}` should be hidden");
            assert_eq!(r.show(path, &string("hunter2")).text, MASK);
        }
    }

    #[test]
    fn a_key_that_merely_mentions_a_key_is_not_hidden() {
        // The one refinement: `key` and `keys` count only at the end. A cache
        // prefix and a sort column are not secrets and a tab that hid them
        // would be a tab nobody used.
        let r = Redaction::standard();
        for path in [
            "cache.key_prefix",
            "cache.key_ttl_s",
            "server.host",
            "server.port",
            "logger.level",
            "database.url",
        ] {
            assert!(!r.hides(path), "`{path}` should be shown");
        }
    }

    #[test]
    fn a_url_with_a_password_keeps_everything_but_the_password() {
        let r = Redaction::standard();
        let shown = r.show(
            "database.url",
            &string("postgres://app:hunter2@db.internal:5432/app?sslmode=require"),
        );
        assert_eq!(
            shown.text,
            "\"postgres://app:[redacted]@db.internal:5432/app?sslmode=require\""
        );
        assert_eq!(shown.reason, Some(Reason::UrlPassword));
        // The part that answers "which database is this process pointed at"
        // survives, which is the whole reason the whole string is not masked.
        assert!(shown.text.contains("db.internal:5432/app"));
        assert!(!shown.text.contains("hunter2"));
    }

    #[test]
    fn a_url_with_no_password_is_left_alone() {
        let r = Redaction::standard();
        for url in [
            "postgres://db.internal:5432/app",
            "postgres://app@db.internal/app",
            "https://example.com/health",
            "redis://cache:6379",
            // An empty password is not a password, and inventing one would be
            // a lie about the file.
            "postgres://app:@db.internal/app",
        ] {
            let shown = r.show("database.url", &string(url));
            assert_eq!(shown.reason, None, "{url}");
            assert!(shown.text.contains(url), "{url}: {}", shown.text);
        }
    }

    #[test]
    fn an_array_is_scanned_element_by_element() {
        let r = Redaction::standard();
        let hosts = Value::new(
            ValueKind::Array(vec![
                string("postgres://a:pw1@one/db"),
                string("postgres://b@two/db"),
            ]),
            Provenance::inline("test"),
        );
        let shown = r.show("database.replicas", &hosts);
        assert!(!shown.text.contains("pw1"), "{}", shown.text);
        assert!(shown.text.contains("postgres://b@two/db"), "{}", shown.text);
        assert_eq!(shown.reason, Some(Reason::UrlPassword));
    }

    #[test]
    fn a_number_under_a_secret_name_is_still_hidden() {
        // Hidden by the name, whatever the kind. A six-digit `pin` is not
        // less of a secret for being an integer.
        let r = Redaction::standard().also("pin");
        let shown = r.show("device.pin", &integer(123_456));
        assert_eq!(shown.text, MASK);
        assert_eq!(shown.reason, Some(Reason::KeyName));
    }

    #[test]
    fn turning_it_off_is_a_line_somebody_wrote() {
        let r = Redaction::none();
        assert!(!r.hides("database.password"));
        assert_eq!(
            r.show("database.url", &string("postgres://app:hunter2@db/app"))
                .text,
            "\"postgres://app:hunter2@db/app\""
        );
    }
}
