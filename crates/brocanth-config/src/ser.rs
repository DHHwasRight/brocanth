//! Turning a plugin's defaults into a [`Value`], and learning its key set on
//! the way.
//!
//! # Why a serialiser and not a schema
//!
//! A plugin's defaults have to become a layer of the document, the lowest one,
//! so that the merge rule covers them and the dump can print them beside the
//! values that beat them. Something must convert `LoggerConfig::default()` into
//! the model, and serde's `Serialize` is the one description of that type that
//! already exists, is derived, and cannot drift from its `Deserialize`.
//!
//! # The trick that makes an unclaimed key detectable
//!
//! serde distinguishes a struct from a map *in the trait*: a Rust struct calls
//! `serialize_struct` and a `HashMap` calls `serialize_map`. So this serialiser
//! learns, for free and with no declaration from the plugin author, which
//! tables have a complete key set ([`Table::closed`]) and which accept anything
//! ([`Table::open`]).
//!
//! That is the whole basis of the unclaimed-key check: a key a file sets inside
//! a closed table is a key the plugin does not have, and a key inside an open
//! one is an entry in a map. No schema, no registration, no second vocabulary
//! to keep in step.

use std::fmt;

use serde::ser::{self, Serialize};

use crate::value::{Provenance, Table, Value, ValueKind};

/// A defaults value that cannot be put in the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerError(String);

impl fmt::Display for SerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SerError {}

impl ser::Error for SerError {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self(msg.to_string())
    }
}

/// Serialises `value` into the model, attributing every node to `from`.
///
/// # Errors
///
/// [`SerError`] when the type holds something the model has no room for: an
/// integer past `i64`, or a map whose keys are not strings.
pub fn to_value<T: Serialize>(value: &T, from: Provenance) -> Result<Value, SerError> {
    value.serialize(ValueSerializer { from })
}

/// Serialises a value into the model, with one provenance for every node.
pub struct ValueSerializer {
    from: Provenance,
}

impl ValueSerializer {
    fn wrap(&self, kind: ValueKind) -> Value {
        Value::new(kind, self.from.clone())
    }
}

macro_rules! scalar {
    ($method:ident, $ty:ty, $kind:expr) => {
        fn $method(self, v: $ty) -> Result<Value, SerError> {
            let f = $kind;
            Ok(self.wrap(f(v)))
        }
    };
}

impl ser::Serializer for ValueSerializer {
    type Ok = Value;
    type Error = SerError;
    type SerializeSeq = SerializeArray;
    type SerializeTuple = SerializeArray;
    type SerializeTupleStruct = SerializeArray;
    type SerializeTupleVariant = SerializeVariantArray;
    type SerializeMap = SerializeTable;
    type SerializeStruct = SerializeTable;
    type SerializeStructVariant = SerializeVariantTable;

    scalar!(serialize_bool, bool, ValueKind::Boolean);
    scalar!(serialize_i8, i8, |v| ValueKind::Integer(i64::from(v)));
    scalar!(serialize_i16, i16, |v| ValueKind::Integer(i64::from(v)));
    scalar!(serialize_i32, i32, |v| ValueKind::Integer(i64::from(v)));
    scalar!(serialize_i64, i64, ValueKind::Integer);
    scalar!(serialize_u8, u8, |v| ValueKind::Integer(i64::from(v)));
    scalar!(serialize_u16, u16, |v| ValueKind::Integer(i64::from(v)));
    scalar!(serialize_u32, u32, |v| ValueKind::Integer(i64::from(v)));
    scalar!(serialize_f32, f32, |v| ValueKind::Float(f64::from(v)));
    scalar!(serialize_f64, f64, ValueKind::Float);

    fn serialize_u64(self, v: u64) -> Result<Value, SerError> {
        let n = i64::try_from(v).map_err(|_| SerError(format!("{v} does not fit an integer")))?;
        Ok(self.wrap(ValueKind::Integer(n)))
    }

    fn serialize_i128(self, v: i128) -> Result<Value, SerError> {
        let n = i64::try_from(v).map_err(|_| SerError(format!("{v} does not fit an integer")))?;
        Ok(self.wrap(ValueKind::Integer(n)))
    }

    fn serialize_u128(self, v: u128) -> Result<Value, SerError> {
        let n = i64::try_from(v).map_err(|_| SerError(format!("{v} does not fit an integer")))?;
        Ok(self.wrap(ValueKind::Integer(n)))
    }

    fn serialize_char(self, v: char) -> Result<Value, SerError> {
        Ok(self.wrap(ValueKind::String(v.to_string())))
    }

    fn serialize_str(self, v: &str) -> Result<Value, SerError> {
        Ok(self.wrap(ValueKind::String(v.to_string())))
    }

    fn serialize_bytes(self, v: &[u8]) -> Result<Value, SerError> {
        let items = v
            .iter()
            .map(|b| self.wrap(ValueKind::Integer(i64::from(*b))))
            .collect();
        Ok(self.wrap(ValueKind::Array(items)))
    }

    fn serialize_none(self) -> Result<Value, SerError> {
        Ok(self.wrap(ValueKind::Unset))
    }

    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<Value, SerError> {
        v.serialize(self)
    }

    fn serialize_unit(self) -> Result<Value, SerError> {
        Ok(self.wrap(ValueKind::Unset))
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, SerError> {
        Ok(self.wrap(ValueKind::Unset))
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<Value, SerError> {
        Ok(self.wrap(ValueKind::String(variant.to_string())))
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        v: &T,
    ) -> Result<Value, SerError> {
        v.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<Value, SerError> {
        let inner = v.serialize(ValueSerializer {
            from: self.from.clone(),
        })?;
        let mut table = Table::closed();
        table.insert(variant, inner);
        Ok(self.wrap(ValueKind::Table(table)))
    }

    fn serialize_seq(self, len: Option<usize>) -> Result<SerializeArray, SerError> {
        Ok(SerializeArray {
            items: Vec::with_capacity(len.unwrap_or(0)),
            from: self.from,
        })
    }

    fn serialize_tuple(self, len: usize) -> Result<SerializeArray, SerError> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<SerializeArray, SerError> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<SerializeVariantArray, SerError> {
        Ok(SerializeVariantArray {
            variant,
            inner: SerializeArray {
                items: Vec::with_capacity(len),
                from: self.from,
            },
        })
    }

    /// A map: any key is allowed, so the table is open.
    fn serialize_map(self, _len: Option<usize>) -> Result<SerializeTable, SerError> {
        Ok(SerializeTable {
            table: Table::open(),
            key: None,
            from: self.from,
        })
    }

    /// A struct: the key set is complete, so the table is closed. This is the
    /// line that makes an unclaimed key detectable.
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<SerializeTable, SerError> {
        Ok(SerializeTable {
            table: Table::closed(),
            key: None,
            from: self.from,
        })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<SerializeVariantTable, SerError> {
        Ok(SerializeVariantTable {
            variant,
            inner: SerializeTable {
                table: Table::closed(),
                key: None,
                from: self.from,
            },
        })
    }
}

/// Collects an array.
pub struct SerializeArray {
    items: Vec<Value>,
    from: Provenance,
}

impl SerializeArray {
    fn push<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), SerError> {
        self.items.push(v.serialize(ValueSerializer {
            from: self.from.clone(),
        })?);
        Ok(())
    }

    fn finish(self) -> Value {
        Value::new(ValueKind::Array(self.items), self.from)
    }
}

impl ser::SerializeSeq for SerializeArray {
    type Ok = Value;
    type Error = SerError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), SerError> {
        self.push(v)
    }

    fn end(self) -> Result<Value, SerError> {
        Ok(self.finish())
    }
}

impl ser::SerializeTuple for SerializeArray {
    type Ok = Value;
    type Error = SerError;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), SerError> {
        self.push(v)
    }

    fn end(self) -> Result<Value, SerError> {
        Ok(self.finish())
    }
}

impl ser::SerializeTupleStruct for SerializeArray {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), SerError> {
        self.push(v)
    }

    fn end(self) -> Result<Value, SerError> {
        Ok(self.finish())
    }
}

/// Collects a tuple variant, as a one-key table.
pub struct SerializeVariantArray {
    variant: &'static str,
    inner: SerializeArray,
}

impl ser::SerializeTupleVariant for SerializeVariantArray {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), SerError> {
        self.inner.push(v)
    }

    fn end(self) -> Result<Value, SerError> {
        let from = self.inner.from.clone();
        let mut table = Table::closed();
        table.insert(self.variant, self.inner.finish());
        Ok(Value::new(ValueKind::Table(table), from))
    }
}

/// Collects a table, open or closed.
pub struct SerializeTable {
    table: Table,
    key: Option<String>,
    from: Provenance,
}

impl SerializeTable {
    fn finish(self) -> Value {
        Value::new(ValueKind::Table(self.table), self.from)
    }
}

impl ser::SerializeMap for SerializeTable {
    type Ok = Value;
    type Error = SerError;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), SerError> {
        self.key = Some(key.serialize(KeySerializer)?);
        Ok(())
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, v: &T) -> Result<(), SerError> {
        let key = self
            .key
            .take()
            .ok_or_else(|| SerError("a map value arrived before its key".into()))?;
        let value = v.serialize(ValueSerializer {
            from: self.from.clone(),
        })?;
        self.table.insert(key, value);
        Ok(())
    }

    fn end(self) -> Result<Value, SerError> {
        Ok(self.finish())
    }
}

impl ser::SerializeStruct for SerializeTable {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), SerError> {
        let value = v.serialize(ValueSerializer {
            from: self.from.clone(),
        })?;
        self.table.insert(key, value);
        Ok(())
    }

    fn end(self) -> Result<Value, SerError> {
        Ok(self.finish())
    }
}

/// Collects a struct variant, as a one-key table.
pub struct SerializeVariantTable {
    variant: &'static str,
    inner: SerializeTable,
}

impl ser::SerializeStructVariant for SerializeVariantTable {
    type Ok = Value;
    type Error = SerError;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), SerError> {
        ser::SerializeStruct::serialize_field(&mut self.inner, key, v)
    }

    fn end(self) -> Result<Value, SerError> {
        let from = self.inner.from.clone();
        let mut table = Table::closed();
        table.insert(self.variant, self.inner.finish());
        Ok(Value::new(ValueKind::Table(table), from))
    }
}

/// A map key must be a name. Everything a configuration format can spell as a
/// key is a string, so a key that is not one is rejected here rather than
/// silently stringified into something no file could ever write.
struct KeySerializer;

macro_rules! key_from_display {
    ($method:ident, $ty:ty) => {
        fn $method(self, v: $ty) -> Result<String, SerError> {
            Ok(v.to_string())
        }
    };
}

macro_rules! key_rejected {
    ($method:ident $(, $arg:ty)*) => {
        fn $method(self $(, _: $arg)*) -> Result<String, SerError> {
            Err(SerError("a configuration key must be a name".into()))
        }
    };
}

impl ser::Serializer for KeySerializer {
    type Ok = String;
    type Error = SerError;
    type SerializeSeq = ser::Impossible<String, SerError>;
    type SerializeTuple = ser::Impossible<String, SerError>;
    type SerializeTupleStruct = ser::Impossible<String, SerError>;
    type SerializeTupleVariant = ser::Impossible<String, SerError>;
    type SerializeMap = ser::Impossible<String, SerError>;
    type SerializeStruct = ser::Impossible<String, SerError>;
    type SerializeStructVariant = ser::Impossible<String, SerError>;

    key_from_display!(serialize_bool, bool);
    key_from_display!(serialize_i8, i8);
    key_from_display!(serialize_i16, i16);
    key_from_display!(serialize_i32, i32);
    key_from_display!(serialize_i64, i64);
    key_from_display!(serialize_i128, i128);
    key_from_display!(serialize_u8, u8);
    key_from_display!(serialize_u16, u16);
    key_from_display!(serialize_u32, u32);
    key_from_display!(serialize_u64, u64);
    key_from_display!(serialize_u128, u128);
    key_from_display!(serialize_f32, f32);
    key_from_display!(serialize_f64, f64);
    key_from_display!(serialize_char, char);
    key_from_display!(serialize_str, &str);

    key_rejected!(serialize_bytes, &[u8]);
    key_rejected!(serialize_none);
    key_rejected!(serialize_unit);
    key_rejected!(serialize_unit_struct, &'static str);

    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<String, SerError> {
        v.serialize(self)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<String, SerError> {
        Ok(variant.to_string())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        v: &T,
    ) -> Result<String, SerError> {
        v.serialize(self)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _v: &T,
    ) -> Result<String, SerError> {
        Err(SerError("a configuration key must be a name".into()))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, SerError> {
        Err(SerError("a configuration key must be a name".into()))
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, SerError> {
        Err(SerError("a configuration key must be a name".into()))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, SerError> {
        Err(SerError("a configuration key must be a name".into()))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, SerError> {
        Err(SerError("a configuration key must be a name".into()))
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, SerError> {
        Err(SerError("a configuration key must be a name".into()))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, SerError> {
        Err(SerError("a configuration key must be a name".into()))
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, SerError> {
        Err(SerError("a configuration key must be a name".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use std::collections::BTreeMap;

    #[derive(Serialize)]
    struct Pool {
        max_size: u32,
        timeout_s: u32,
    }

    #[derive(Serialize)]
    struct Db {
        url: String,
        pool: Pool,
        hosts: Vec<String>,
        label: Option<String>,
        tuning: BTreeMap<String, i64>,
    }

    fn defaults() -> Db {
        Db {
            url: "sqlite://app.db".into(),
            pool: Pool {
                max_size: 5,
                timeout_s: 30,
            },
            hosts: vec!["localhost".into()],
            label: None,
            tuning: BTreeMap::from([("cache_kb".to_string(), 2048)]),
        }
    }

    #[test]
    fn a_struct_becomes_a_closed_table_and_a_map_an_open_one() {
        let value = to_value(&defaults(), Provenance::default_of("db")).unwrap();
        let table = value.as_table().unwrap();
        assert!(!table.is_open(), "a struct names every key it has");
        assert!(
            !table["pool"].as_table().unwrap().is_open(),
            "and so does a nested struct"
        );
        assert!(
            table["tuning"].as_table().unwrap().is_open(),
            "a map takes any key"
        );
    }

    #[test]
    fn every_node_carries_the_plugins_name() {
        let value = to_value(&defaults(), Provenance::default_of("db")).unwrap();
        for (_, leaf) in value.leaves() {
            assert_eq!(leaf.provenance(), &Provenance::default_of("db"));
        }
    }

    #[test]
    fn a_none_default_is_unset_so_the_key_is_claimed_without_a_value() {
        let value = to_value(&defaults(), Provenance::default_of("db")).unwrap();
        let table = value.as_table().unwrap();
        assert!(table.contains_key("label"), "the key is claimed");
        assert!(table["label"].is_unset(), "and it has no value");
    }

    #[test]
    fn values_the_model_has_no_room_for_are_rejected_rather_than_mangled() {
        #[derive(Serialize)]
        struct Big {
            n: u64,
        }
        let err = to_value(&Big { n: u64::MAX }, Provenance::default_of("big")).unwrap_err();
        assert!(err.to_string().contains("does not fit"), "{err}");
    }
}
