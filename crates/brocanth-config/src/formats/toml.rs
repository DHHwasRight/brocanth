//! TOML, through `toml` 1.1.
//!
//! The crate is taken with `default-features = false` and the `parse` feature
//! added by *our* `toml` feature, so turning TOML off leaves `toml_parser` and
//! `winnow` out of the build rather than leaving them in behind a branch.
//!
//! Spans come from `toml::de::DeTable::parse`, which returns every key and
//! every value wrapped in a `serde_spanned::Spanned`. That is why this file
//! walks the `De*` types rather than deserialising into `toml::Value`: a
//! `toml::Value` has no position in it and a provenance that cannot name a line
//! is not worth having.

use std::sync::Arc;

use serde_spanned::Spanned;
use toml::de::{DeTable, DeValue};

use crate::config::Source;
use crate::formats::ParseError;
use crate::value::{line_and_column, Provenance, Table, Value, ValueKind};

pub(crate) fn parse(text: &str, file: &Arc<Source>) -> Result<Value, ParseError> {
    let document = DeTable::parse(text).map_err(|e| {
        let (line, column) = e
            .span()
            .map_or((1, 1), |span| line_and_column(text, span.start));
        ParseError::at(line, column, e.message().to_string())
    })?;

    let from = provenance(text, file, document.span().start);
    let table = convert_table(text, file, document.into_inner())?;
    Ok(Value::new(ValueKind::Table(table), from))
}

fn provenance(text: &str, file: &Arc<Source>, offset: usize) -> Provenance {
    let (line, column) = line_and_column(text, offset);
    Provenance::File {
        file: Arc::clone(file),
        line,
        column,
    }
}

fn convert_table(text: &str, file: &Arc<Source>, table: DeTable<'_>) -> Result<Table, ParseError> {
    // A table read from a file never declares a key set: only a plugin's own
    // defaults can do that, because only a Rust struct knows what its keys are.
    let mut out = Table::open();
    for (key, value) in table {
        let name = key.into_inner().into_owned();
        out.insert(name, convert_value(text, file, value)?);
    }
    Ok(out)
}

fn convert_value(
    text: &str,
    file: &Arc<Source>,
    value: Spanned<DeValue<'_>>,
) -> Result<Value, ParseError> {
    let span = value.span();
    let from = provenance(text, file, span.start);
    let (line, column) = line_and_column(text, span.start);

    let kind = match value.into_inner() {
        DeValue::String(s) => ValueKind::String(s.into_owned()),
        DeValue::Integer(i) => {
            let n = i64::from_str_radix(i.as_str(), i.radix()).map_err(|_| {
                ParseError::at(line, column, format!("`{i}` does not fit an integer"))
            })?;
            ValueKind::Integer(n)
        }
        DeValue::Float(f) => {
            let n: f64 = f
                .as_str()
                .parse()
                .map_err(|_| ParseError::at(line, column, format!("`{f}` is not a number")))?;
            ValueKind::Float(n)
        }
        DeValue::Boolean(b) => ValueKind::Boolean(b),
        DeValue::Datetime(d) => ValueKind::Datetime(d.to_string()),
        DeValue::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(convert_value(text, file, item)?);
            }
            ValueKind::Array(out)
        }
        DeValue::Table(t) => ValueKind::Table(convert_table(text, file, t)?),
    };

    Ok(Value::new(kind, from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Origin;
    use crate::value::{file_source, Format};
    use std::path::PathBuf;

    fn source() -> Arc<Source> {
        file_source(
            PathBuf::from("/etc/my_app/production.toml"),
            Origin::SystemDir,
            Format::Toml,
        )
    }

    #[test]
    fn every_value_knows_the_line_it_was_written_on() {
        let text = "\
[server]
host = \"0.0.0.0\"
port = 8080

[server.pool]
max_size = 20
";
        let value = parse(text, &source()).unwrap();

        let port = value.at("server.port").unwrap();
        assert_eq!(port.as_integer(), Some(8080));
        match port.provenance() {
            Provenance::File { line, file, .. } => {
                assert_eq!(*line, 3);
                assert_eq!(file.format, Format::Toml);
            }
            other => panic!("expected a file provenance, got {other:?}"),
        }

        let max = value.at("server.pool.max_size").unwrap();
        assert_eq!(
            max.provenance().to_string(),
            "/etc/my_app/production.toml:6"
        );
    }

    #[test]
    fn the_scalar_types_all_arrive() {
        let text = "\
s = \"x\"
i = 42
f = 1.5
b = true
d = 1979-05-27T07:32:00Z
a = [1, 2]
";
        let v = parse(text, &source()).unwrap();
        assert_eq!(v.at("s").unwrap().as_str(), Some("x"));
        assert_eq!(v.at("i").unwrap().as_integer(), Some(42));
        assert_eq!(v.at("f").unwrap().as_float(), Some(1.5));
        assert_eq!(v.at("b").unwrap().as_bool(), Some(true));
        assert_eq!(
            v.at("d").unwrap().kind().type_name(),
            "datetime",
            "a TOML datetime keeps its own kind"
        );
        assert_eq!(v.at("a").unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_syntax_error_names_the_line() {
        let err = parse("[server\nport = 1\n", &source()).unwrap_err();
        assert_eq!(err.line, Some(1), "{err}");
    }

    #[test]
    fn a_table_read_from_a_file_is_open_because_a_file_declares_no_key_set() {
        let v = parse("[server]\nport = 1\n", &source()).unwrap();
        assert!(v.at("server").unwrap().as_table().unwrap().is_open());
    }
}
