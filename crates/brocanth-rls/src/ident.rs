//! Names and types as SQL text: quoted, checked, never formatted from input
//! unchecked.

use serde::{Deserialize, Serialize};

use crate::error::RlsError;

/// The longest identifier Postgres keeps. A longer one is truncated, and
/// the diff would compare two different names forever.
pub const MAX_IDENT: usize = brocanth_db::principal::MAX_IDENT;

/// Checks a name: not empty, at most 63 bytes, no NUL, no `.` (a `.` would
/// make a snapshot key ambiguous; use `in_schema` for a schema).
pub(crate) fn check(what: &str, name: &str) -> Result<(), RlsError> {
    if name.is_empty() {
        return Err(RlsError::definition(
            format!("a {what} name is empty"),
            format!("give the {what} a name"),
        ));
    }
    if name.len() > MAX_IDENT {
        return Err(RlsError::definition(
            format!(
                "the {what} name `{name}` is {} bytes; Postgres keeps {MAX_IDENT}",
                name.len()
            ),
            "use a shorter name: Postgres truncates a longer one",
        ));
    }
    if name.contains('\0') || name.contains('.') {
        return Err(RlsError::definition(
            format!("the {what} name `{name}` holds a `.` or a NUL byte"),
            "use letters, digits and `_`; name a schema with `in_schema`",
        ));
    }
    Ok(())
}

/// `"name"`, with each `"` doubled. Call [`check`] first.
pub(crate) fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `'text'`, with each `'` doubled. For a name inside a `DO` block.
pub(crate) fn literal(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// `"schema"."name"` or `"name"`.
pub(crate) fn qualified(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(schema) => format!("{}.{}", quote(schema), quote(name)),
        None => quote(name),
    }
}

/// `schema.name` or `name`: the key in a snapshot.
pub(crate) fn key(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(schema) => format!("{schema}.{name}"),
        None => name.to_string(),
    }
}

/// A Postgres type, for a [`Setting`](crate::Setting) and a
/// [`Function`](crate::Function)'s arguments and result.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum SqlType {
    /// `bigint`.
    BigInt,
    /// `integer`.
    Integer,
    /// `text`.
    Text,
    /// `uuid`.
    Uuid,
    /// `boolean`.
    Boolean,
    /// Any other type name, such as `timestamptz` or `bigint[]`. Checked
    /// against `[A-Za-z0-9_ .,()\[\]]`, so it cannot end a statement.
    Custom(String),
}

impl SqlType {
    /// The type as SQL text.
    pub fn as_sql(&self) -> &str {
        match self {
            SqlType::BigInt => "bigint",
            SqlType::Integer => "integer",
            SqlType::Text => "text",
            SqlType::Uuid => "uuid",
            SqlType::Boolean => "boolean",
            SqlType::Custom(name) => name,
        }
    }

    pub(crate) fn check(&self) -> Result<(), RlsError> {
        let SqlType::Custom(name) = self else {
            return Ok(());
        };
        let allowed = |c: char| c.is_ascii_alphanumeric() || " _.,()[]".contains(c);
        if name.trim().is_empty() || !name.chars().all(allowed) {
            return Err(RlsError::definition(
                format!("`{name}` is not a type name this crate writes"),
                "use letters, digits, spaces and `_ . , ( ) [ ]`: `timestamptz`, `numeric(10, 2)`, `bigint[]`",
            ));
        }
        Ok(())
    }
}

impl From<SqlType> for String {
    fn from(t: SqlType) -> String {
        t.as_sql().to_string()
    }
}

impl TryFrom<String> for SqlType {
    type Error = String;

    fn try_from(s: String) -> Result<SqlType, String> {
        let t = match s.as_str() {
            "bigint" => SqlType::BigInt,
            "integer" => SqlType::Integer,
            "text" => SqlType::Text,
            "uuid" => SqlType::Uuid,
            "boolean" => SqlType::Boolean,
            _ => SqlType::Custom(s),
        };
        t.check().map_err(|e| e.to_string())?;
        Ok(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_quoted_with_its_quotes_doubled() {
        assert_eq!(quote(r#"we"ird"#), r#""we""ird""#);
        assert_eq!(literal("o'brien"), "'o''brien'");
        assert_eq!(qualified(Some("app"), "posts"), r#""app"."posts""#);
    }

    #[test]
    fn a_long_empty_or_dotted_name_is_refused_with_help() {
        for name in ["", "a.b", &"x".repeat(64)] {
            let e = check("table", name).unwrap_err().to_string();
            assert!(e.contains("\nhelp: "), "{e}");
        }
        check("table", &"x".repeat(63)).unwrap();
    }

    #[test]
    fn a_custom_type_cannot_end_the_statement() {
        assert!(SqlType::Custom("bigint; DROP TABLE x".into())
            .check()
            .is_err());
        assert!(SqlType::Custom("text'".into()).check().is_err());
        SqlType::Custom("numeric(10, 2)".into()).check().unwrap();
        SqlType::Custom("bigint[]".into()).check().unwrap();
    }
}
