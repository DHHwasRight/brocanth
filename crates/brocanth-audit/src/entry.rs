//! [`Entry`]: one change, as the history keeps it.

use brocanth_events::{ChangeKind, RowChange};

/// What happened to the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// It was created.
    Created,
    /// It was changed.
    Updated,
    /// It was deleted.
    Deleted,
}

impl Kind {
    /// `created`, `updated` or `deleted`: the stored word.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Created => "created",
            Kind::Updated => "updated",
            Kind::Deleted => "deleted",
        }
    }

    /// The stored word read back. Anything else is `updated`, the kind
    /// that claims the least.
    pub fn parse(word: &str) -> Kind {
        match word {
            "created" => Kind::Created,
            "deleted" => Kind::Deleted,
            _ => Kind::Updated,
        }
    }
}

impl From<&ChangeKind<'_>> for Kind {
    fn from(kind: &ChangeKind<'_>) -> Kind {
        match kind {
            ChangeKind::Created => Kind::Created,
            ChangeKind::Updated { .. } => Kind::Updated,
            ChangeKind::Deleted => Kind::Deleted,
        }
    }
}

/// One column a change touched.
///
/// `before` and `after` are text, as a form shows them, and `None` when
/// the value is unknown or NULL. A change the model layer published names
/// its columns only: a `RowChange` carries no values, so both are `None`.
/// A change the admin made carries both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The column name.
    pub name: String,
    /// The value before.
    pub before: Option<String>,
    /// The value after.
    pub after: Option<String>,
}

impl Field {
    /// A column named with no values.
    pub fn named(name: impl Into<String>) -> Field {
        Field {
            name: name.into(),
            before: None,
            after: None,
        }
    }
}

/// One change to one row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The stored id. 0 before the entry is stored.
    pub id: i64,
    /// When, in seconds since 1970, UTC.
    pub at: i64,
    /// Who, as the application names them. `None` for a change nobody was
    /// signed in for: a job, a migration, a page with no user.
    pub actor: Option<String>,
    /// Where the change came from: `admin`, or `app` for a write through
    /// the model layer.
    pub source: String,
    /// The table, as the database spells it.
    pub table: String,
    /// The row's key: `7`, or `1,2` for a composite key.
    pub row: String,
    /// What the row was called on screen, when the writer knew. Empty
    /// otherwise.
    pub label: String,
    /// Created, updated or deleted.
    pub kind: Kind,
    /// The columns it touched.
    pub fields: Vec<Field>,
}

impl Entry {
    /// The entry for a change the model layer published, made by `actor`
    /// at `at`.
    pub fn from_change(change: &RowChange<'_>, actor: Option<String>, at: i64) -> Entry {
        Entry {
            id: 0,
            at,
            actor,
            source: "app".to_string(),
            table: change.collection.to_string(),
            row: change.row.to_string(),
            label: String::new(),
            kind: (&change.kind).into(),
            fields: change.changed_fields().iter().map(Field::named).collect(),
        }
    }
}

/// Seconds since 1970 as `2026-11-09 14:03 UTC`. No date library: the
/// civil-from-days algorithm from Howard Hinnant's `date` paper.
pub fn format_time(at: i64) -> String {
    let days = at.div_euclid(86_400);
    let secs = at.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        secs / 3_600,
        secs % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_published_change_becomes_an_entry_with_its_column_names() {
        let fields = ["title", "body"];
        let change = RowChange::updated("post", 7_i64, fields.as_slice());
        let entry = Entry::from_change(&change, Some("ada".into()), 100);
        assert_eq!(entry.table, "post");
        assert_eq!(entry.row, "7");
        assert_eq!(entry.kind, Kind::Updated);
        assert_eq!(entry.source, "app");
        assert_eq!(
            entry.fields,
            vec![Field::named("title"), Field::named("body")]
        );
    }

    #[test]
    fn a_time_prints_as_a_utc_date_and_minute() {
        assert_eq!(format_time(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_time(1_762_697_000), "2025-11-09 14:03 UTC");
        assert_eq!(format_time(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn a_kind_round_trips_through_its_word() {
        for kind in [Kind::Created, Kind::Updated, Kind::Deleted] {
            assert_eq!(Kind::parse(kind.as_str()), kind);
        }
    }
}
