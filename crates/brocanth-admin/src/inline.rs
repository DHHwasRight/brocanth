//! Child rows on the parent's form. Django's `InlineModelAdmin`.
//!
//! An [`Inline`] puts the records of another model that point at this one
//! on this model's create and edit form: the posts of a user, the lines of
//! an invoice. Each child row can be changed, deleted, or added, in the
//! same `<form>` and the same POST as the parent.
//!
//! ```
//! use brocanth_admin::{AdminConfig, Inline};
//!
//! let config = AdminConfig::new().model("user", |m| {
//!     m.inline(Inline::new("post").fields(["title", "published"]))
//! });
//! # let _ = config;
//! ```
//!
//! # The rules
//!
//! - **The child's rules, per row.** A child row is changed under the
//!   child model's screen policy (`Action::Update` on that record) and its
//!   row rule, as on the child's own edit form. A row the rules refuse is
//!   shown read-only, and a POST that changes it anyway is refused with
//!   403 before anything is written.
//! - **One check before any write.** The admin checks and validates the
//!   parent and every child row first. One error re-renders the form with
//!   422, the error beside its row, and writes nothing.
//! - **No JavaScript.** The form renders `extra` blank rows for new
//!   children. A blank row that comes back blank adds nothing.
//!
//! # The names a row posts
//!
//! Django's prefixes: `{model}-{n}-{column}` for a field, `{model}-{n}-pk`
//! for the key of an existing child and `{model}-{n}-delete` for its
//! delete box. `n` is the row's place on the form. The key, not `n`, says
//! which child a row changes, and the admin checks that the key belongs to
//! this parent.

use std::collections::BTreeMap;

use crate::form::Submission;

/// How many child rows an inline shows when nothing says otherwise.
pub const DEFAULT_MAX_ROWS: usize = 50;

/// One model's records on another model's form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inline {
    pub(crate) model: String,
    pub(crate) fk: Option<String>,
    pub(crate) fields: Vec<String>,
    pub(crate) extra: usize,
    pub(crate) max: usize,
    /// The parent column the child's `fk` holds. Set by `Admin::new`.
    pub(crate) parent_column: String,
}

impl Inline {
    /// The records of the model `model` that point at the configured one.
    ///
    /// The column that points is found from the schema: the one column of
    /// `model` whose relation targets the parent. Name it with
    /// [`Inline::fk`] when there are two, or when the schema does not
    /// report the relation.
    pub fn new(model: impl Into<String>) -> Self {
        Inline {
            model: model.into(),
            fk: None,
            fields: Vec::new(),
            extra: 1,
            max: DEFAULT_MAX_ROWS,
            parent_column: String::new(),
        }
    }

    /// The child column that holds the parent's key. Django's `fk_name`.
    pub fn fk(mut self, column: impl Into<String>) -> Self {
        self.fk = Some(column.into());
        self
    }

    /// The child columns each row shows, in this order. Empty means the
    /// child's form fields without the column that points at the parent.
    pub fn fields<I, S>(mut self, columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.fields = columns.into_iter().map(Into::into).collect();
        self
    }

    /// How many blank rows the form offers for new children. 1 when not
    /// set; 0 offers none. Django's `extra`.
    pub fn extra(mut self, rows: usize) -> Self {
        self.extra = rows;
        self
    }

    /// The most existing children the form shows. [`DEFAULT_MAX_ROWS`]
    /// when not set. A parent with more shows the first ones by key and a
    /// line that says so. Django's `max_num`, for the rows shown.
    pub fn max(mut self, rows: usize) -> Self {
        self.max = rows.max(1);
        self
    }

    /// The child model's key.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// The prefix of every name a row of this inline posts.
    pub fn prefix(&self) -> &str {
        &self.model
    }
}

/// One child row as it was posted: its key, its delete box, and its fields
/// with the prefix taken off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PostedRow {
    /// `{prefix}-{n}-pk`, when the row is an existing child.
    pub(crate) key: Option<String>,
    /// `{prefix}-{n}-delete` was ticked.
    pub(crate) delete: bool,
    /// Every other `{prefix}-{n}-{column}`, as `column`.
    pub(crate) fields: Submission,
}

impl PostedRow {
    /// Whether a new row came back with nothing in it: a blank extra row.
    pub(crate) fn is_blank(&self) -> bool {
        self.key.is_none() && self.fields.pairs().iter().all(|(_, v)| v.trim().is_empty())
    }
}

/// The rows one inline posted, in the order of their `n`.
///
/// `None` when more than `cap` rows were posted: the form never renders
/// that many, so the request was not made with it.
pub(crate) fn posted(submission: &Submission, prefix: &str, cap: usize) -> Option<Vec<PostedRow>> {
    let mut rows: BTreeMap<usize, PostedRow> = BTreeMap::new();
    let start = format!("{prefix}-");
    for (name, value) in submission.pairs() {
        let Some(rest) = name.strip_prefix(&start) else {
            continue;
        };
        let Some((index, column)) = rest.split_once('-') else {
            continue;
        };
        let Ok(index) = index.parse::<usize>() else {
            continue;
        };
        let row = rows.entry(index).or_default();
        match column {
            "pk" => row.key = Some(value.clone()),
            "delete" => row.delete = matches!(value.as_str(), "on" | "true" | "1" | "yes"),
            _ => {
                let fields = std::mem::take(&mut row.fields);
                row.fields = fields.with(column, value.clone());
            }
        }
    }
    (rows.len() <= cap).then(|| rows.into_values().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_grouped_by_their_place_and_the_prefix_is_taken_off() {
        let form = Submission::new()
            .with("title", "the parent's own field")
            .with("post-0-pk", "7")
            .with("post-0-title", "Seven")
            .with("post-1-title", "")
            .with("post-0-delete", "on")
            .with("post-10-title", "Ten")
            .with("comment-0-title", "another inline");
        let rows = posted(&form, "post", 5).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].key.as_deref(), Some("7"));
        assert!(rows[0].delete);
        assert_eq!(rows[0].fields.get("title"), Some("Seven"));
        assert!(rows[1].is_blank());
        assert_eq!(rows[2].fields.get("title"), Some("Ten"));
        assert!(posted(&form, "post", 2).is_none(), "over the cap");
    }
}
