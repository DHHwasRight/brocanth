//! The schema IR, and the boundary this crate consumes schemas across.
//!
//! # The boundary
//!
//! `brocanth-schema` is a sibling crate, written in parallel, that describes
//! request and response bodies and publishes a walkable description of them.
//! Waiting for it would have meant writing nothing, so this crate does not
//! depend on it. It depends on [`SchemaSource`], a trait with two methods,
//! and on [`Schema`], an IR of exactly the JSON Schema 2020-12 subset
//! OpenAPI 3.1 needs.
//!
//! [`crate::adapter`] holds the one piece that knows about the other crate:
//! a `SchemaNode` shape and a `From` impl for it. When the real crate lands,
//! that module changes and nothing else does.
//!
//! # Why 3.1 rather than 3.0
//!
//! OpenAPI 3.0 had its own schema dialect, close to JSON Schema draft-04 but
//! not equal to it — `nullable: true` instead of a null type, `exclusiveMinimum`
//! as a boolean, no `$ref` beside anything. 3.1's schema object *is* a JSON
//! Schema 2020-12 schema, so this IR maps to it without inventing a
//! translation that is wrong in the corners.
//!
//! Two consequences show up in [`Schema::to_json`]:
//!
//! * A nullable value is `type: [T, "null"]`. There is no `nullable` keyword
//!   in 3.1 and emitting one is silently ignored.
//! * A nullable *reference* cannot be `{"$ref": ..., "type": "null"}` — it
//!   becomes `anyOf: [{"$ref": ...}, {"type": "null"}]`. 3.1 permits siblings
//!   beside `$ref` and a great many tools still ignore them, so this crate
//!   never relies on one: a reference that carries a description is wrapped in
//!   a single-element `allOf` instead.

use crate::json::{Json, Obj};

/// The JSON Schema type keywords this IR emits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SchemaType {
    /// `"boolean"`
    Boolean,
    /// `"integer"`
    Integer,
    /// `"number"`
    Number,
    /// `"string"`
    String,
}

impl SchemaType {
    /// The keyword, as it appears in the document.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::String => "string",
        }
    }
}

/// What an object says about properties it did not declare.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Additional {
    /// Say nothing, which means anything is allowed.
    #[default]
    Unspecified,
    /// `additionalProperties: false`.
    Denied,
    /// `additionalProperties: <schema>`.
    Schema(Box<Schema>),
}

/// The shape of a schema, before the annotations that decorate it.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum SchemaKind {
    /// No constraint at all: the empty schema, which accepts anything.
    #[default]
    Any,
    /// A scalar.
    Scalar(SchemaType),
    /// An array of one item type.
    Array(Box<Schema>),
    /// An object.
    Object {
        /// Declared properties, in declaration order.
        properties: Vec<(String, Schema)>,
        /// The names of the required properties.
        ///
        /// A list of names, never a flag per property: that is what JSON
        /// Schema says and what a generator reads.
        required: Vec<String>,
        /// What undeclared properties may be.
        additional: Additional,
    },
    /// A reference to `#/components/schemas/<name>`.
    Reference(String),
    /// `oneOf`.
    OneOf(Vec<Schema>),
    /// `anyOf`.
    AnyOf(Vec<Schema>),
    /// `allOf`.
    AllOf(Vec<Schema>),
}

/// A JSON Schema 2020-12 schema, in the subset OpenAPI 3.1 documents use.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Schema {
    kind: SchemaKind,
    nullable: bool,
    title: Option<String>,
    description: Option<String>,
    format: Option<String>,
    default: Option<Json>,
    example: Option<Json>,
    deprecated: bool,
    read_only: bool,
    write_only: bool,
    enumeration: Vec<Json>,
    minimum: Option<f64>,
    maximum: Option<f64>,
    exclusive_minimum: Option<f64>,
    exclusive_maximum: Option<f64>,
    multiple_of: Option<f64>,
    min_length: Option<u64>,
    max_length: Option<u64>,
    min_items: Option<u64>,
    max_items: Option<u64>,
    unique_items: bool,
    pattern: Option<String>,
}

impl Schema {
    fn of(kind: SchemaKind) -> Self {
        Self {
            kind,
            ..Self::default()
        }
    }

    /// The empty schema: anything at all.
    ///
    /// This is the honest default for a body nobody described, and it is why
    /// an undocumented handler still produces a valid operation.
    #[must_use]
    pub fn any() -> Self {
        Self::of(SchemaKind::Any)
    }

    /// `type: boolean`.
    #[must_use]
    pub fn boolean() -> Self {
        Self::of(SchemaKind::Scalar(SchemaType::Boolean))
    }

    /// `type: integer`.
    #[must_use]
    pub fn integer() -> Self {
        Self::of(SchemaKind::Scalar(SchemaType::Integer))
    }

    /// `type: number`.
    #[must_use]
    pub fn number() -> Self {
        Self::of(SchemaKind::Scalar(SchemaType::Number))
    }

    /// `type: string`.
    #[must_use]
    pub fn string() -> Self {
        Self::of(SchemaKind::Scalar(SchemaType::String))
    }

    /// `type: array`, with an item schema.
    #[must_use]
    pub fn array(items: Schema) -> Self {
        Self::of(SchemaKind::Array(Box::new(items)))
    }

    /// An object with no properties yet.
    #[must_use]
    pub fn object() -> Self {
        Self::of(SchemaKind::Object {
            properties: Vec::new(),
            required: Vec::new(),
            additional: Additional::Unspecified,
        })
    }

    /// A reference to a named component schema.
    ///
    /// The name must be registered with
    /// [`Components::schema`](crate::document::Components::schema), or
    /// validation reports a dangling reference.
    #[must_use]
    pub fn reference(name: impl Into<String>) -> Self {
        Self::of(SchemaKind::Reference(name.into()))
    }

    /// `oneOf`.
    #[must_use]
    pub fn one_of(variants: Vec<Schema>) -> Self {
        Self::of(SchemaKind::OneOf(variants))
    }

    /// `anyOf`.
    #[must_use]
    pub fn any_of(variants: Vec<Schema>) -> Self {
        Self::of(SchemaKind::AnyOf(variants))
    }

    /// `allOf`.
    #[must_use]
    pub fn all_of(parts: Vec<Schema>) -> Self {
        Self::of(SchemaKind::AllOf(parts))
    }

    /// An optional property.
    ///
    /// # Panics
    ///
    /// If this schema is not an object.
    #[must_use]
    pub fn property(mut self, name: impl Into<String>, schema: Schema) -> Self {
        let SchemaKind::Object { properties, .. } = &mut self.kind else {
            panic!("property() is only valid on an object schema");
        };
        properties.push((name.into(), schema));
        self
    }

    /// A required property: declared, and named in `required`.
    ///
    /// # Panics
    ///
    /// If this schema is not an object.
    #[must_use]
    pub fn required(mut self, name: impl Into<String>, schema: Schema) -> Self {
        let name = name.into();
        let SchemaKind::Object {
            properties,
            required,
            ..
        } = &mut self.kind
        else {
            panic!("required() is only valid on an object schema");
        };
        properties.push((name.clone(), schema));
        required.push(name);
        self
    }

    /// What undeclared properties may be.
    ///
    /// # Panics
    ///
    /// If this schema is not an object.
    #[must_use]
    pub fn additional_properties(mut self, value: Additional) -> Self {
        let SchemaKind::Object { additional, .. } = &mut self.kind else {
            panic!("additional_properties() is only valid on an object schema");
        };
        *additional = value;
        self
    }

    /// Makes the value nullable.
    ///
    /// Emitted as `type: [T, "null"]`, which is what 3.1 reads. Never as
    /// `nullable: true`, which 3.1 ignores.
    #[must_use]
    pub fn nullable(mut self) -> Self {
        self.nullable = true;
        self
    }

    /// A `title`.
    #[must_use]
    pub fn title(mut self, text: impl Into<String>) -> Self {
        self.title = Some(text.into());
        self
    }

    /// A `description`.
    #[must_use]
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// A `format`, such as `date-time` or `uuid`.
    #[must_use]
    pub fn format(mut self, text: impl Into<String>) -> Self {
        self.format = Some(text.into());
        self
    }

    /// A `default`.
    #[must_use]
    pub fn default_value(mut self, value: impl Into<Json>) -> Self {
        self.default = Some(value.into());
        self
    }

    /// An `example`.
    #[must_use]
    pub fn example(mut self, value: impl Into<Json>) -> Self {
        self.example = Some(value.into());
        self
    }

    /// Marks the value deprecated.
    #[must_use]
    pub fn deprecated(mut self) -> Self {
        self.deprecated = true;
        self
    }

    /// `readOnly: true`.
    #[must_use]
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// `writeOnly: true`.
    #[must_use]
    pub fn write_only(mut self) -> Self {
        self.write_only = true;
        self
    }

    /// An `enum` of allowed values.
    #[must_use]
    pub fn enumeration(mut self, values: Vec<Json>) -> Self {
        self.enumeration = values;
        self
    }

    /// An `enum` of allowed strings.
    #[must_use]
    pub fn string_enum<I, S>(self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let values = values.into_iter().map(|v| Json::Str(v.into())).collect();
        self.enumeration(values)
    }

    /// `minimum`.
    #[must_use]
    pub fn minimum(mut self, value: f64) -> Self {
        self.minimum = Some(value);
        self
    }

    /// `maximum`.
    #[must_use]
    pub fn maximum(mut self, value: f64) -> Self {
        self.maximum = Some(value);
        self
    }

    /// `exclusiveMinimum`.
    ///
    /// A number in JSON Schema 2020-12 and in OpenAPI 3.1. It was a boolean
    /// beside `minimum` in 3.0, which is one of the incompatibilities that
    /// makes a 3.0 emitter a translation rather than a mapping.
    #[must_use]
    pub fn exclusive_minimum(mut self, value: f64) -> Self {
        self.exclusive_minimum = Some(value);
        self
    }

    /// `exclusiveMaximum`.
    #[must_use]
    pub fn exclusive_maximum(mut self, value: f64) -> Self {
        self.exclusive_maximum = Some(value);
        self
    }

    /// `multipleOf`.
    #[must_use]
    pub fn multiple_of(mut self, value: f64) -> Self {
        self.multiple_of = Some(value);
        self
    }

    /// `uniqueItems: true`.
    #[must_use]
    pub fn unique_items(mut self) -> Self {
        self.unique_items = true;
        self
    }

    /// `minLength`.
    #[must_use]
    pub fn min_length(mut self, value: u64) -> Self {
        self.min_length = Some(value);
        self
    }

    /// `maxLength`.
    #[must_use]
    pub fn max_length(mut self, value: u64) -> Self {
        self.max_length = Some(value);
        self
    }

    /// `minItems`.
    #[must_use]
    pub fn min_items(mut self, value: u64) -> Self {
        self.min_items = Some(value);
        self
    }

    /// `maxItems`.
    #[must_use]
    pub fn max_items(mut self, value: u64) -> Self {
        self.max_items = Some(value);
        self
    }

    /// `pattern`.
    #[must_use]
    pub fn pattern(mut self, value: impl Into<String>) -> Self {
        self.pattern = Some(value.into());
        self
    }

    /// The shape, without the annotations.
    #[must_use]
    pub const fn kind(&self) -> &SchemaKind {
        &self.kind
    }

    /// Whether this is a bare reference to a component.
    #[must_use]
    pub fn as_reference(&self) -> Option<&str> {
        match &self.kind {
            SchemaKind::Reference(name) => Some(name),
            _ => None,
        }
    }

    /// Every component name this schema refers to, at any depth.
    pub fn collect_references(&self, out: &mut Vec<String>) {
        match &self.kind {
            SchemaKind::Reference(name) => out.push(name.clone()),
            SchemaKind::Array(items) => items.collect_references(out),
            SchemaKind::Object {
                properties,
                additional,
                ..
            } => {
                for (_, schema) in properties {
                    schema.collect_references(out);
                }
                if let Additional::Schema(schema) = additional {
                    schema.collect_references(out);
                }
            }
            SchemaKind::OneOf(parts) | SchemaKind::AnyOf(parts) | SchemaKind::AllOf(parts) => {
                for schema in parts {
                    schema.collect_references(out);
                }
            }
            SchemaKind::Any | SchemaKind::Scalar(_) => {}
        }
    }

    /// Structural problems that make a schema a quiet failure.
    pub(crate) fn problems(&self, at: &str, out: &mut Vec<String>) {
        match &self.kind {
            SchemaKind::Object {
                properties,
                required,
                additional,
            } => {
                for name in required {
                    if !properties.iter().any(|(declared, _)| declared == name) {
                        out.push(format!(
                            "{at}: `required` names `{name}`, which is not a declared property"
                        ));
                    }
                }
                let mut seen: Vec<&str> = Vec::new();
                for (name, schema) in properties {
                    if seen.contains(&name.as_str()) {
                        out.push(format!("{at}: the property `{name}` is declared twice"));
                    }
                    seen.push(name);
                    schema.problems(&format!("{at}.{name}"), out);
                }
                if let Additional::Schema(schema) = additional {
                    schema.problems(&format!("{at}.additionalProperties"), out);
                }
            }
            SchemaKind::Array(items) => items.problems(&format!("{at}[]"), out),
            SchemaKind::OneOf(parts) | SchemaKind::AnyOf(parts) | SchemaKind::AllOf(parts) => {
                if parts.is_empty() {
                    out.push(format!("{at}: a composition with no members"));
                }
                for (index, schema) in parts.iter().enumerate() {
                    schema.problems(&format!("{at}[{index}]"), out);
                }
            }
            SchemaKind::Any | SchemaKind::Scalar(_) | SchemaKind::Reference(_) => {}
        }
    }

    fn has_annotations(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.format.is_some()
            || self.default.is_some()
            || self.example.is_some()
            || self.deprecated
            || self.read_only
            || self.write_only
            || !self.enumeration.is_empty()
            || self.minimum.is_some()
            || self.maximum.is_some()
            || self.exclusive_minimum.is_some()
            || self.exclusive_maximum.is_some()
            || self.multiple_of.is_some()
            || self.unique_items
            || self.min_length.is_some()
            || self.max_length.is_some()
            || self.min_items.is_some()
            || self.max_items.is_some()
            || self.pattern.is_some()
    }

    fn write_annotations(&self, obj: &mut Obj) {
        obj.maybe("title", self.title.as_ref());
        obj.maybe("description", self.description.as_ref());
        obj.maybe("format", self.format.as_ref());
        obj.maybe("default", self.default.clone());
        obj.list("enum", self.enumeration.clone());
        obj.maybe("minimum", self.minimum);
        obj.maybe("maximum", self.maximum);
        obj.maybe("exclusiveMinimum", self.exclusive_minimum);
        obj.maybe("exclusiveMaximum", self.exclusive_maximum);
        obj.maybe("multipleOf", self.multiple_of);
        obj.maybe("minLength", self.min_length);
        obj.maybe("maxLength", self.max_length);
        obj.maybe("minItems", self.min_items);
        obj.maybe("maxItems", self.max_items);
        obj.flag("uniqueItems", self.unique_items);
        obj.maybe("pattern", self.pattern.as_ref());
        obj.flag("readOnly", self.read_only);
        obj.flag("writeOnly", self.write_only);
        obj.flag("deprecated", self.deprecated);
        obj.maybe("example", self.example.clone());
    }

    /// The schema as it appears in the document.
    #[must_use]
    pub fn to_json(&self) -> Json {
        // A reference with anything else on it is wrapped rather than
        // decorated. `$ref` siblings are legal in 3.1 and ignored by enough
        // tooling that relying on one is a silent loss.
        if let SchemaKind::Reference(name) = &self.kind {
            let reference = reference_json(name);
            return match (self.nullable, self.has_annotations()) {
                (false, false) => reference,
                (false, true) => {
                    let mut obj = Json::object();
                    obj.set("allOf", vec![reference]);
                    self.write_annotations(&mut obj);
                    obj.build()
                }
                (true, _) => {
                    let mut obj = Json::object();
                    obj.set("anyOf", vec![reference, null_type()]);
                    self.write_annotations(&mut obj);
                    obj.build()
                }
            };
        }

        let mut obj = Json::object();
        match &self.kind {
            SchemaKind::Any => {
                if self.nullable {
                    // `{}` already admits null; saying so again would be
                    // noise, not information.
                }
            }
            SchemaKind::Scalar(scalar) => {
                obj.set("type", self.type_json(scalar.as_str()));
            }
            SchemaKind::Array(items) => {
                obj.set("type", self.type_json("array"));
                obj.set("items", items.to_json());
            }
            SchemaKind::Object {
                properties,
                required,
                additional,
            } => {
                obj.set("type", self.type_json("object"));
                if !properties.is_empty() {
                    let mut props = Json::object();
                    for (name, schema) in properties {
                        props.set(name, schema.to_json());
                    }
                    obj.set("properties", props.build());
                }
                if !required.is_empty() {
                    let names: Vec<Json> = required.iter().map(|n| Json::Str(n.clone())).collect();
                    obj.set("required", names);
                }
                match additional {
                    Additional::Unspecified => {}
                    Additional::Denied => {
                        obj.set("additionalProperties", false);
                    }
                    Additional::Schema(schema) => {
                        obj.set("additionalProperties", schema.to_json());
                    }
                }
            }
            SchemaKind::OneOf(parts) => self.write_composition(&mut obj, "oneOf", parts),
            SchemaKind::AnyOf(parts) => self.write_composition(&mut obj, "anyOf", parts),
            SchemaKind::AllOf(parts) => self.write_composition(&mut obj, "allOf", parts),
            SchemaKind::Reference(_) => unreachable!("handled above"),
        }
        self.write_annotations(&mut obj);
        obj.build()
    }

    fn write_composition(&self, obj: &mut Obj, keyword: &str, parts: &[Schema]) {
        let mut members: Vec<Json> = parts.iter().map(Schema::to_json).collect();
        if self.nullable {
            if keyword == "anyOf" || keyword == "oneOf" {
                members.push(null_type());
                obj.set(keyword, members);
            } else {
                // A nullable `allOf` is not an `allOf` with a null branch —
                // that would be unsatisfiable — so it becomes a choice.
                let inner = Json::Object(vec![(keyword.to_owned(), Json::Array(members))]);
                obj.set("anyOf", vec![inner, null_type()]);
            }
        } else {
            obj.set(keyword, members);
        }
    }

    fn type_json(&self, name: &str) -> Json {
        if self.nullable {
            Json::Array(vec![Json::Str(name.to_owned()), Json::Str("null".to_owned())])
        } else {
            Json::Str(name.to_owned())
        }
    }
}

fn reference_json(name: &str) -> Json {
    Json::Object(vec![(
        "$ref".to_owned(),
        Json::Str(format!("#/components/schemas/{name}")),
    )])
}

fn null_type() -> Json {
    Json::Object(vec![("type".to_owned(), Json::Str("null".to_owned()))])
}

/// A schema with the name it is registered under in `components/schemas`.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedSchema {
    /// The component name.
    pub name: String,
    /// The schema itself.
    pub schema: Schema,
}

impl NamedSchema {
    /// A named schema.
    pub fn new(name: impl Into<String>, schema: Schema) -> Self {
        Self {
            name: name.into(),
            schema,
        }
    }
}

/// A type that can describe itself as a schema.
///
/// This is the whole of what this crate asks of `brocanth-schema`, or of any
/// other description layer. Two required pieces of information and one
/// optional one:
///
/// * [`SchemaSource::schema`] — the shape.
/// * [`SchemaSource::schema_name`] — a component name, or [`None`] for a type
///   that should be inlined. A named type is registered once under
///   `components/schemas` and referenced everywhere it is used, which is the
///   difference between a document a generator turns into one model and a
///   document it turns into three.
/// * [`SchemaSource::dependencies`] — other named schemas this one refers to,
///   registered alongside it so a `$ref` never dangles.
///
/// ```
/// use brocanth_openapi::{Schema, SchemaSource};
///
/// struct Post;
///
/// impl SchemaSource for Post {
///     fn schema_name() -> Option<&'static str> {
///         Some("Post")
///     }
///
///     fn schema() -> Schema {
///         Schema::object()
///             .required("id", Schema::integer())
///             .required("title", Schema::string())
///             .property("body", Schema::string().nullable())
///     }
/// }
/// ```
pub trait SchemaSource {
    /// The name to register under, or [`None`] to inline the schema.
    fn schema_name() -> Option<&'static str> {
        None
    }

    /// The shape.
    fn schema() -> Schema;

    /// Named schemas this one refers to.
    fn dependencies() -> Vec<NamedSchema> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_is_a_list_of_names_not_a_flag() {
        let schema = Schema::object()
            .required("id", Schema::integer())
            .property("nickname", Schema::string());
        let json = schema.to_json();
        assert_eq!(
            json.get("required"),
            Some(&Json::Array(vec![Json::Str("id".into())]))
        );
        let properties = json.get("properties").unwrap();
        assert!(properties.get("id").unwrap().get("required").is_none());
    }

    #[test]
    fn an_empty_required_list_is_omitted() {
        let json = Schema::object().property("a", Schema::string()).to_json();
        assert!(json.get("required").is_none());
    }

    #[test]
    fn nullable_is_a_type_array_in_three_one() {
        let json = Schema::string().nullable().to_json();
        assert_eq!(
            json.get("type"),
            Some(&Json::Array(vec![
                Json::Str("string".into()),
                Json::Str("null".into())
            ]))
        );
        assert!(json.get("nullable").is_none());
    }

    #[test]
    fn a_nullable_reference_becomes_any_of() {
        let json = Schema::reference("Post").nullable().to_json();
        assert!(json.get("$ref").is_none());
        let branches = json.get("anyOf").unwrap().as_array().unwrap();
        assert_eq!(branches.len(), 2);
        assert_eq!(
            branches[0].get("$ref").and_then(Json::as_str),
            Some("#/components/schemas/Post")
        );
    }

    #[test]
    fn a_described_reference_does_not_rely_on_ref_siblings() {
        let json = Schema::reference("Post").description("the post").to_json();
        assert!(json.get("$ref").is_none());
        assert_eq!(json.get("allOf").unwrap().as_array().unwrap().len(), 1);
        assert_eq!(json.get("description").and_then(Json::as_str), Some("the post"));
    }

    #[test]
    fn a_bare_reference_is_just_a_ref() {
        let json = Schema::reference("Post").to_json();
        assert_eq!(json.as_object().unwrap().len(), 1);
        assert_eq!(
            json.get("$ref").and_then(Json::as_str),
            Some("#/components/schemas/Post")
        );
    }

    #[test]
    fn required_naming_an_undeclared_property_is_a_problem() {
        let mut schema = Schema::object().required("id", Schema::integer());
        if let SchemaKind::Object { required, .. } = &mut schema.kind {
            required.push("ghost".into());
        }
        let mut problems = Vec::new();
        schema.problems("Post", &mut problems);
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("ghost"));
    }

    #[test]
    fn references_are_collected_from_any_depth() {
        let schema = Schema::object()
            .property("posts", Schema::array(Schema::reference("Post")))
            .property("author", Schema::reference("User").nullable());
        let mut found = Vec::new();
        schema.collect_references(&mut found);
        found.sort();
        assert_eq!(found, vec!["Post".to_string(), "User".to_string()]);
    }
}
