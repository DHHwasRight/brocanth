//! `SchemaNode`: the description a schema hands to whoever asks.
//!
//! # This is a contract, not an implementation detail
//!
//! Two other crates read it and neither of them can validate anything:
//! `brocanth-openapi` turns a node into an OpenAPI schema object, and
//! `brocanth-admin` turns one into form widgets. They depend on this type and
//! this type does not depend on them.
//!
//! **[`SchemaNode`] is deliberately not `#[non_exhaustive]`.** A consumer's
//! `match` is total, so adding a vocabulary item breaks every consumer's
//! build. That is the point: an OpenAPI emitter that silently skips a variant
//! it has never heard of emits a wrong specification, and a form builder that
//! skips one renders a field the user cannot fill. A broken build is the cheap
//! version of both. Adding a variant here is a major version of this crate.
//!
//! # Walking one
//!
//! ```
//! use brocanth_schema::{array, object, string, Schema, SchemaNode};
//!
//! let schema = object()
//!     .field("tags", array(string().min_length(1)).max_items(5));
//!
//! fn depth(node: &SchemaNode) -> usize {
//!     match node {
//!         SchemaNode::Object(o) => 1 + o.fields.iter().map(|f| depth(&f.schema)).max().unwrap_or(0),
//!         SchemaNode::Array(a) => 1 + depth(&a.items),
//!         _ => 1,
//!     }
//! }
//!
//! assert_eq!(depth(&schema.describe()), 3);
//! ```

use std::borrow::Cow;

/// The description of one schema.
///
/// Exhaustive on purpose; see the module documentation.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
        /// One line for a human: a form hint, or a `description` in OpenAPI.
        description: Option<Cow<'static, str>>,
    },
    /// An inner schema whose accepted value is then converted.
    ///
    /// The *input* is what a client sends, so an OpenAPI emitter describes
    /// `inner` and ignores `output`. `output` is the Rust type name, for an
    /// admin UI that wants to say what it becomes.
    Transformed {
        /// The schema the wire value must satisfy.
        inner: Box<SchemaNode>,
        /// The Rust type the conversion produces.
        output: Cow<'static, str>,
    },
}

impl SchemaNode {
    /// The vocabulary item's name: `string`, `array`, `optional`.
    ///
    /// Stable, and the right thing to key a lookup table on.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            SchemaNode::String(_) => "string",
            SchemaNode::Integer(_) => "integer",
            SchemaNode::Number(_) => "number",
            SchemaNode::Boolean => "boolean",
            SchemaNode::Date(_) => "date",
            SchemaNode::Time(_) => "time",
            SchemaNode::DateTime(_) => "date-time",
            SchemaNode::Enum(_) => "enum",
            SchemaNode::Literal(_) => "literal",
            SchemaNode::Array(_) => "array",
            SchemaNode::Object(_) => "object",
            SchemaNode::Optional(_) => "optional",
            SchemaNode::Nullable(_) => "nullable",
            SchemaNode::Default { .. } => "default",
            SchemaNode::Union(_) => "union",
            SchemaNode::Intersection(_) => "intersection",
            SchemaNode::Refined { .. } => "refined",
            SchemaNode::Transformed { .. } => "transformed",
        }
    }

    /// `true` when the field this describes may be left out.
    ///
    /// [`SchemaNode::Nullable`] is not optional: the key must be there, and its
    /// value may be null. That distinction is the whole reason both exist.
    #[must_use]
    pub fn is_optional(&self) -> bool {
        matches!(
            self,
            SchemaNode::Optional(_) | SchemaNode::Default { .. }
        )
    }

    /// Strip [`SchemaNode::Optional`], [`SchemaNode::Nullable`],
    /// [`SchemaNode::Default`], [`SchemaNode::Refined`] and
    /// [`SchemaNode::Transformed`] to reach the item underneath.
    ///
    /// A form builder calls this to decide which widget to draw.
    #[must_use]
    pub fn unwrapped(&self) -> &SchemaNode {
        match self {
            SchemaNode::Optional(inner)
            | SchemaNode::Nullable(inner)
            | SchemaNode::Default { inner, .. }
            | SchemaNode::Refined { inner, .. }
            | SchemaNode::Transformed { inner, .. } => inner.unwrapped(),
            other => other,
        }
    }

    /// Visit this node and every node under it, outermost first.
    pub fn walk(&self, visit: &mut impl FnMut(&SchemaNode)) {
        visit(self);
        match self {
            SchemaNode::Array(a) => a.items.walk(visit),
            SchemaNode::Object(o) => {
                for field in &o.fields {
                    field.schema.walk(visit);
                }
            }
            SchemaNode::Optional(inner)
            | SchemaNode::Nullable(inner)
            | SchemaNode::Default { inner, .. }
            | SchemaNode::Refined { inner, .. }
            | SchemaNode::Transformed { inner, .. } => inner.walk(visit),
            SchemaNode::Union(parts) | SchemaNode::Intersection(parts) => {
                for part in parts {
                    part.walk(visit);
                }
            }
            SchemaNode::String(_)
            | SchemaNode::Integer(_)
            | SchemaNode::Number(_)
            | SchemaNode::Boolean
            | SchemaNode::Date(_)
            | SchemaNode::Time(_)
            | SchemaNode::DateTime(_)
            | SchemaNode::Enum(_)
            | SchemaNode::Literal(_) => {}
        }
    }
}

/// The string constraints.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StringNode {
    /// Fewest characters allowed.
    pub min_length: Option<usize>,
    /// Most characters allowed.
    pub max_length: Option<usize>,
    /// Exactly this many characters.
    pub exact_length: Option<usize>,
    /// A regular expression the whole value must match.
    ///
    /// Present whether or not the `pattern` feature is on, so an emitter never
    /// needs a regex engine to copy it into a specification.
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
    pub fn as_str(self) -> &'static str {
        match self {
            StringFormat::Email => "email",
            StringFormat::Url => "uri",
            StringFormat::Uuid => "uuid",
        }
    }
}

/// The whole-number constraints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RangeNode {
    /// Earliest value allowed, inclusive.
    pub min: Option<String>,
    /// Latest value allowed, inclusive.
    pub max: Option<String>,
}

/// A fixed set of strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EnumNode {
    /// Every accepted value, in declaration order.
    pub variants: Vec<Cow<'static, str>>,
}

/// A single constant value.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

impl LiteralValue {
    /// The literal as it would appear in JSON.
    #[must_use]
    pub fn to_json(&self) -> String {
        match self {
            LiteralValue::String(s) => {
                let mut out = String::new();
                crate::issue::push_json_string(&mut out, s);
                out
            }
            LiteralValue::Integer(i) => i.to_string(),
            LiteralValue::Number(n) => crate::issue::render_f64(*n),
            LiteralValue::Boolean(b) => b.to_string(),
            LiteralValue::Null => "null".to_string(),
        }
    }
}

impl From<&str> for LiteralValue {
    fn from(value: &str) -> Self {
        LiteralValue::String(value.to_string())
    }
}

impl From<String> for LiteralValue {
    fn from(value: String) -> Self {
        LiteralValue::String(value)
    }
}

impl From<i64> for LiteralValue {
    fn from(value: i64) -> Self {
        LiteralValue::Integer(value)
    }
}

impl From<f64> for LiteralValue {
    fn from(value: f64) -> Self {
        LiteralValue::Number(value)
    }
}

impl From<bool> for LiteralValue {
    fn from(value: bool) -> Self {
        LiteralValue::Boolean(value)
    }
}

/// A sequence and its constraints.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ObjectNode {
    /// The declared fields, in declaration order.
    pub fields: Vec<FieldNode>,
    /// What happens to a key nobody declared.
    pub unknown_keys: UnknownKeys,
}

impl ObjectNode {
    /// One declared field by name.
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&FieldNode> {
        self.fields.iter().find(|f| f.name == name)
    }

    /// The names of the fields that must be present — OpenAPI's `required`.
    #[must_use]
    pub fn required(&self) -> Vec<&str> {
        self.fields
            .iter()
            .filter(|f| f.required)
            .map(|f| f.name.as_ref())
            .collect()
    }
}

/// One field of an object.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FieldNode {
    /// The key on the wire.
    pub name: Cow<'static, str>,
    /// `false` when the schema is optional or carries a default.
    pub required: bool,
    /// What the value must satisfy.
    pub schema: SchemaNode,
    /// One line for a human: a label hint or an OpenAPI `description`.
    pub description: Option<Cow<'static, str>>,
}

/// What an object schema does with a key it does not declare.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum UnknownKeys {
    /// Report an `unknown_key` issue. The default, and the opposite of zod's.
    ///
    /// zod strips silently, which hides a renamed field behind a form that
    /// appears to work. A framework whose thesis is compile-time rigour should
    /// not default to the quiet option at run time either.
    #[default]
    Reject,
    /// Drop it without a word.
    Ignore,
    /// Keep it, unvalidated.
    Passthrough,
}

impl UnknownKeys {
    /// `reject`, `ignore` or `passthrough`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            UnknownKeys::Reject => "reject",
            UnknownKeys::Ignore => "ignore",
            UnknownKeys::Passthrough => "passthrough",
        }
    }
}
