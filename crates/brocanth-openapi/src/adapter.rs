//! The one module that knows about `brocanth-schema`.
//!
//! # Why this is quarantined, and what to do when the sibling crate lands
//!
//! `brocanth-schema` is being written in parallel with this crate. At the
//! time this was written its `src/node.rs` existed and its `src/lib.rs` did
//! not, so the crate does not build yet and depending on it would have made
//! this crate's test suite hostage to another crate's half hour.
//!
//! So this module holds a **mirror** of `brocanth_schema::node`: the same
//! enum, the same variant names, the same field names and the same
//! documented meanings, taken from that crate's source. Everything else in
//! `brocanth-openapi` talks to [`Schema`] and [`SchemaSource`], which are
//! ours. Wiring the real crate up is three edits, all in this file:
//!
//! 1. Add `brocanth-schema = { path = "../brocanth-schema", default-features
//!    = false }` to `Cargo.toml`. That crate's own manifest says the
//!    featureless build is "what `brocanth-openapi` and `brocanth-admin`
//!    want", which is exactly right: no regex engine, no derive, no
//!    framework.
//! 2. Delete the mirrored types below and write
//!    `pub use brocanth_schema::node::*;` in their place.
//! 3. Fix whatever the compiler then points at in [`schema_for`], which is
//!    the only function that reads a node.
//!
//! # What this crate assumes, stated so it can be contradicted
//!
//! Read from `brocanth-schema`'s source rather than guessed, but still
//! assumptions, because that source is moving:
//!
//! 1. **A node is a tree and owns its children.** There is no registry to
//!    consult and no cycle to detect, so a recursive conversion terminates.
//! 2. **Optionality and nullability are different facts**, carried
//!    separately: `FieldNode::required` is the field's, and
//!    `SchemaNode::Nullable` is the value's. OpenAPI needs both and needs
//!    them apart — one goes in the object's `required` list, the other in the
//!    value's `type` array.
//! 3. **A node has no name.** Nothing in `SchemaNode` says "this is a
//!    `Post`", so this crate cannot decide on its own what belongs in
//!    `components/schemas`. The caller names it:
//!    `.component("Post", schema_for(&Post::describe()))`, and then refers to
//!    it with `Schema::reference("Post")`. That is a real limitation and the
//!    alternative — inventing names from Rust type paths — produces
//!    `crate__models__post__Post` in a public API document.
//! 4. **`StringNode::pattern` is present whether or not the `pattern`
//!    feature is on**, which that crate's manifest states explicitly. So this
//!    conversion never needs a regex engine to copy a pattern into a
//!    document.
//! 5. **`Refined` and `Transformed` describe the value on the wire** in their
//!    `inner`. A predicate no description can express is not describable in
//!    JSON Schema either, so `Refined` contributes its `description` and
//!    nothing more, and `Transformed` is its `inner` — what a client sends is
//!    what a document describes.

use std::borrow::Cow;

use crate::schema::{Additional, Schema};

// ---------------------------------------------------------------------------
// Mirror of `brocanth_schema::node`. Delete when the dependency is added.
// ---------------------------------------------------------------------------

/// The description of one schema.
///
/// A mirror of `brocanth_schema::node::SchemaNode`. See the module
/// documentation.
#[derive(Clone, Debug, PartialEq)]
pub enum SchemaNode {
    /// Text, with the string constraints.
    String(StringNode),
    /// A whole number.
    Integer(IntegerNode),
    /// A number that may have a fractional part.
    Number(NumberNode),
    /// `true` or `false`.
    Boolean,
    /// A calendar date, `YYYY-MM-DD`.
    Date(RangeNode),
    /// A wall-clock time, `HH:MM[:SS[.fff]]`.
    Time(RangeNode),
    /// A date and a time, optionally with an offset.
    DateTime(RangeNode),
    /// One of a fixed set of strings.
    Enum(EnumNode),
    /// Exactly one value.
    Literal(LiteralValue),
    /// A sequence, with one schema for every element.
    Array(Box<ArrayNode>),
    /// A set of named fields.
    Object(Box<ObjectNode>),
    /// The field may be absent.
    Optional(Box<SchemaNode>),
    /// The value may be null.
    Nullable(Box<SchemaNode>),
    /// The field may be absent, and this is what it means when it is.
    Default {
        /// What is validated when the field is present.
        inner: Box<SchemaNode>,
        /// What the field means when it is absent.
        value: LiteralValue,
    },
    /// Any one of these.
    Union(Vec<SchemaNode>),
    /// All of these at once.
    Intersection(Vec<SchemaNode>),
    /// An inner schema plus a predicate no description can express.
    Refined {
        /// What is validated before the predicate runs.
        inner: Box<SchemaNode>,
        /// The code the predicate reports, which is also its name.
        code: Cow<'static, str>,
        /// One line for a human.
        description: Option<Cow<'static, str>>,
    },
    /// An inner schema whose accepted value is then converted.
    Transformed {
        /// The schema the wire value must satisfy.
        inner: Box<SchemaNode>,
        /// The Rust type the conversion produces.
        output: Cow<'static, str>,
    },
}

/// The string constraints.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StringNode {
    /// Fewest characters allowed.
    pub min_length: Option<usize>,
    /// Most characters allowed.
    pub max_length: Option<usize>,
    /// Exactly this many characters.
    pub exact_length: Option<usize>,
    /// A regular expression the whole value must match.
    pub pattern: Option<String>,
    /// A named format.
    pub format: Option<StringFormat>,
    /// A required prefix.
    pub starts_with: Option<String>,
    /// A required suffix.
    pub ends_with: Option<String>,
    /// A required substring.
    pub contains: Option<String>,
}

/// The named string formats, which map one-to-one onto OpenAPI's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StringFormat {
    /// An email address.
    Email,
    /// An absolute URL.
    Url,
    /// A hyphenated UUID.
    Uuid,
}

impl StringFormat {
    /// The OpenAPI `format` keyword: `email`, `uri`, `uuid`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Email => "email",
            Self::Url => "uri",
            Self::Uuid => "uuid",
        }
    }
}

/// The whole-number constraints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IntegerNode {
    /// Smallest value allowed, inclusive.
    pub min: Option<i64>,
    /// Largest value allowed, inclusive.
    pub max: Option<i64>,
    /// The value must be strictly greater than this.
    pub exclusive_min: Option<i64>,
    /// The value must be strictly less than this.
    pub exclusive_max: Option<i64>,
    /// The value must be a multiple of this.
    pub multiple_of: Option<i64>,
}

/// The fractional-number constraints.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NumberNode {
    /// Smallest value allowed, inclusive.
    pub min: Option<f64>,
    /// Largest value allowed, inclusive.
    pub max: Option<f64>,
    /// The value must be strictly greater than this.
    pub exclusive_min: Option<f64>,
    /// The value must be strictly less than this.
    pub exclusive_max: Option<f64>,
    /// The value must be a multiple of this.
    pub multiple_of: Option<f64>,
    /// The value must have no fractional part.
    pub integer: bool,
    /// The value must be neither `NaN` nor an infinity.
    pub finite: bool,
}

/// The bounds on a date, a time or a date-time, in their ISO 8601 text form.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RangeNode {
    /// Earliest value allowed, inclusive.
    pub min: Option<String>,
    /// Latest value allowed, inclusive.
    pub max: Option<String>,
}

/// A fixed set of strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnumNode {
    /// Every accepted value, in declaration order.
    pub variants: Vec<Cow<'static, str>>,
}

/// A single constant value.
#[derive(Clone, Debug, PartialEq)]
pub enum LiteralValue {
    /// A string.
    String(String),
    /// A whole number.
    Integer(i64),
    /// A number with a fractional part.
    Number(f64),
    /// A boolean.
    Boolean(bool),
    /// Null.
    Null,
}

/// A sequence and its constraints.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrayNode {
    /// The schema every element must satisfy.
    pub items: SchemaNode,
    /// Fewest elements allowed.
    pub min_items: Option<usize>,
    /// Most elements allowed.
    pub max_items: Option<usize>,
    /// Exactly this many elements.
    pub exact_items: Option<usize>,
    /// No two elements may be equal.
    pub unique: bool,
}

/// A set of named fields.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjectNode {
    /// The declared fields, in declaration order.
    pub fields: Vec<FieldNode>,
    /// What happens to a key nobody declared.
    pub unknown_keys: UnknownKeys,
}

/// One field of an object.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldNode {
    /// The key on the wire.
    pub name: Cow<'static, str>,
    /// `false` when the schema is optional or carries a default.
    pub required: bool,
    /// What the value must satisfy.
    pub schema: SchemaNode,
    /// One line for a human.
    pub description: Option<Cow<'static, str>>,
}

/// What an object schema does with a key it does not declare.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UnknownKeys {
    /// Report an `unknown_key` issue. The default.
    #[default]
    Reject,
    /// Drop it without a word.
    Ignore,
    /// Keep it, unvalidated.
    Passthrough,
}

// ---------------------------------------------------------------------------
// The conversion. This is the part that is real work.
// ---------------------------------------------------------------------------

/// The OpenAPI schema for a description node.
///
/// ```
/// use brocanth_openapi::adapter::{schema_for, SchemaNode, StringNode};
///
/// let node = SchemaNode::Nullable(Box::new(SchemaNode::String(StringNode {
///     max_length: Some(80),
///     ..StringNode::default()
/// })));
/// let json = schema_for(&node).to_json();
/// assert_eq!(json.get("maxLength").unwrap().to_json(), "80");
/// ```
#[must_use]
pub fn schema_for(node: &SchemaNode) -> Schema {
    match node {
        SchemaNode::String(text) => string_schema(text),
        SchemaNode::Integer(number) => integer_schema(*number),
        SchemaNode::Number(number) => number_schema(*number),
        SchemaNode::Boolean => Schema::boolean(),
        SchemaNode::Date(range) => dated_schema("date", range),
        SchemaNode::Time(range) => dated_schema("time", range),
        SchemaNode::DateTime(range) => dated_schema("date-time", range),
        SchemaNode::Enum(choice) => {
            Schema::string().string_enum(choice.variants.iter().map(|v| v.to_string()))
        }
        // JSON Schema has `const`, and `enum` with one member means the same
        // thing to every validator and every generator. Using `enum` keeps
        // one code path for "a fixed set of values".
        SchemaNode::Literal(value) => literal_schema(value),
        SchemaNode::Array(array) => {
            let mut schema = Schema::array(schema_for(&array.items));
            if let Some(exact) = array.exact_items {
                schema = schema.min_items(exact as u64).max_items(exact as u64);
            } else {
                if let Some(min) = array.min_items {
                    schema = schema.min_items(min as u64);
                }
                if let Some(max) = array.max_items {
                    schema = schema.max_items(max as u64);
                }
            }
            if array.unique {
                schema = schema.unique_items();
            }
            schema
        }
        SchemaNode::Object(object) => object_schema(object),
        // An optional *field* is a name missing from `required`, which the
        // object conversion handles. An optional node reached any other way
        // says nothing a schema can carry, so the wrapper is transparent.
        SchemaNode::Optional(inner) => schema_for(inner),
        SchemaNode::Nullable(inner) => schema_for(inner).nullable(),
        SchemaNode::Default { inner, value } => {
            let schema = schema_for(inner);
            match literal_json(value) {
                Some(json) => schema.default_value(json),
                None => schema,
            }
        }
        // `Union` is "any one of these"; `oneOf` is "exactly one of these".
        // `anyOf` is the honest translation, and the stricter keyword would
        // reject a value two branches both accept.
        SchemaNode::Union(parts) => Schema::any_of(parts.iter().map(schema_for).collect()),
        SchemaNode::Intersection(parts) => Schema::all_of(parts.iter().map(schema_for).collect()),
        SchemaNode::Refined {
            inner, description, ..
        } => {
            let schema = schema_for(inner);
            match description {
                Some(text) => schema.description(text.to_string()),
                None => schema,
            }
        }
        // What a client sends is what the document describes. `output` is the
        // Rust type the server converts to, which is not on the wire.
        SchemaNode::Transformed { inner, .. } => schema_for(inner),
    }
}

/// Every schema a description produces, as a [`Schema`].
///
/// Equivalent to [`schema_for`]; present because `From` is the shape a caller
/// expects and a free function is the shape a `map` call wants.
impl From<&SchemaNode> for Schema {
    fn from(node: &SchemaNode) -> Self {
        schema_for(node)
    }
}

impl From<SchemaNode> for Schema {
    fn from(node: SchemaNode) -> Self {
        schema_for(&node)
    }
}

fn string_schema(text: &StringNode) -> Schema {
    let mut schema = Schema::string();
    if let Some(exact) = text.exact_length {
        schema = schema.min_length(exact as u64).max_length(exact as u64);
    } else {
        if let Some(min) = text.min_length {
            schema = schema.min_length(min as u64);
        }
        if let Some(max) = text.max_length {
            schema = schema.max_length(max as u64);
        }
    }
    if let Some(pattern) = &text.pattern {
        schema = schema.pattern(pattern.clone());
    }
    if let Some(format) = text.format {
        schema = schema.format(format.as_str());
    }
    // `startsWith`, `endsWith` and `contains` have no JSON Schema keyword for
    // strings. Dropping them silently would make the document claim less than
    // the server enforces, so they are written into the description, where a
    // human reads them and a generator ignores them.
    let mut notes = Vec::new();
    if let Some(prefix) = &text.starts_with {
        notes.push(format!("starts with `{prefix}`"));
    }
    if let Some(suffix) = &text.ends_with {
        notes.push(format!("ends with `{suffix}`"));
    }
    if let Some(needle) = &text.contains {
        notes.push(format!("contains `{needle}`"));
    }
    if !notes.is_empty() {
        schema = schema.description(format!("Must be a value that {}.", notes.join(", ")));
    }
    schema
}

#[allow(clippy::cast_precision_loss)]
fn integer_schema(number: IntegerNode) -> Schema {
    let mut schema = Schema::integer();
    if let Some(min) = number.min {
        schema = schema.minimum(min as f64);
    }
    if let Some(max) = number.max {
        schema = schema.maximum(max as f64);
    }
    if let Some(min) = number.exclusive_min {
        schema = schema.exclusive_minimum(min as f64);
    }
    if let Some(max) = number.exclusive_max {
        schema = schema.exclusive_maximum(max as f64);
    }
    if let Some(step) = number.multiple_of {
        schema = schema.multiple_of(step as f64);
    }
    schema
}

fn number_schema(number: NumberNode) -> Schema {
    // `integer: true` on a fractional node is the same constraint OpenAPI
    // spells `type: integer`, so it changes the type rather than adding a
    // keyword nobody checks.
    let mut schema = if number.integer {
        Schema::integer()
    } else {
        Schema::number()
    };
    if let Some(min) = number.min {
        schema = schema.minimum(min);
    }
    if let Some(max) = number.max {
        schema = schema.maximum(max);
    }
    if let Some(min) = number.exclusive_min {
        schema = schema.exclusive_minimum(min);
    }
    if let Some(max) = number.exclusive_max {
        schema = schema.exclusive_maximum(max);
    }
    if let Some(step) = number.multiple_of {
        schema = schema.multiple_of(step);
    }
    schema
}

fn dated_schema(format: &str, range: &RangeNode) -> Schema {
    let mut schema = Schema::string().format(format);
    // JSON Schema has no bounds on a formatted string, so the bounds go where
    // a human can read them rather than nowhere.
    let note = match (&range.min, &range.max) {
        (Some(min), Some(max)) => Some(format!("Between {min} and {max}, inclusive.")),
        (Some(min), None) => Some(format!("Not before {min}.")),
        (None, Some(max)) => Some(format!("Not after {max}.")),
        (None, None) => None,
    };
    if let Some(note) = note {
        schema = schema.description(note);
    }
    schema
}

fn object_schema(object: &ObjectNode) -> Schema {
    let mut schema = Schema::object();
    for field in &object.fields {
        let mut value = schema_for(&field.schema);
        if let Some(description) = &field.description {
            value = value.description(description.to_string());
        }
        schema = if field.required {
            schema.required(field.name.to_string(), value)
        } else {
            schema.property(field.name.to_string(), value)
        };
    }
    match object.unknown_keys {
        // `Reject` is the vocabulary's default and `additionalProperties:
        // false` is its exact JSON Schema spelling, so saying so costs one
        // line and tells a client generator to emit a closed model.
        UnknownKeys::Reject => schema.additional_properties(Additional::Denied),
        UnknownKeys::Ignore | UnknownKeys::Passthrough => schema,
    }
}

fn literal_schema(value: &LiteralValue) -> Schema {
    let base = match value {
        LiteralValue::String(_) => Schema::string(),
        LiteralValue::Integer(_) => Schema::integer(),
        LiteralValue::Number(_) => Schema::number(),
        LiteralValue::Boolean(_) => Schema::boolean(),
        LiteralValue::Null => return Schema::any().enumeration(vec![crate::json::Json::Null]),
    };
    match literal_json(value) {
        Some(json) => base.enumeration(vec![json]),
        None => base,
    }
}

fn literal_json(value: &LiteralValue) -> Option<crate::json::Json> {
    use crate::json::Json;
    Some(match value {
        LiteralValue::String(text) => Json::Str(text.clone()),
        LiteralValue::Integer(number) => Json::Int(*number),
        LiteralValue::Number(number) if number.is_finite() => Json::Float(*number),
        LiteralValue::Number(_) => return None,
        LiteralValue::Boolean(flag) => Json::Bool(*flag),
        LiteralValue::Null => Json::Null,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::Json;

    fn post() -> SchemaNode {
        SchemaNode::Object(Box::new(ObjectNode {
            fields: vec![
                FieldNode {
                    name: "id".into(),
                    required: true,
                    schema: SchemaNode::Integer(IntegerNode {
                        min: Some(1),
                        ..IntegerNode::default()
                    }),
                    description: None,
                },
                FieldNode {
                    name: "title".into(),
                    required: true,
                    schema: SchemaNode::String(StringNode {
                        max_length: Some(120),
                        ..StringNode::default()
                    }),
                    description: Some("The headline.".into()),
                },
                FieldNode {
                    name: "body".into(),
                    required: true,
                    schema: SchemaNode::Nullable(Box::new(SchemaNode::String(
                        StringNode::default(),
                    ))),
                    description: None,
                },
                FieldNode {
                    name: "published_at".into(),
                    required: false,
                    schema: SchemaNode::Optional(Box::new(SchemaNode::DateTime(
                        RangeNode::default(),
                    ))),
                    description: None,
                },
            ],
            unknown_keys: UnknownKeys::Reject,
        }))
    }

    #[test]
    fn required_comes_from_the_field_and_nullable_from_the_value() {
        let json = schema_for(&post()).to_json();
        assert_eq!(
            json.get("required"),
            Some(&Json::Array(vec![
                Json::Str("id".into()),
                Json::Str("title".into()),
                Json::Str("body".into()),
            ]))
        );
        let properties = json.get("properties").unwrap();
        // Required and nullable.
        assert_eq!(
            properties.get("body").unwrap().get("type"),
            Some(&Json::Array(vec![
                Json::Str("string".into()),
                Json::Str("null".into())
            ]))
        );
        // Optional and not nullable.
        assert_eq!(
            properties.get("published_at").unwrap().get("type"),
            Some(&Json::Str("string".into()))
        );
        assert_eq!(
            properties
                .get("published_at")
                .unwrap()
                .get("format")
                .and_then(Json::as_str),
            Some("date-time")
        );
    }

    #[test]
    fn rejecting_unknown_keys_closes_the_model() {
        let json = schema_for(&post()).to_json();
        assert_eq!(json.get("additionalProperties"), Some(&Json::Bool(false)));
    }

    #[test]
    fn a_field_description_lands_on_the_property() {
        let json = schema_for(&post()).to_json();
        assert_eq!(
            json.get("properties")
                .unwrap()
                .get("title")
                .unwrap()
                .get("description")
                .and_then(Json::as_str),
            Some("The headline.")
        );
    }

    #[test]
    fn a_union_is_any_of_rather_than_one_of() {
        let node = SchemaNode::Union(vec![
            SchemaNode::Boolean,
            SchemaNode::Integer(IntegerNode::default()),
        ]);
        let json = schema_for(&node).to_json();
        assert!(json.get("oneOf").is_none());
        assert_eq!(json.get("anyOf").unwrap().as_array().unwrap().len(), 2);
    }

    #[test]
    fn a_default_becomes_a_default_keyword() {
        let node = SchemaNode::Default {
            inner: Box::new(SchemaNode::Integer(IntegerNode::default())),
            value: LiteralValue::Integer(25),
        };
        assert_eq!(
            schema_for(&node).to_json().get("default"),
            Some(&Json::Int(25))
        );
    }

    #[test]
    fn a_literal_becomes_a_one_member_enum() {
        let node = SchemaNode::Literal(LiteralValue::String("draft".into()));
        let json = schema_for(&node).to_json();
        assert_eq!(
            json.get("enum"),
            Some(&Json::Array(vec![Json::Str("draft".into())]))
        );
    }

    #[test]
    fn an_exact_length_is_both_bounds() {
        let node = SchemaNode::String(StringNode {
            exact_length: Some(6),
            ..StringNode::default()
        });
        let json = schema_for(&node).to_json();
        assert_eq!(json.get("minLength"), Some(&Json::Int(6)));
        assert_eq!(json.get("maxLength"), Some(&Json::Int(6)));
    }

    #[test]
    fn constraints_with_no_keyword_go_into_the_description() {
        let node = SchemaNode::String(StringNode {
            starts_with: Some("https://".into()),
            ..StringNode::default()
        });
        let json = schema_for(&node).to_json();
        assert!(json
            .get("description")
            .and_then(Json::as_str)
            .unwrap()
            .contains("https://"));
    }

    #[test]
    fn a_transformed_node_describes_what_the_client_sends() {
        let node = SchemaNode::Transformed {
            inner: Box::new(SchemaNode::String(StringNode {
                format: Some(StringFormat::Uuid),
                ..StringNode::default()
            })),
            output: "uuid::Uuid".into(),
        };
        let json = schema_for(&node).to_json();
        assert_eq!(json.get("type"), Some(&Json::Str("string".into())));
        assert_eq!(json.get("format").and_then(Json::as_str), Some("uuid"));
    }
}
