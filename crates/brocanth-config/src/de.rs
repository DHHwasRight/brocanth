//! Deserialising a [`Value`] into the type a plugin declared.
//!
//! The plugin's `Deserialize` implementation *is* the schema. That is a
//! deliberate choice, written up in `lib.rs`: a second schema vocabulary beside
//! it would have to be kept in step with it by hand, and the one that is
//! checked by the compiler is the one that cannot drift.
//!
//! What this module adds over a plain serde value deserialiser is the error.
//! When a value is the wrong type, the message carries the
//! [`Provenance`](crate::Provenance) of the exact node that was wrong, so the
//! error says `invalid type: found string, expected integer — $MY_APP_SERVER__PORT`
//! rather than naming a key and leaving an operator to guess which of five
//! layers set it.

use std::fmt;

use serde::de::{self, IntoDeserializer};

use crate::value::{Provenance, Table, Value, ValueKind};

/// A value that does not fit the type a plugin declared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeError {
    message: String,
    from: Option<Provenance>,
}

impl DeError {
    /// What went wrong, without the provenance.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Where the offending value came from, when the failure was on one node.
    #[must_use]
    pub fn provenance(&self) -> Option<&Provenance> {
        self.from.as_ref()
    }

    /// Attaches a provenance, unless a nearer one is already attached.
    fn at(mut self, from: &Provenance) -> Self {
        if self.from.is_none() {
            self.from = Some(from.clone());
        }
        self
    }
}

impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(from) = &self.from {
            write!(f, " — set at {from}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DeError {}

impl de::Error for DeError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self {
            message: msg.to_string(),
            from: None,
        }
    }
}

/// Deserialises `value` into `T`.
///
/// # Errors
///
/// [`DeError`] when the value does not fit `T`, carrying the provenance of the
/// node that did not fit.
pub fn from_value<T: de::DeserializeOwned>(value: Value) -> Result<T, DeError> {
    T::deserialize(ValueDeserializer { value })
}

/// A [`Value`], as a serde deserialiser.
pub struct ValueDeserializer {
    value: Value,
}

impl ValueDeserializer {
    /// Wraps a value.
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self { value }
    }
}

impl<'de> de::Deserializer<'de> for ValueDeserializer {
    type Error = DeError;

    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        let from = self.value.provenance().clone();
        let result = match self.value.into_kind() {
            ValueKind::String(s) | ValueKind::Datetime(s) => visitor.visit_string(s),
            ValueKind::Integer(i) => visitor.visit_i64(i),
            ValueKind::Float(f) => visitor.visit_f64(f),
            ValueKind::Boolean(b) => visitor.visit_bool(b),
            ValueKind::Array(a) => visitor.visit_seq(SeqAccess {
                iter: a.into_iter(),
            }),
            ValueKind::Table(t) => visitor.visit_map(MapAccess {
                iter: t.into_iter(),
                value: None,
            }),
            ValueKind::Unset => visitor.visit_unit(),
        };
        result.map_err(|e| e.at(&from))
    }

    fn deserialize_option<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        if self.value.is_unset() {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        let from = self.value.provenance().clone();
        let result = match self.value.into_kind() {
            ValueKind::String(s) => visitor.visit_enum(s.into_deserializer()),
            ValueKind::Table(t) if t.len() == 1 => {
                let (variant, value) = t.into_iter().next().expect("checked len == 1");
                visitor.visit_enum(EnumAccess { variant, value })
            }
            ValueKind::Table(t) => Err(de::Error::custom(format!(
                "`{name}` is one variant, so its table must have exactly one key; this one has {}",
                t.len()
            ))),
            other => Err(de::Error::invalid_type(
                de::Unexpected::Other(other.type_name()),
                &"a variant name, or a table with the variant name as its one key",
            )),
        };
        result.map_err(|e: DeError| e.at(&from))
    }

    fn deserialize_ignored_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct
        identifier
    }
}

struct SeqAccess {
    iter: std::vec::IntoIter<Value>,
}

impl<'de> de::SeqAccess<'de> for SeqAccess {
    type Error = DeError;

    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, DeError> {
        match self.iter.next() {
            Some(value) => seed.deserialize(ValueDeserializer { value }).map(Some),
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct MapAccess {
    iter: <Table as IntoIterator>::IntoIter,
    value: Option<Value>,
}

impl<'de> de::MapAccess<'de> for MapAccess {
    type Error = DeError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, DeError> {
        match self.iter.next() {
            Some((key, value)) => {
                self.value = Some(value);
                seed.deserialize(key.into_deserializer()).map(Some)
            }
            None => Ok(None),
        }
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, DeError> {
        let value = self.value.take().expect("next_key_seed ran first");
        seed.deserialize(ValueDeserializer { value })
    }
}

struct EnumAccess {
    variant: String,
    value: Value,
}

impl<'de> de::EnumAccess<'de> for EnumAccess {
    type Error = DeError;
    type Variant = VariantAccess;

    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, VariantAccess), DeError> {
        let variant = seed.deserialize(self.variant.into_deserializer())?;
        Ok((variant, VariantAccess { value: self.value }))
    }
}

struct VariantAccess {
    value: Value,
}

impl<'de> de::VariantAccess<'de> for VariantAccess {
    type Error = DeError;

    fn unit_variant(self) -> Result<(), DeError> {
        Err(de::Error::custom(
            "this variant takes no value, so write its name instead of a table",
        ))
    }

    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, DeError> {
        seed.deserialize(ValueDeserializer { value: self.value })
    }

    fn tuple_variant<V: de::Visitor<'de>>(
        self,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        de::Deserializer::deserialize_any(ValueDeserializer { value: self.value }, visitor)
    }

    fn struct_variant<V: de::Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        de::Deserializer::deserialize_any(ValueDeserializer { value: self.value }, visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Table;
    use serde::Deserialize;
    use std::collections::BTreeMap;

    fn v(kind: ValueKind) -> Value {
        Value::new(kind, Provenance::env("MY_APP_SERVER__PORT"))
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Server {
        host: String,
        port: u16,
        tags: Vec<String>,
        #[serde(default)]
        note: Option<String>,
        labels: BTreeMap<String, String>,
    }

    fn server_table() -> Value {
        let mut labels = Table::open();
        labels.insert("tier", v(ValueKind::String("edge".into())));

        let mut t = Table::open();
        t.insert("host", v(ValueKind::String("127.0.0.1".into())));
        t.insert("port", v(ValueKind::Integer(8080)));
        t.insert(
            "tags",
            v(ValueKind::Array(vec![v(ValueKind::String("a".into()))])),
        );
        t.insert("note", v(ValueKind::Unset));
        t.insert("labels", v(ValueKind::Table(labels)));
        v(ValueKind::Table(t))
    }

    #[test]
    fn a_table_deserialises_into_the_declared_type() {
        let got: Server = from_value(server_table()).unwrap();
        assert_eq!(got.host, "127.0.0.1");
        assert_eq!(got.port, 8080);
        assert_eq!(got.tags, vec!["a".to_string()]);
        assert_eq!(got.note, None, "an unset value is a None, not an error");
        assert_eq!(got.labels["tier"], "edge");
    }

    #[test]
    fn a_wrong_type_reports_where_the_wrong_value_came_from() {
        let mut t = Table::open();
        t.insert("host", v(ValueKind::String("x".into())));
        t.insert("port", v(ValueKind::String("eighty".into())));
        t.insert("tags", v(ValueKind::Array(vec![])));
        t.insert("labels", v(ValueKind::Table(Table::open())));

        let err = from_value::<Server>(v(ValueKind::Table(t))).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid type"), "{msg}");
        assert!(msg.contains("$MY_APP_SERVER__PORT"), "{msg}");
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum Level {
        Warn,
        Error,
    }

    #[test]
    fn an_enum_reads_from_a_plain_string() {
        let got: Level = from_value(v(ValueKind::String("warn".into()))).unwrap();
        assert_eq!(got, Level::Warn);
    }

    #[test]
    fn an_integer_fits_a_float_field_and_an_out_of_range_integer_does_not() {
        #[derive(Debug, Deserialize)]
        struct Small {
            #[allow(dead_code)]
            n: u8,
        }
        let mut t = Table::open();
        t.insert("n", v(ValueKind::Integer(300)));
        let err = from_value::<Small>(v(ValueKind::Table(t))).unwrap_err();
        assert!(err.to_string().contains("300"), "{err}");

        #[derive(Deserialize)]
        struct Ratio {
            r: f64,
        }
        let mut t = Table::open();
        t.insert("r", v(ValueKind::Integer(2)));
        let got: Ratio = from_value(v(ValueKind::Table(t))).unwrap();
        assert!((got.r - 2.0).abs() < f64::EPSILON);
    }
}
