//! JSON, through `jsonc-parser` 0.34.
//!
//! # Why not `serde_json`
//!
//! `serde_json` is the obvious crate and it cannot do the one thing this crate
//! needs: it reports a position in an *error* and never in a *value*, so a
//! document it parsed cannot say which line any key was written on. The
//! provenance model is the point of this crate, so the parser that can carry it
//! wins. `jsonc-parser` returns an AST in which every node has a byte range.
//!
//! # The dialect
//!
//! Comments and trailing commas are allowed, because a configuration file that
//! cannot be annotated is a configuration file whose decisions are undocumented
//! — the same argument §8 makes about `main`. Everything else JSON5 permits is
//! refused, so a `.json` file here is JSON plus comments plus trailing commas,
//! and no reader has to wonder whether `0xFF` or `'single'` meant something.
//!
//! `null` is not a value. It removes the key, which lets a deployment reset one
//! setting to the plugin's default without having to know what that default is.

use std::sync::Arc;

use jsonc_parser::ast::Value as JsonValue;
use jsonc_parser::{parse_to_ast, CollectOptions, CommentCollectionStrategy, ParseOptions};

use crate::config::Source;
use crate::formats::ParseError;
use crate::value::{line_and_column, Provenance, Table, Value, ValueKind};

fn options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

pub(crate) fn parse(text: &str, file: &Arc<Source>) -> Result<Value, ParseError> {
    let parsed = parse_to_ast(
        text,
        &CollectOptions {
            comments: CommentCollectionStrategy::Off,
            tokens: false,
        },
        &options(),
    )
    .map_err(|e: jsonc_parser::errors::ParseError| {
        let (line, column) = line_and_column(text, e.range().start);
        ParseError::at(line, column, e.kind().to_string())
    })?;

    match parsed.value {
        None => Ok(Value::new(
            ValueKind::Table(Table::open()),
            provenance(text, file, 0),
        )),
        Some(JsonValue::Object(_)) => convert(text, file, parsed.value.expect("matched above")),
        Some(other) => {
            let (line, column) = line_and_column(text, node_start(&other));
            Err(ParseError::at(
                line,
                column,
                "a configuration document must be a JSON object",
            ))
        }
    }
}

fn provenance(text: &str, file: &Arc<Source>, offset: usize) -> Provenance {
    let (line, column) = line_and_column(text, offset);
    Provenance::File {
        file: Arc::clone(file),
        line,
        column,
    }
}

fn node_start(value: &JsonValue<'_>) -> usize {
    match value {
        JsonValue::StringLit(n) => n.range.start,
        JsonValue::NumberLit(n) => n.range.start,
        JsonValue::BooleanLit(n) => n.range.start,
        JsonValue::Object(n) => n.range.start,
        JsonValue::Array(n) => n.range.start,
        JsonValue::NullKeyword(n) => n.range.start,
    }
}

fn convert(text: &str, file: &Arc<Source>, value: JsonValue<'_>) -> Result<Value, ParseError> {
    let start = node_start(&value);
    let from = provenance(text, file, start);
    let (line, column) = line_and_column(text, start);

    let kind = match value {
        JsonValue::StringLit(n) => ValueKind::String(n.value.into_owned()),
        JsonValue::BooleanLit(n) => ValueKind::Boolean(n.value),
        JsonValue::NullKeyword(_) => ValueKind::Unset,
        JsonValue::NumberLit(n) => {
            if let Ok(i) = n.value.parse::<i64>() {
                ValueKind::Integer(i)
            } else {
                let f: f64 = n.value.parse().map_err(|_| {
                    ParseError::at(line, column, format!("`{}` is not a number", n.value))
                })?;
                ValueKind::Float(f)
            }
        }
        JsonValue::Array(n) => {
            let mut items = Vec::with_capacity(n.elements.len());
            for element in n.elements {
                items.push(convert(text, file, element)?);
            }
            ValueKind::Array(items)
        }
        JsonValue::Object(n) => {
            let mut table = Table::open();
            for property in n.properties {
                let name = property.name.into_string();
                table.insert(name, convert(text, file, property.value)?);
            }
            ValueKind::Table(table)
        }
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
            PathBuf::from("./config/production.json"),
            Origin::ConfigDir,
            Format::Json,
        )
    }

    #[test]
    fn every_value_knows_the_line_it_was_written_on() {
        let text = "{\n  \"server\": {\n    \"host\": \"0.0.0.0\",\n    \"port\": 8080\n  }\n}\n";
        let value = parse(text, &source()).unwrap();
        assert_eq!(value.at("server.port").unwrap().as_integer(), Some(8080));
        assert_eq!(
            value.at("server.port").unwrap().provenance().to_string(),
            "./config/production.json:4"
        );
    }

    #[test]
    fn comments_and_trailing_commas_are_allowed() {
        let text = "{\n  // the public port\n  \"port\": 80,\n}\n";
        let value = parse(text, &source()).unwrap();
        assert_eq!(value.at("port").unwrap().as_integer(), Some(80));
    }

    #[test]
    fn the_rest_of_json5_is_not() {
        let err = parse("{ 'port': 80 }", &source()).unwrap_err();
        assert_eq!(err.line, Some(1), "{err}");
    }

    #[test]
    fn null_is_the_absence_of_a_value() {
        let value = parse("{ \"level\": null }", &source()).unwrap();
        assert!(value.at("level").unwrap().is_unset());
    }

    #[test]
    fn a_document_that_is_not_an_object_is_refused() {
        let err = parse("[1, 2]", &source()).unwrap_err();
        assert!(err.message.contains("JSON object"), "{err}");
    }

    #[test]
    fn an_integer_stays_an_integer_and_a_fraction_becomes_a_float() {
        let value = parse("{ \"a\": 7, \"b\": 1.5 }", &source()).unwrap();
        assert_eq!(value.at("a").unwrap().as_integer(), Some(7));
        assert_eq!(value.at("b").unwrap().as_float(), Some(1.5));
    }
}
