//! What the admin tells the rest of the application after it writes, and the
//! links it adds to a record.
//!
//! Django's admin writes a `LogEntry` row after every save and draws a
//! History button on every object. This crate keeps no table, so it says
//! what it wrote to a [`Journal`] the application installs, and it draws a
//! link per [`record_link`](crate::AdminConfig::record_link) on the detail
//! screen. `brocanth-audit` is one journal and one link: the change history
//! and its page.
//!
//! A journal hears a write after the data source accepted it, never before
//! and never for a write that failed. It hears the values as the form shows
//! them: text, and `None` for a NULL.

/// What happened to the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Written {
    /// A create.
    Created,
    /// An edit.
    Updated,
    /// A delete.
    Deleted,
}

impl Written {
    /// `created`, `updated` or `deleted`.
    pub fn as_str(self) -> &'static str {
        match self {
            Written::Created => "created",
            Written::Updated => "updated",
            Written::Deleted => "deleted",
        }
    }
}

/// One column a write changed: its value before and after.
///
/// `before` is `None` on a create and for a NULL; `after` is `None` on a
/// delete and for a NULL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldChange {
    /// The column name.
    pub name: String,
    /// The value before the write.
    pub before: Option<String>,
    /// The value after the write.
    pub after: Option<String>,
}

/// One write the admin made.
#[derive(Clone, Debug)]
pub struct AdminWrite<'a> {
    /// Who made it, as [`AdminConfig::user_label`] names them. `None`
    /// when no label is configured.
    ///
    /// [`AdminConfig::user_label`]: crate::AdminConfig::user_label
    pub who: Option<&'a str>,
    /// The model key.
    pub model: &'a str,
    /// The table, as the database spells it. The same word a
    /// `brocanth_events::RowChange` carries as its collection.
    pub table: &'a str,
    /// The record's key, in its URL form: `7`, or `1,2` for a
    /// composite key.
    pub key: &'a str,
    /// What the record is called on screen.
    pub label: &'a str,
    /// Created, updated or deleted.
    pub kind: Written,
    /// The columns that changed. Every column with a value on a create
    /// and a delete; only the ones whose value differs on an edit.
    pub fields: &'a [FieldChange],
}

/// Hears every write the admin makes.
///
/// Called synchronously, after the write, on the request's task. For a
/// save with inline rows, after the commit: a write that was rolled back
/// is never heard. It must
/// not block: a journal that stores rows queues them and writes later.
pub trait Journal: Send + Sync {
    /// One write.
    fn record(&self, write: &AdminWrite<'_>);
}

impl<F> Journal for F
where
    F: Fn(&AdminWrite<'_>) + Send + Sync,
{
    fn record(&self, write: &AdminWrite<'_>) {
        self(write)
    }
}

/// The record a [`record_link`](crate::AdminConfig::record_link) is asked
/// about.
#[derive(Clone, Copy, Debug)]
pub struct RecordRef<'a> {
    /// Where the admin is mounted, so a link can point at a panel.
    pub base_path: &'a str,
    /// The model key.
    pub model: &'a str,
    /// The table.
    pub table: &'a str,
    /// The record's key, in its URL form.
    pub key: &'a str,
}

/// Builds the `href` of one link on the detail screen, or `None` for no
/// link on this record.
pub type RecordLinkFn = Box<dyn Fn(&RecordRef<'_>) -> Option<String> + Send + Sync>;

/// The columns that differ between `before` and `after`, in `after`'s
/// order, as text.
pub(crate) fn changes(
    before: Option<&crate::model::Row>,
    after: &[(String, crate::model::Value)],
) -> Vec<FieldChange> {
    let text = |value: &crate::model::Value| (!value.is_null()).then(|| value.as_input());
    after
        .iter()
        .filter_map(|(name, value)| {
            let was = before.and_then(|row| row.get(name)).and_then(text);
            let now = text(value);
            (before.is_none() || was != now).then(|| FieldChange {
                name: name.clone(),
                before: was,
                after: now,
            })
        })
        .collect()
}

/// Every column of a deleted row, as it was.
pub(crate) fn removed(row: &crate::model::Row) -> Vec<FieldChange> {
    row.iter()
        .map(|(name, value)| FieldChange {
            name: name.to_string(),
            before: (!value.is_null()).then(|| value.as_input()),
            after: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Row, Value};

    #[test]
    fn an_edit_reports_only_the_columns_that_differ() {
        let before = Row::new().with("title", "Old").with("body", "Same");
        let after = vec![
            ("title".to_string(), Value::Text("New".into())),
            ("body".to_string(), Value::Text("Same".into())),
        ];
        let changed = changes(Some(&before), &after);
        assert_eq!(
            changed,
            vec![FieldChange {
                name: "title".into(),
                before: Some("Old".into()),
                after: Some("New".into()),
            }]
        );
    }

    #[test]
    fn a_create_reports_every_column_and_a_delete_every_old_value() {
        let after = vec![("title".to_string(), Value::Text("New".into()))];
        assert_eq!(changes(None, &after)[0].before, None);
        let gone = removed(&Row::new().with("title", "Old").with("n", Value::Null));
        assert_eq!(gone.len(), 2);
        assert!(gone.iter().all(|f| f.after.is_none()));
        assert_eq!(gone.iter().find(|f| f.name == "n").unwrap().before, None);
    }
}
