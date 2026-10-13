//! YAML, through `saphyr` 0.2.
//!
//! # Why this crate and not one of the `serde_yaml` family
//!
//! `serde_yaml` was deprecated by its author in March 2024 and has had no
//! release since. `serde_yaml_ng` last published in May 2024 and `serde_norway`
//! in December 2024; neither is being developed. More decisive than any of
//! that: none of the three reports a position for a *value*, only for an error,
//! so a document any of them parsed could not say which line a key was written
//! on, and this crate exists to answer exactly that question.
//!
//! `saphyr` is maintained, is YAML 1.2 compliant, and hands back a
//! `MarkedYaml` whose every node carries a `Span` with a line and a column. It
//! is the successor to `yaml-rust2`, by the same maintainer.
//!
//! # The dialect
//!
//! One document per file. A `---` that starts a second document is an error
//! naming the line, because a configuration file with two documents has an
//! answer this crate cannot give: which one wins.
//!
//! Keys must be scalars. YAML allows a mapping or a sequence as a key; no
//! configuration format this crate reads can spell one, and a value whose key
//! only one of three formats can write is not portable configuration.
//!
//! `null` is not a value. It removes the key, which lets a deployment reset one
//! setting to the plugin's default without having to know what that default is.
//! `~`, `null` and an empty value all mean it.
//!
//! Aliases and anchors are resolved by the parser before this file sees the
//! tree, so an alias takes the provenance of the node it points at. A tag is
//! ignored and the node under it is kept.

use std::sync::Arc;

use saphyr::{LoadableYamlNode, MarkedYaml, Scalar, YamlData};

use crate::config::Source;
use crate::formats::ParseError;
use crate::value::{Provenance, Table, Value, ValueKind};

pub(crate) fn parse(text: &str, file: &Arc<Source>) -> Result<Value, ParseError> {
    let documents = MarkedYaml::load_from_str(text).map_err(|e| {
        let marker = e.marker();
        ParseError::at(
            marker.line() as u32,
            marker.col() as u32 + 1,
            e.info().to_string(),
        )
    })?;

    let mut documents = documents.into_iter();
    let Some(document) = documents.next() else {
        return Ok(Value::new(
            ValueKind::Table(Table::open()),
            Provenance::File {
                file: Arc::clone(file),
                line: 1,
                column: 1,
            },
        ));
    };

    if let Some(second) = documents.next() {
        return Err(ParseError::at(
            second.span.start.line() as u32,
            second.span.start.col() as u32 + 1,
            "a configuration file holds one document; this one starts a second",
        ));
    }

    let value = convert(file, document)?;
    if !value.is_table() {
        return Err(ParseError::whole_file(
            "a configuration document must be a YAML mapping",
        ));
    }
    Ok(value)
}

fn convert(file: &Arc<Source>, node: MarkedYaml<'_>) -> Result<Value, ParseError> {
    let line = node.span.start.line() as u32;
    let column = node.span.start.col() as u32 + 1;
    let from = Provenance::File {
        file: Arc::clone(file),
        line,
        column,
    };

    let kind = match node.data {
        YamlData::Value(scalar) => match scalar {
            Scalar::Null => ValueKind::Unset,
            Scalar::Boolean(b) => ValueKind::Boolean(b),
            Scalar::Integer(i) => ValueKind::Integer(i),
            Scalar::FloatingPoint(f) => ValueKind::Float(f.into_inner()),
            Scalar::String(s) => ValueKind::String(s.into_owned()),
        },
        YamlData::Representation(raw, _, _) => ValueKind::String(raw.into_owned()),
        YamlData::Sequence(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                out.push(convert(file, item)?);
            }
            ValueKind::Array(out)
        }
        YamlData::Mapping(entries) => {
            // A mapping read from a file never declares a key set: only a
            // plugin's own defaults can do that.
            let mut table = Table::open();
            for (key, value) in entries {
                let name = scalar_key(&key).ok_or_else(|| {
                    ParseError::at(
                        key.span.start.line() as u32,
                        key.span.start.col() as u32 + 1,
                        "a configuration key must be a name, not a mapping or a sequence",
                    )
                })?;
                table.insert(name, convert(file, value)?);
            }
            ValueKind::Table(table)
        }
        YamlData::Tagged(_, inner) => return convert(file, *inner),
        YamlData::Alias(_) => {
            return Err(ParseError::at(
                line,
                column,
                "this alias points at nothing the parser resolved",
            ))
        }
        YamlData::BadValue => {
            return Err(ParseError::at(
                line,
                column,
                "this value is syntactically valid YAML whose contents make no sense",
            ))
        }
    };

    Ok(Value::new(kind, from))
}

fn scalar_key(node: &MarkedYaml<'_>) -> Option<String> {
    match &node.data {
        YamlData::Value(Scalar::String(s)) => Some(s.to_string()),
        YamlData::Value(Scalar::Integer(i)) => Some(i.to_string()),
        YamlData::Value(Scalar::Boolean(b)) => Some(b.to_string()),
        YamlData::Representation(raw, _, _) => Some(raw.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Origin;
    use crate::value::{file_source, Format};
    use std::path::PathBuf;

    fn source() -> Arc<Source> {
        file_source(
            PathBuf::from("./config/production.yaml"),
            Origin::ConfigDir,
            Format::Yaml,
        )
    }

    #[test]
    fn every_value_knows_the_line_it_was_written_on() {
        let text = "\
server:
  host: 0.0.0.0
  port: 8080
  pool:
    max_size: 20
";
        let value = parse(text, &source()).unwrap();
        assert_eq!(value.at("server.port").unwrap().as_integer(), Some(8080));
        assert_eq!(
            value.at("server.port").unwrap().provenance().to_string(),
            "./config/production.yaml:3"
        );
        assert_eq!(
            value
                .at("server.pool.max_size")
                .unwrap()
                .provenance()
                .to_string(),
            "./config/production.yaml:5"
        );
    }

    #[test]
    fn the_core_schema_types_all_arrive() {
        let text = "s: x\ni: 42\nf: 1.5\nb: true\na:\n  - 1\n  - 2\n";
        let v = parse(text, &source()).unwrap();
        assert_eq!(v.at("s").unwrap().as_str(), Some("x"));
        assert_eq!(v.at("i").unwrap().as_integer(), Some(42));
        assert_eq!(v.at("f").unwrap().as_float(), Some(1.5));
        assert_eq!(v.at("b").unwrap().as_bool(), Some(true));
        assert_eq!(v.at("a").unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn null_is_the_absence_of_a_value() {
        let v = parse("level: null\nother: ~\n", &source()).unwrap();
        assert!(v.at("level").unwrap().is_unset());
        assert!(v.at("other").unwrap().is_unset());
    }

    #[test]
    fn a_second_document_is_an_error_that_names_the_line() {
        let err = parse("a: 1\n---\nb: 2\n", &source()).unwrap_err();
        assert!(err.message.contains("one document"), "{err}");
        assert!(err.line.is_some(), "{err}");
    }

    #[test]
    fn a_document_that_is_not_a_mapping_is_refused() {
        let err = parse("- 1\n- 2\n", &source()).unwrap_err();
        assert!(err.message.contains("mapping"), "{err}");
    }

    #[test]
    fn an_anchor_and_its_alias_both_arrive() {
        let text = "\
defaults: &d
  level: warn
logger: *d
";
        let v = parse(text, &source()).unwrap();
        assert_eq!(v.at("logger.level").unwrap().as_str(), Some("warn"));
    }
}
