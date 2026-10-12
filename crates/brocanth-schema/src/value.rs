//! `Value`: the shape data has before it is a struct.
//!
//! A derived schema validates a struct and needs none of this. The
//! function-style builder validates a document whose fields are not known to
//! the compiler — a JSON body, a form submission, a config blob — and that
//! needs a value type.
//!
//! It is JSON-shaped and it is ours, so this crate does not pull in a
//! serialiser to describe a validation rule. With the `serde` feature on, a
//! `serde_json::Value` converts into one in a few lines on the caller's side;
//! without it, the parser that produced the body builds a `Value` directly.
//!
//! Object fields are a `Vec` of pairs rather than a map, so the order a client
//! sent its keys is the order its errors come back in. A form that lists its
//! errors in hash order is a form whose errors move between submissions.

/// A piece of data that has not been turned into a struct yet.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// JSON `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A whole number.
    Int(i64),
    /// A number with a fractional part.
    Float(f64),
    /// Text.
    Text(String),
    /// A sequence.
    Array(Vec<Value>),
    /// Named fields, in the order they arrived.
    Object(Vec<(String, Value)>),
}

impl Value {
    /// Text, from anything string-like.
    pub fn text(value: impl Into<String>) -> Self {
        Value::Text(value.into())
    }

    /// An array, from anything iterable.
    pub fn array(items: impl IntoIterator<Item = Value>) -> Self {
        Value::Array(items.into_iter().collect())
    }

    /// An object, from name/value pairs.
    ///
    /// ```
    /// use brocanth_schema::Value;
    /// let v = Value::object([("name", Value::text("ada"))]);
    /// assert_eq!(v.get("name"), Some(&Value::Text("ada".into())));
    /// ```
    pub fn object<K: Into<String>>(fields: impl IntoIterator<Item = (K, Value)>) -> Self {
        Value::Object(fields.into_iter().map(|(k, v)| (k.into(), v)).collect())
    }

    /// The name of this value's kind, for a `wrong_type` issue.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "a boolean",
            Value::Int(_) => "a whole number",
            Value::Float(_) => "a number",
            Value::Text(_) => "text",
            Value::Array(_) => "an array",
            Value::Object(_) => "an object",
        }
    }

    /// One field of an object, or [`None`] for anything else.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Value> {
        match self {
            Value::Object(fields) => fields.iter().find(|(k, _)| k == name).map(|(_, v)| v),
            _ => None,
        }
    }

    /// `true` when this is JSON `null`.
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// The value as it appears in an issue's `received`.
    #[must_use]
    pub fn render(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => crate::issue::render_f64(*f),
            Value::Text(s) => crate::issue::quote_received(s),
            Value::Array(items) => crate::issue::count(items.len(), "element", "elements"),
            Value::Object(fields) => crate::issue::count(fields.len(), "key", "keys"),
        }
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Text(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Text(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<crate::LiteralValue> for Value {
    fn from(value: crate::LiteralValue) -> Self {
        match value {
            crate::LiteralValue::String(s) => Value::Text(s),
            crate::LiteralValue::Integer(i) => Value::Int(i),
            crate::LiteralValue::Number(n) => Value::Float(n),
            crate::LiteralValue::Boolean(b) => Value::Bool(b),
            crate::LiteralValue::Null => Value::Null,
        }
    }
}

// ---------------------------------------------------------------------------
// Reading a schema's input out of a Value
// ---------------------------------------------------------------------------

/// How a schema's input type is found inside a [`Value`].
///
/// This is what lets a typed schema — whose `Input` is `str`, `i64`, `bool` —
/// be stored in an object field next to schemas with a different input, and
/// still report `wrong_type` with the right words rather than panicking.
pub trait ReadValue {
    /// Find `Self` inside `value` and hand it to `f`, or name what was
    /// expected.
    ///
    /// It is a callback rather than a `&Self` return because one case has to
    /// materialise: a float schema reading `Value::Int(1)` must see `1.0`, and
    /// there is no `&f64` inside an `i64` to borrow. `1` *is* a number, so
    /// rejecting it as a type error would be wrong.
    ///
    /// # Errors
    ///
    /// The `&'static str` is the noun phrase an issue needs: `text`,
    /// `a whole number`.
    fn with<R>(value: &Value, f: impl FnOnce(&Self) -> R) -> Result<R, &'static str>;
}

impl ReadValue for str {
    fn with<R>(value: &Value, f: impl FnOnce(&Self) -> R) -> Result<R, &'static str> {
        match value {
            Value::Text(s) => Ok(f(s)),
            _ => Err("text"),
        }
    }
}

impl ReadValue for i64 {
    fn with<R>(value: &Value, f: impl FnOnce(&Self) -> R) -> Result<R, &'static str> {
        match value {
            Value::Int(i) => Ok(f(i)),
            _ => Err("a whole number"),
        }
    }
}

impl ReadValue for f64 {
    fn with<R>(value: &Value, f: impl FnOnce(&Self) -> R) -> Result<R, &'static str> {
        match value {
            Value::Float(n) => Ok(f(n)),
            Value::Int(i) => Ok(f(&(*i as f64))),
            _ => Err("a number"),
        }
    }
}

impl ReadValue for bool {
    fn with<R>(value: &Value, f: impl FnOnce(&Self) -> R) -> Result<R, &'static str> {
        match value {
            Value::Bool(b) => Ok(f(b)),
            _ => Err("a boolean"),
        }
    }
}

impl ReadValue for [Value] {
    fn with<R>(value: &Value, f: impl FnOnce(&Self) -> R) -> Result<R, &'static str> {
        match value {
            Value::Array(items) => Ok(f(items)),
            _ => Err("an array"),
        }
    }
}

impl ReadValue for Value {
    fn with<R>(value: &Value, f: impl FnOnce(&Self) -> R) -> Result<R, &'static str> {
        Ok(f(value))
    }
}
