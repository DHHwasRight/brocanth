//! Validation errors, shaped for rendering next to a form field.

use std::fmt;

use sea_orm::DbErr;
use serde::{Deserialize, Serialize};

/// Marks a [`DbErr::Custom`] that carries a serialised [`Errors`].
///
/// SeaORM's hooks can only fail with a `DbErr`, so this is the one channel a
/// `before_save` validation has. [`Error::from`] decodes it again, which is
/// what lets a direct `active_model.insert(db)` — a call that never went
/// through our write helpers — still return field-level errors.
const SENTINEL: &str = "brocanth.validation\u{1}";

/// One validation failure on one field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Field the message belongs to, e.g. `"email"`.
    pub field: String,
    /// Machine-readable reason, e.g. `"blank"`, `"too_long"`, `"taken"`.
    pub code: String,
    /// Message without the field name, e.g. `"can't be blank"`.
    pub message: String,
}

impl Entry {
    /// The message with the humanised field name in front, e.g.
    /// `"Email can't be blank"`.
    pub fn full_message(&self) -> String {
        format!("{} {}", humanize(&self.field), self.message)
    }
}

/// Every validation failure on one record, in declaration order.
///
/// A form renderer asks [`Errors::on`] for one field and gets the messages to
/// print beside that input; a flash message asks [`Errors::full_messages`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Errors {
    entries: Vec<Entry>,
}

impl Errors {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a failure.
    pub fn add(
        &mut self,
        field: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.entries.push(Entry {
            field: field.into(),
            code: code.into(),
            message: message.into(),
        });
    }

    /// `true` when the record passed every rule.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Number of failures.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Every failure, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, Entry> {
        self.entries.iter()
    }

    /// Messages for one field. Empty when the field passed.
    ///
    /// This is the call a form builder makes per input.
    pub fn on(&self, field: &str) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.field == field)
            .map(|e| e.message.as_str())
            .collect()
    }

    /// `true` when `field` has at least one message.
    pub fn has(&self, field: &str) -> bool {
        self.entries.iter().any(|e| e.field == field)
    }

    /// `true` when `field` failed with this code.
    pub fn has_code(&self, field: &str, code: &str) -> bool {
        self.entries
            .iter()
            .any(|e| e.field == field && e.code == code)
    }

    /// Fields that have at least one message, first-seen order, no repeats.
    pub fn fields(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for e in &self.entries {
            if !out.contains(&e.field.as_str()) {
                out.push(&e.field);
            }
        }
        out
    }

    /// Every message with its humanised field name in front.
    pub fn full_messages(&self) -> Vec<String> {
        self.entries.iter().map(Entry::full_message).collect()
    }

    /// Append another set.
    pub fn merge(&mut self, other: Errors) {
        self.entries.extend(other.entries);
    }

    /// Encode into the one error channel SeaORM's hooks give us.
    pub fn into_db_err(self) -> DbErr {
        let payload =
            serde_json::to_string(&self).unwrap_or_else(|_| "{\"entries\":[]}".to_owned());
        DbErr::Custom(format!("{SENTINEL}{payload}"))
    }

    /// Recover errors encoded by [`Errors::into_db_err`], if that is what this
    /// `DbErr` is.
    pub fn from_db_err(err: &DbErr) -> Option<Errors> {
        match err {
            DbErr::Custom(s) => s
                .strip_prefix(SENTINEL)
                .and_then(|json| serde_json::from_str(json).ok()),
            _ => None,
        }
    }
}

impl<'a> IntoIterator for &'a Errors {
    type Item = &'a Entry;
    type IntoIter = std::slice::Iter<'a, Entry>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl fmt::Display for Errors {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for e in &self.entries {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            f.write_str(&e.full_message())?;
        }
        Ok(())
    }
}

/// `"email_address"` becomes `"Email address"`.
pub fn humanize(field: &str) -> String {
    let mut out = field.replace('_', " ");
    if let Some(first) = out.get(..1).map(str::to_uppercase) {
        out.replace_range(..1, &first);
    }
    out
}

/// What a brocanth write can fail with.
///
/// [`Error::Invalid`] is the one the form path cares about; everything else is
/// a database problem.
#[derive(Debug)]
pub enum Error {
    /// The record did not pass validation. No write was attempted.
    Invalid(Errors),
    /// The database refused or failed.
    Db(DbErr),
}

impl Error {
    /// The validation errors, when that is why the write failed.
    pub fn errors(&self) -> Option<&Errors> {
        match self {
            Error::Invalid(e) => Some(e),
            Error::Db(_) => None,
        }
    }

    /// `true` when the write failed validation rather than the database.
    pub fn is_invalid(&self) -> bool {
        matches!(self, Error::Invalid(_))
    }
}

impl From<DbErr> for Error {
    fn from(err: DbErr) -> Self {
        match Errors::from_db_err(&err) {
            Some(errors) => Error::Invalid(errors),
            None => Error::Db(err),
        }
    }
}

impl From<Error> for DbErr {
    fn from(err: Error) -> Self {
        match err {
            Error::Invalid(e) => e.into_db_err(),
            Error::Db(e) => e,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(e) => write!(f, "validation failed: {e}"),
            Error::Db(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Db(e) => Some(e),
            Error::Invalid(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_field_names() {
        assert_eq!(humanize("email_address"), "Email address");
        assert_eq!(humanize("title"), "Title");
        assert_eq!(humanize(""), "");
    }

    #[test]
    fn errors_round_trip_through_a_db_err() {
        let mut errors = Errors::new();
        errors.add("email", "blank", "can't be blank");
        errors.add("email", "taken", "has already been taken");
        errors.add("title", "too_short", "is too short (minimum is 3)");

        let recovered = Errors::from_db_err(&errors.clone().into_db_err()).expect("decodes");
        assert_eq!(recovered, errors);
        assert_eq!(recovered.on("email").len(), 2);
        assert_eq!(recovered.fields(), ["email", "title"]);
        assert_eq!(recovered.full_messages()[0], "Email can't be blank");
    }

    #[test]
    fn a_plain_db_err_is_not_mistaken_for_a_validation() {
        let err = DbErr::Custom("something else".to_owned());
        assert!(Errors::from_db_err(&err).is_none());
        assert!(!Error::from(err).is_invalid());
    }
}
