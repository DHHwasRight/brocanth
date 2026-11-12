//! Names and types as SQL text: quoted, checked, never formatted from input
//! unchecked.

use serde::{Deserialize, Serialize};

use crate::error::RlsError;

/// The longest identifier Postgres keeps. A longer one is truncated, and
/// the diff would compare two different names forever.
pub const MAX_IDENT: usize = brocanth_db::principal::MAX_IDENT;

/// Checks a name with `brocanth-db`'s checker (not empty, at most 63 bytes,
/// no control character), and refuses a `.`: a `.` would make a snapshot
/// key ambiguous; use `in_schema` for a schema.
pub(crate) fn check(what: &str, name: &str) -> Result<(), RlsError> {
    brocanth_db::principal::check_ident(what, name).map_err(RlsError::from_config)?;
    if name.contains('.') {
        return Err(RlsError::definition(
            format!("the {what} name `{name}` holds a `.`"),
            "use letters, digits and `_`; name a schema with `in_schema`",
        ));
    }
    Ok(())
}

/// `"name"`, with each `"` doubled. Call [`check`] first.
pub(crate) fn quote(name: &str) -> String {
    brocanth_db::principal::double_quote(name)
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
    /// Any other type name, such as `timestamptz` or `bigint[]`. Its shape
    /// is `name[ name]*[(digits[, digits])][[]]*`: words, an optional
    /// modifier, array brackets. Nothing else, so it cannot end a statement
    /// or add an operator.
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

    /// The built-in variant for a `Custom` name Postgres reads as one
    /// (`bigint`, `int8`, `BOOL`), so `Custom("bigint")` and `BigInt` are
    /// one type and the diff does not recreate a function forever.
    pub fn normalized(self) -> SqlType {
        let SqlType::Custom(name) = &self else {
            return self;
        };
        match name.trim().to_ascii_lowercase().as_str() {
            "bigint" | "int8" => SqlType::BigInt,
            "integer" | "int" | "int4" => SqlType::Integer,
            "text" => SqlType::Text,
            "uuid" => SqlType::Uuid,
            "boolean" | "bool" => SqlType::Boolean,
            _ => self,
        }
    }

    /// Whether Postgres reads `value` as this type. `true` for a `Custom`
    /// type, which this crate cannot check.
    pub fn accepts(&self, value: &str) -> bool {
        match self {
            SqlType::BigInt => value.trim().parse::<i64>().is_ok(),
            SqlType::Integer => value.trim().parse::<i32>().is_ok(),
            SqlType::Text | SqlType::Custom(_) => true,
            SqlType::Uuid => is_uuid(value.trim()),
            SqlType::Boolean => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "true" | "false" | "t" | "f" | "yes" | "no" | "on" | "off" | "1" | "0"
            ),
        }
    }

    pub(crate) fn check(&self) -> Result<(), RlsError> {
        let SqlType::Custom(name) = self else {
            return Ok(());
        };
        if !type_shaped(name) {
            return Err(RlsError::definition(
                format!("`{name}` is not a type name this crate writes"),
                "write words, an optional `(digits[, digits])` and `[]`: `timestamptz`, `numeric(10, 2)`, `bigint[]`",
            ));
        }
        Ok(())
    }
}

/// `8-4-4-4-12` hex digits, or 32 hex digits.
fn is_uuid(value: &str) -> bool {
    let hex: String = value.chars().filter(|c| *c != '-').collect();
    let dashes_ok = !value.contains('-') || value.split('-').map(str::len).eq([8, 4, 4, 4, 12]);
    hex.len() == 32 && dashes_ok && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// `name[ name]*[(digits[,digits])][[]]*`, where a name is
/// `[A-Za-z_][A-Za-z0-9_]*`, optionally `schema.name`, with spaces between
/// words and around the modifier's digits.
fn type_shaped(text: &str) -> bool {
    let s = text.trim();
    // The words, up to `(` or `[`.
    let end = s.find(['(', '[']).unwrap_or(s.len());
    let (words, mut rest) = s.split_at(end);
    let word_ok = |w: &str| {
        w.split('.').count() <= 2
            && w.split('.').all(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
            })
    };
    let words: Vec<&str> = words.split(' ').filter(|w| !w.is_empty()).collect();
    if words.is_empty() || !words.iter().all(|w| word_ok(w)) {
        return false;
    }
    // The modifier: `(digits)` or `(digits, digits)`.
    if let Some(inner) = rest.strip_prefix('(') {
        let Some(close) = inner.find(')') else {
            return false;
        };
        let digits: Vec<&str> = inner[..close].split(',').map(str::trim).collect();
        if digits.len() > 2
            || !digits
                .iter()
                .all(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
        {
            return false;
        }
        rest = inner[close + 1..].trim_start();
    }
    // Array brackets, any number.
    while let Some(after) = rest.strip_prefix("[]") {
        rest = after;
    }
    rest.is_empty()
}

impl From<SqlType> for String {
    fn from(t: SqlType) -> String {
        t.as_sql().to_string()
    }
}

impl TryFrom<String> for SqlType {
    type Error = String;

    fn try_from(s: String) -> Result<SqlType, String> {
        let t = SqlType::Custom(s).normalized();
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
    fn a_long_empty_dotted_or_control_name_is_refused_with_help() {
        for name in ["", "a.b", &"x".repeat(64), "a\nb", "a\r-- down", "a\0"] {
            let e = check("table", name).unwrap_err().to_string();
            assert!(e.contains("\nhelp: "), "{e}");
        }
        check("table", &"x".repeat(63)).unwrap();
    }

    #[test]
    fn a_custom_type_cannot_end_the_statement() {
        for bad in [
            "bigint; DROP TABLE x",
            "text'",
            "bigint) OR (true",
            "numeric(10, 2) OR true",
            "int[1]",
            "a(b)",
            "x(1,2,3)",
            "",
            "(1)",
        ] {
            assert!(SqlType::Custom(bad.into()).check().is_err(), "{bad}");
        }
        for good in [
            "numeric(10, 2)",
            "bigint[]",
            "timestamptz",
            "double precision",
            "character varying(20)[]",
            "public.citext",
        ] {
            SqlType::Custom(good.into()).check().unwrap();
        }
    }

    #[test]
    fn a_custom_built_in_name_is_the_built_in() {
        assert_eq!(
            SqlType::Custom("bigint".into()).normalized(),
            SqlType::BigInt
        );
        assert_eq!(
            SqlType::Custom("BOOL".into()).normalized(),
            SqlType::Boolean
        );
        let back: SqlType = serde_json::from_str("\"int8\"").unwrap();
        assert_eq!(back, SqlType::BigInt);
        assert_eq!(
            SqlType::Custom("timestamptz".into()).normalized(),
            SqlType::Custom("timestamptz".into())
        );
    }

    #[test]
    fn a_value_is_checked_against_its_type() {
        assert!(SqlType::BigInt.accepts("7"));
        assert!(!SqlType::BigInt.accepts("seven"));
        assert!(SqlType::Uuid.accepts("67e55044-10b1-426f-9247-bb680e5fe0c8"));
        assert!(!SqlType::Uuid.accepts("67e55044"));
        assert!(SqlType::Boolean.accepts("true"));
        assert!(!SqlType::Boolean.accepts("maybe"));
    }
}
