//! A small ordered JSON value, with a parser, a JSON writer and a YAML
//! writer.
//!
//! # Why this is not `serde_json`
//!
//! Three reasons, and the third is the one that decided it.
//!
//! 1. **Key order is part of the output.** A document whose keys move between
//!    runs cannot be committed and diffed, and `serde_json` keeps insertion
//!    order only with the `preserve_order` feature, which pulls in
//!    `indexmap`. Here order is the representation: an object *is* a `Vec` of
//!    pairs.
//! 2. **YAML has no maintained serde backend.** `serde_yaml` is archived. The
//!    document is a closed set of shapes — objects, arrays, strings, numbers,
//!    booleans — so a block-style writer is a page of code rather than a
//!    dependency.
//! 3. **This crate is a plugin.** §8 of the plan: a plugin that is not
//!    imported contributes nothing, and a plugin that *is* imported should
//!    contribute as little as it can get away with. Emitting a document does
//!    not justify a serialiser in the tree of every application that wants
//!    one.
//!
//! # Equality
//!
//! [`Json`] compares by value rather than by spelling: `1` equals `1.0`, and
//! two objects with the same pairs in a different order are equal. Drift
//! detection is built on that, so reformatting a committed document is not a
//! change and reordering its keys is not a change — only its content is.

use core::fmt::Write as _;

/// A JSON value that remembers the order its keys were inserted in.
#[derive(Debug, Clone)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A number with no fractional part.
    Int(i64),
    /// A number with a fractional part or an exponent.
    Float(f64),
    /// A string.
    Str(String),
    /// An array.
    Array(Vec<Json>),
    /// An object, in insertion order.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// An empty object builder.
    #[must_use]
    pub fn object() -> Obj {
        Obj::new()
    }

    /// The value at `key`, when this is an object that has one.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The string inside, when this is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The entries, when this is an object.
    #[must_use]
    pub fn as_object(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Object(entries) => Some(entries),
            _ => None,
        }
    }

    /// The items, when this is an array.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The name of this value's kind, for error messages.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "boolean",
            Json::Int(_) | Json::Float(_) => "number",
            Json::Str(_) => "string",
            Json::Array(_) => "array",
            Json::Object(_) => "object",
        }
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            #[allow(clippy::cast_precision_loss)]
            Json::Int(i) => Some(*i as f64),
            Json::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Render as indented JSON. No trailing newline.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        write_json(&mut out, self, 0);
        out
    }

    /// Render as block-style YAML. No trailing newline.
    #[must_use]
    pub fn to_yaml(&self) -> String {
        let mut out = String::new();
        if is_inline(self) {
            out.push_str(&inline_yaml(self));
        } else {
            write_yaml_block(&mut out, self, 0);
            while out.ends_with('\n') {
                out.pop();
            }
        }
        out
    }
}

impl PartialEq for Json {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Json::Null, Json::Null) => true,
            (Json::Bool(a), Json::Bool(b)) => a == b,
            (Json::Str(a), Json::Str(b)) => a == b,
            (Json::Int(_) | Json::Float(_), Json::Int(_) | Json::Float(_)) => {
                // `1` and `1.0` are the same JSON number, so a committed
                // document that spells one of them the other way has not
                // drifted.
                self.as_f64() == other.as_f64()
            }
            (Json::Array(a), Json::Array(b)) => a == b,
            (Json::Object(a), Json::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, value)| b.iter().any(|(k, v)| k == key && v == value))
            }
            _ => false,
        }
    }
}

impl From<&str> for Json {
    fn from(value: &str) -> Self {
        Json::Str(value.to_owned())
    }
}

impl From<String> for Json {
    fn from(value: String) -> Self {
        Json::Str(value)
    }
}

impl From<&String> for Json {
    fn from(value: &String) -> Self {
        Json::Str(value.clone())
    }
}

impl From<bool> for Json {
    fn from(value: bool) -> Self {
        Json::Bool(value)
    }
}

impl From<i64> for Json {
    fn from(value: i64) -> Self {
        Json::Int(value)
    }
}

impl From<u64> for Json {
    #[allow(clippy::cast_possible_wrap)]
    fn from(value: u64) -> Self {
        Json::Int(value as i64)
    }
}

impl From<usize> for Json {
    #[allow(clippy::cast_possible_wrap)]
    fn from(value: usize) -> Self {
        Json::Int(value as i64)
    }
}

impl From<u16> for Json {
    fn from(value: u16) -> Self {
        Json::Int(i64::from(value))
    }
}

impl From<f64> for Json {
    fn from(value: f64) -> Self {
        Json::Float(value)
    }
}

impl From<Vec<Json>> for Json {
    fn from(value: Vec<Json>) -> Self {
        Json::Array(value)
    }
}

/// Builds a [`Json::Object`] without losing the order of its keys.
#[derive(Debug, Default, Clone)]
pub struct Obj {
    entries: Vec<(String, Json)>,
}

impl Obj {
    /// An empty object.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds `key`.
    pub fn set(&mut self, key: &str, value: impl Into<Json>) -> &mut Self {
        self.entries.push((key.to_owned(), value.into()));
        self
    }

    /// Adds `key` only when the value is present.
    pub fn maybe(&mut self, key: &str, value: Option<impl Into<Json>>) -> &mut Self {
        if let Some(value) = value {
            self.set(key, value);
        }
        self
    }

    /// Adds `key` only when the flag is true.
    pub fn flag(&mut self, key: &str, value: bool) -> &mut Self {
        if value {
            self.set(key, true);
        }
        self
    }

    /// Adds `key` only when the list has something in it.
    pub fn list(&mut self, key: &str, values: Vec<Json>) -> &mut Self {
        if !values.is_empty() {
            self.set(key, values);
        }
        self
    }

    /// Whether nothing was added.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Finishes the object.
    #[must_use]
    pub fn build(self) -> Json {
        Json::Object(self.entries)
    }
}

impl From<Obj> for Json {
    fn from(value: Obj) -> Self {
        value.build()
    }
}

// ---------------------------------------------------------------------------
// JSON writer
// ---------------------------------------------------------------------------

fn write_json(out: &mut String, value: &Json, indent: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Json::Float(f) => out.push_str(&float_text(*f)),
        Json::Str(s) => write_json_string(out, s),
        Json::Array(items) if items.is_empty() => out.push_str("[]"),
        Json::Array(items) => {
            out.push_str("[\n");
            for (index, item) in items.iter().enumerate() {
                pad(out, indent + 2);
                write_json(out, item, indent + 2);
                if index + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            pad(out, indent);
            out.push(']');
        }
        Json::Object(entries) if entries.is_empty() => out.push_str("{}"),
        Json::Object(entries) => {
            out.push_str("{\n");
            for (index, (key, item)) in entries.iter().enumerate() {
                pad(out, indent + 2);
                write_json_string(out, key);
                out.push_str(": ");
                write_json(out, item, indent + 2);
                if index + 1 < entries.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            pad(out, indent);
            out.push('}');
        }
    }
}

fn pad(out: &mut String, spaces: usize) {
    for _ in 0..spaces {
        out.push(' ');
    }
}

fn float_text(value: f64) -> String {
    if value.is_finite() {
        let text = format!("{value}");
        // JSON has no `1` that is distinct from `1.0`, but `inf` and `NaN`
        // have no spelling at all; those are rejected before they get here.
        text
    } else {
        "null".to_owned()
    }
}

fn write_json_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------
// YAML writer
// ---------------------------------------------------------------------------

fn is_inline(value: &Json) -> bool {
    match value {
        Json::Array(items) => items.is_empty(),
        Json::Object(entries) => entries.is_empty(),
        _ => true,
    }
}

fn inline_yaml(value: &Json) -> String {
    match value {
        Json::Null => "null".to_owned(),
        Json::Bool(b) => b.to_string(),
        Json::Int(i) => i.to_string(),
        Json::Float(f) => float_text(*f),
        Json::Str(s) => yaml_string(s),
        Json::Array(_) => "[]".to_owned(),
        Json::Object(_) => "{}".to_owned(),
    }
}

fn write_yaml_block(out: &mut String, value: &Json, indent: usize) {
    match value {
        Json::Object(entries) => {
            for (key, item) in entries {
                pad(out, indent);
                out.push_str(&yaml_string(key));
                out.push(':');
                if is_inline(item) {
                    out.push(' ');
                    out.push_str(&inline_yaml(item));
                    out.push('\n');
                } else {
                    out.push('\n');
                    write_yaml_block(out, item, indent + 2);
                }
            }
        }
        Json::Array(items) => {
            for item in items {
                pad(out, indent);
                out.push('-');
                if is_inline(item) {
                    out.push(' ');
                    out.push_str(&inline_yaml(item));
                    out.push('\n');
                } else {
                    out.push(' ');
                    let mut nested = String::new();
                    write_yaml_block(&mut nested, item, indent + 2);
                    // The first line's padding is replaced by the `- ` that
                    // was just written; both are `indent + 2` wide, so the
                    // rest of the block lines up unchanged.
                    out.push_str(&nested[indent + 2..]);
                }
            }
        }
        other => {
            pad(out, indent);
            out.push_str(&inline_yaml(other));
            out.push('\n');
        }
    }
}

/// Quote a YAML scalar unless it is unambiguously plain.
///
/// The test is deliberately strict. A path key such as `/users/{id}`, a
/// reference such as `#/components/schemas/Post` and a status code such as
/// `200` all look like something else to a YAML parser, and quoting costs two
/// characters.
fn yaml_string(text: &str) -> String {
    if plain_is_safe(text) {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn plain_is_safe(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    const RESERVED: [&str; 14] = [
        "true", "false", "null", "yes", "no", "on", "off", "y", "n", "~", "True", "False", "Null",
        "None",
    ];
    if RESERVED.iter().any(|word| word.eq_ignore_ascii_case(text)) {
        return false;
    }
    let mut chars = text.chars();
    let first = chars.next().expect("not empty");
    if !first.is_ascii_alphabetic() {
        return false;
    }
    text.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ' '))
        && !text.ends_with(' ')
        && !text.contains(": ")
        && !text.contains(" #")
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

/// JSON that does not parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message} at byte {offset}")]
pub struct JsonError {
    /// What went wrong.
    pub message: String,
    /// Where it went wrong, as a byte offset into the input.
    pub offset: usize,
}

/// Parse a JSON document.
///
/// # Errors
///
/// [`JsonError`] when the text is not one complete JSON value.
pub fn parse(input: &str) -> Result<Json, JsonError> {
    let mut parser = Parser {
        bytes: input.as_bytes(),
        text: input,
        at: 0,
    };
    parser.skip_ws();
    let value = parser.value()?;
    parser.skip_ws();
    if parser.at != parser.bytes.len() {
        return Err(parser.error("trailing input after the document"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> JsonError {
        JsonError {
            message: message.to_owned(),
            offset: self.at,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), JsonError> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(self.error(&format!("expected `{}`", byte as char)))
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, JsonError> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(value)
        } else {
            Err(self.error("not a JSON value"))
        }
    }

    fn value(&mut self) -> Result<Json, JsonError> {
        match self.peek() {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => self.string().map(Json::Str),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) => Err(self.error("not a JSON value")),
            None => Err(self.error("unexpected end of input")),
        }
    }

    fn object(&mut self) -> Result<Json, JsonError> {
        self.expect(b'{')?;
        let mut entries = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(Json::Object(entries));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    return Ok(Json::Object(entries));
                }
                _ => return Err(self.error("expected `,` or `}`")),
            }
        }
    }

    fn array(&mut self) -> Result<Json, JsonError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.at += 1;
            return Ok(Json::Array(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Json::Array(items));
                }
                _ => return Err(self.error("expected `,` or `]`")),
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let Some(byte) = self.peek() else {
                return Err(self.error("unterminated string"));
            };
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.at += 1;
                    let Some(escape) = self.peek() else {
                        return Err(self.error("unterminated escape"));
                    };
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => return Err(self.error("unknown escape")),
                    }
                }
                _ => {
                    let rest = &self.text[self.at..];
                    let ch = rest.chars().next().ok_or_else(|| self.error("bad UTF-8"))?;
                    self.at += ch.len_utf8();
                    out.push(ch);
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, JsonError> {
        let high = self.hex4()?;
        if (0xD800..0xDC00).contains(&high) {
            if !self.text[self.at..].starts_with("\\u") {
                return Err(self.error("lone surrogate"));
            }
            self.at += 2;
            let low = self.hex4()?;
            if !(0xDC00..0xE000).contains(&low) {
                return Err(self.error("lone surrogate"));
            }
            let combined = 0x1_0000 + ((high - 0xD800) << 10) + (low - 0xDC00);
            return char::from_u32(combined).ok_or_else(|| self.error("invalid code point"));
        }
        char::from_u32(high).ok_or_else(|| self.error("invalid code point"))
    }

    fn hex4(&mut self) -> Result<u32, JsonError> {
        if self.at + 4 > self.bytes.len() {
            return Err(self.error("short unicode escape"));
        }
        let slice = &self.text[self.at..self.at + 4];
        let value = u32::from_str_radix(slice, 16).map_err(|_| self.error("bad unicode escape"))?;
        self.at += 4;
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
            self.at += 1;
        }
        let mut floating = false;
        if self.peek() == Some(b'.') {
            floating = true;
            self.at += 1;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.at += 1;
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            floating = true;
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.at += 1;
            }
        }
        let text = &self.text[start..self.at];
        if floating {
            text.parse::<f64>()
                .map(Json::Float)
                .map_err(|_| self.error("not a number"))
        } else {
            text.parse::<i64>()
                .map(Json::Int)
                .map_err(|_| self.error("not a number"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_object_keeps_its_insertion_order() {
        let mut obj = Json::object();
        obj.set("b", 1_i64).set("a", 2_i64);
        assert_eq!(obj.build().to_json(), "{\n  \"b\": 1,\n  \"a\": 2\n}");
    }

    #[test]
    fn numbers_compare_by_value_not_by_spelling() {
        assert_eq!(Json::Int(1), Json::Float(1.0));
        assert_ne!(Json::Int(1), Json::Float(1.5));
    }

    #[test]
    fn objects_compare_without_regard_to_order() {
        let a = Json::Object(vec![("x".into(), Json::Int(1)), ("y".into(), Json::Int(2))]);
        let b = Json::Object(vec![("y".into(), Json::Int(2)), ("x".into(), Json::Int(1))]);
        assert_eq!(a, b);
    }

    #[test]
    fn round_trip_through_the_parser() {
        let source = r#"{"a":[1,2.5,true,null,"x\ny"],"b":{"c":{}}}"#;
        let value = parse(source).unwrap();
        let again = parse(&value.to_json()).unwrap();
        assert_eq!(value, again);
    }

    #[test]
    fn surrogate_pairs_decode() {
        let value = parse(r#""😀""#).unwrap();
        assert_eq!(value.as_str(), Some("\u{1f600}"));
    }

    #[test]
    fn trailing_input_is_rejected() {
        assert!(parse("{} {}").is_err());
    }

    #[test]
    fn yaml_quotes_anything_a_parser_could_misread() {
        assert_eq!(yaml_string("summary"), "summary");
        assert_eq!(yaml_string("/users/{id}"), "\"/users/{id}\"");
        assert_eq!(yaml_string("200"), "\"200\"");
        assert_eq!(yaml_string("true"), "\"true\"");
        assert_eq!(
            yaml_string("#/components/schemas/Post"),
            "\"#/components/schemas/Post\""
        );
    }

    #[test]
    fn yaml_nests_objects_inside_arrays() {
        let value = Json::Object(vec![(
            "servers".into(),
            Json::Array(vec![Json::Object(vec![
                ("url".into(), Json::Str("/api".into())),
                ("description".into(), Json::Str("local".into())),
            ])]),
        )]);
        assert_eq!(
            value.to_yaml(),
            "servers:\n  - url: \"/api\"\n    description: local"
        );
    }

    #[test]
    fn yaml_writes_empty_containers_inline() {
        let value = Json::Object(vec![
            ("a".into(), Json::Object(Vec::new())),
            ("b".into(), Json::Array(Vec::new())),
        ]);
        assert_eq!(value.to_yaml(), "a: {}\nb: []");
    }
}
