//! Reading a `zod` schema into the document.
//!
//! [`crate::adapter`] does the same job for `brocanth-schema` and explains
//! why it holds a mirror of that crate's node type rather than depending on
//! it. This module is the other source, and it does depend on its crate,
//! behind the `zod` feature.
//!
//! # The limitation that is gone
//!
//! `adapter`'s header records an assumption worth repeating, because this
//! module is the one that breaks it:
//!
//! > **A node has no name.** Nothing in `SchemaNode` says "this is a
//! > `Post`", so this crate cannot decide on its own what belongs in
//! > `components/schemas`. [...] the alternative — inventing names from Rust
//! > type paths — produces `crate__models__post__Post` in a public API
//! > document.
//!
//! A `zod::Node` does have a name, because `#[derive(Entry)]` is the one
//! place that knows what the struct is called and puts it in the tree. So
//! this module registers components by itself: [`body_for`] gives back the
//! schema and every named part it refers to, each one once, and a nested
//! struct is a `$ref` rather than a copy. That is the difference between a
//! document a generator turns into one model and one it turns into three.
//!
//! ```
//! use brocanth_openapi::zod_node::body_for;
//! use zod::{Entry, Text};
//!
//! #[derive(Entry)]
//! struct Address {
//!     #[text(filled)]
//!     street: String,
//! }
//!
//! #[derive(Entry)]
//! struct Order {
//!     #[text(uuid)]
//!     reference: String,
//!     #[nested]
//!     ship_to: Address,
//! }
//!
//! let body = body_for::<Order>();
//! assert_eq!(body.name.as_deref(), Some("Order"));
//!
//! let names: Vec<&str> = body.parts.iter().map(|p| p.name.as_str()).collect();
//! assert_eq!(names, vec!["Order", "Address"]);
//!
//! // The use site is a reference, not a second copy of Address.
//! let order = &body.parts[0].schema;
//! let ship_to = order.property_named("ship_to").unwrap();
//! assert_eq!(ship_to.as_reference(), Some("Address"));
//! ```

use zod::node::{Field, Node, Object};

use crate::schema::{Additional, NamedSchema, Schema};

/// A schema, and every named part a document has to register beside it.
#[derive(Clone, Debug)]
pub struct Body {
    /// The name to refer to it by, for a node that has one.
    pub name: Option<String>,
    /// The schema at the use site: a `$ref` for a named node, the shape
    /// itself for one with no name.
    pub schema: Schema,
    /// Every named struct in the tree, each one once, outermost first.
    /// Register all of them or a `$ref` dangles.
    pub parts: Vec<NamedSchema>,
}

/// Everything a document needs about one `zod` node.
pub fn body_of(node: &Node) -> Body {
    let parts = node
        .named_parts()
        .into_iter()
        .map(|(name, inner)| NamedSchema::new(name, shape(inner)))
        .collect();

    Body {
        name: node.name().map(str::to_string),
        schema: convert(node),
        parts,
    }
}

/// The same, for a struct that derives [`zod::Entry`].
pub fn body_for<T: zod::Entry>() -> Body {
    body_of(&T::describe())
}

/// One node at a use site: a reference when it has a name, the shape
/// otherwise.
fn convert(node: &Node) -> Schema {
    match node {
        // A named struct is registered once and pointed at everywhere.
        Node::Named { name, .. } => Schema::reference(*name),
        // zod's `optional` accepts an explicit null as well as an absent
        // field, so both halves are said: the type gains null here, and the
        // object leaves the name out of `required`.
        Node::Optional(inner) => convert(inner).nullable(),
        Node::Defaulted(inner) => convert(inner),
        Node::Said { inner, says } => noted(convert(inner), says),
        Node::Rule { inner, says } => noted(convert(inner), says),
        Node::Labelled { inner, label } => titled(convert(inner), label),
        leaf => shape(leaf),
    }
}

/// The shape itself, with no name in front of it.
fn shape(node: &Node) -> Schema {
    match node {
        Node::Text(facts) => text(facts),
        Node::Int(facts) => whole(facts),
        Node::Real(facts) => real(facts),
        Node::Bool => Schema::boolean(),
        Node::List { item, min, max } => {
            let mut schema = Schema::array(convert(item));
            if *min > 0 {
                schema = schema.min_items(*min as u64);
            }
            if *max != usize::MAX {
                schema = schema.max_items(*max as u64);
            }
            schema
        }
        Node::Object(object) => fields(object),
        // A wrapper reached here rather than through `convert` still has to
        // produce the thing underneath it.
        Node::Named { inner, .. }
        | Node::Optional(inner)
        | Node::Defaulted(inner)
        | Node::Said { inner, .. }
        | Node::Rule { inner, .. }
        | Node::Labelled { inner, .. } => shape(inner),
    }
}

fn fields(object: &Object) -> Schema {
    let mut schema = Schema::object();
    for field in &object.fields {
        let property = described(field);
        schema = if field.required {
            schema.required(field.name, property)
        } else {
            schema.property(field.name, property)
        };
    }
    if object.strict {
        schema = schema.additional_properties(Additional::Denied);
    }
    schema
}

/// One field, with the label as its title and the message as its
/// description.
///
/// A `says` is what the schema tells a person when the field is wrong, and
/// a description is what a document tells a person before they send it.
/// They are the same sentence, which is the point of writing it once.
fn described(field: &Field) -> Schema {
    let mut schema = convert(&field.node);
    if let Some(label) = field.label {
        schema = titled(schema, label);
    }
    if let Some(says) = field.says {
        schema = noted(schema, says);
    }
    schema
}

/// Text, with the rules JSON Schema can carry.
fn text(facts: &zod::TextFacts) -> Schema {
    let mut schema = Schema::string();
    if facts.min > 0 {
        schema = schema.min_length(facts.min as u64);
    }
    if facts.max != usize::MAX {
        schema = schema.max_length(facts.max as u64);
    }
    if let Some(format) = format_of(facts.shape) {
        schema = schema.format(format);
    }

    let mut patterns: Vec<String> = Vec::new();
    if let Some(pattern) = pattern_of(facts.shape) {
        patterns.push(pattern.to_string());
    }
    if let Some(prefix) = facts.starts {
        patterns.push(format!("^{}", escaped(prefix)));
    }
    if let Some(suffix) = facts.ends {
        patterns.push(format!("{}$", escaped(suffix)));
    }
    if let Some(part) = facts.holds {
        patterns.push(escaped(part));
    }

    match patterns.len() {
        0 => schema,
        1 => schema.pattern(patterns.remove(0)),
        // Two rules are two patterns, and one regular expression that means
        // both needs a lookahead. `allOf` says it without one, and every
        // validator reads it.
        _ => {
            let mut parts = vec![schema];
            parts.extend(patterns.into_iter().map(|p| Schema::string().pattern(p)));
            Schema::all_of(parts)
        }
    }
}

/// A whole number. The bounds are the ones the read enforces, which for a
/// narrow type is the type's own range.
fn whole(facts: &zod::IntFacts) -> Schema {
    let fits_32 = facts.min >= i128::from(i32::MIN) && facts.max <= i128::from(i32::MAX);
    let (low, high) = if fits_32 {
        (i128::from(i32::MIN), i128::from(i32::MAX))
    } else {
        (i128::from(i64::MIN), i128::from(i64::MAX))
    };

    let mut schema = Schema::integer().format(if fits_32 { "int32" } else { "int64" });
    // A bound the format already implies is left out, for two reasons. It
    // is noise -- `int32` says `-2147483648` -- and `minimum` is a double
    // in JSON Schema, so `i64::MIN` written as one comes back a different
    // number. Saying nothing is right; saying it wrongly is not.
    if facts.min != i128::MIN && facts.min != low {
        schema = schema.minimum(facts.min as f64);
    }
    if facts.max != i128::MAX && facts.max != high {
        schema = schema.maximum(facts.max as f64);
    }
    if facts.step != 0 {
        schema = schema.multiple_of(facts.step as f64);
    }
    schema
}

fn real(facts: &zod::RealFacts) -> Schema {
    let mut schema = Schema::number().format("double");
    if facts.min.is_finite() {
        schema = schema.minimum(facts.min);
    }
    if facts.max.is_finite() {
        schema = schema.maximum(facts.max);
    }
    schema
}

/// A shape JSON Schema has a name for.
fn format_of(shape: zod::TextShape) -> Option<&'static str> {
    match shape {
        zod::TextShape::Email => Some("email"),
        zod::TextShape::Uuid => Some("uuid"),
        zod::TextShape::DateIso => Some("date"),
        _ => None,
    }
}

/// A shape JSON Schema has no name for, as the regular expression it is.
///
/// Written out rather than derived: these are what the const checker
/// enforces, and a pattern that drifts from it would promise a client
/// something the server refuses.
fn pattern_of(shape: zod::TextShape) -> Option<&'static str> {
    match shape {
        zod::TextShape::Free | zod::TextShape::Email | zod::TextShape::Uuid => None,
        zod::TextShape::DateIso => None,
        zod::TextShape::Ascii => Some("^[\\x00-\\x7F]*$"),
        zod::TextShape::Alnum => Some("^[A-Za-z0-9]*$"),
        zod::TextShape::Digits => Some("^[0-9]*$"),
        zod::TextShape::Hex => Some("^[0-9A-Fa-f]*$"),
        zod::TextShape::Lower => Some("^[^A-Z]*$"),
        zod::TextShape::Upper => Some("^[^a-z]*$"),
        zod::TextShape::Slug => Some("^[a-z0-9]+(-[a-z0-9]+)*$"),
    }
}

/// A literal, as a regular expression that matches it and nothing else.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\^$.|?*+()[]{}/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A `$ref` carries no title and no description in a document a generator
/// will read, so the wrapper is dropped rather than written somewhere it is
/// ignored.
fn noted(schema: Schema, text: &str) -> Schema {
    if schema.as_reference().is_some() {
        schema
    } else {
        schema.description(text)
    }
}

fn titled(schema: Schema, text: &str) -> Schema {
    if schema.as_reference().is_some() {
        schema
    } else {
        schema.title(text)
    }
}

// -- the builder's side ----------------------------------------------------

impl crate::spec::SpecBuilder {
    /// Register every named part of a `zod` struct under
    /// `components/schemas`.
    ///
    /// One call per struct the document refers to. A nested struct comes
    /// along with the one that holds it, each registered once, so a `$ref`
    /// never dangles and a generator makes one model per struct.
    ///
    /// ```
    /// use brocanth_openapi::{Schema, SpecBuilder};
    /// use zod::{Entry, Int, Text};
    ///
    /// #[derive(Entry)]
    /// struct Signup {
    ///     #[text(min = 3, max = 16, slug)]
    ///     handle: String,
    ///     #[int(min = 13, max = 130)]
    ///     age: i64,
    /// }
    ///
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// let document = SpecBuilder::new("Accounts", "1.0.0")
    ///     .describing::<Signup>()
    ///     .build()?;
    ///
    /// let json = document.to_json().to_json();
    /// assert!(json.contains("\"Signup\""));
    /// assert!(json.contains("\"minLength\": 3"));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn describing<T: zod::Entry>(self) -> Self {
        self.describing_node(&T::describe())
    }

    /// The same, for a node the caller already has.
    #[must_use]
    pub fn describing_node(mut self, node: &Node) -> Self {
        for part in body_of(node).parts {
            self = self.component(part.name, part.schema);
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json::Json;
    use crate::schema::SchemaKind;
    use zod::{Entry, Int, Schema as _, Text};

    #[derive(Entry)]
    struct Signup {
        #[text(min = 3, max = 16, slug)]
        #[label("Handle")]
        #[says("Pick 3 to 16 lowercase letters, digits or hyphens")]
        handle: String,

        #[text(filled, email)]
        email: String,

        #[int(min = 13, max = 130)]
        age: i64,

        #[int(min = 1, max = 99)]
        quantity: u8,

        #[real(min = 0.0, max = 1.0)]
        ratio: f64,

        #[text(min = 2)]
        #[list(min = 1, max = 3)]
        tags: Vec<String>,

        #[text(min = 4)]
        note: Option<String>,

        subscribed: bool,
    }

    fn signup() -> Schema {
        body_for::<Signup>().parts.remove(0).schema
    }

    #[test]
    fn a_struct_becomes_a_component_under_its_own_name() {
        let body = body_for::<Signup>();
        assert_eq!(body.name.as_deref(), Some("Signup"));
        assert_eq!(body.schema.as_reference(), Some("Signup"));
        assert_eq!(body.parts.len(), 1);
        assert_eq!(body.parts[0].name, "Signup");
    }

    #[test]
    fn a_required_field_is_a_name_in_the_required_list() {
        let schema = signup();
        assert!(schema.requires("handle"));
        assert!(!schema.requires("note"), "an Option is not required");
    }

    #[test]
    fn an_optional_field_also_admits_null_because_the_schema_does() {
        // zod's `optional` accepts an explicit null as well as an absent
        // field, so a document that only left it out of `required` would
        // describe a stricter server than the one that is running.
        let json = signup().property_named("note").unwrap().to_json().to_json();
        assert!(json.contains("null"), "{json}");
    }

    #[test]
    fn a_length_rule_becomes_min_length_and_max_length() {
        let handle = signup().property_named("handle").unwrap().to_json();
        assert_eq!(
            handle.get("minLength").map(Json::to_json).as_deref(),
            Some("3")
        );
        assert_eq!(
            handle.get("maxLength").map(Json::to_json).as_deref(),
            Some("16")
        );
    }

    #[test]
    fn a_shape_becomes_a_format_where_json_schema_has_one() {
        let email = signup().property_named("email").unwrap().to_json();
        assert_eq!(
            email.get("format").map(Json::to_json).as_deref(),
            Some("\"email\"")
        );
    }

    #[test]
    fn a_shape_with_no_format_becomes_the_pattern_it_is() {
        let handle = signup().property_named("handle").unwrap().to_json();
        assert_eq!(
            handle.get("pattern").map(Json::to_json).as_deref(),
            Some("\"^[a-z0-9]+(-[a-z0-9]+)*$\"")
        );
    }

    #[test]
    fn two_patterns_are_two_schemas_and_not_one_wrong_regex() {
        #[derive(Entry)]
        struct Linked {
            #[text(slug, starts = "x-")]
            key: String,
        }
        let schema = body_for::<Linked>().parts.remove(0).schema;
        let key = schema.property_named("key").unwrap();
        let SchemaKind::AllOf(parts) = key.kind() else {
            panic!("two rules, two patterns: {key:?}");
        };
        // The string itself, then one schema per pattern.
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn a_literal_in_a_pattern_is_escaped_so_it_matches_itself() {
        #[derive(Entry)]
        struct Dotted {
            #[text(ends = ".json")]
            name: String,
        }
        let schema = body_for::<Dotted>().parts.remove(0).schema;
        let json = schema.property_named("name").unwrap().to_json().to_json();
        assert!(json.contains("\\\\.json$"), "the dot is escaped: {json}");
    }

    #[test]
    fn a_narrow_integer_publishes_the_range_it_accepts() {
        let quantity = signup().property_named("quantity").unwrap().to_json();
        assert_eq!(
            quantity.get("minimum").map(Json::to_json).as_deref(),
            Some("1")
        );
        assert_eq!(
            quantity.get("maximum").map(Json::to_json).as_deref(),
            Some("99")
        );
    }

    #[test]
    fn a_whole_number_says_which_width_it_fits() {
        let age = signup().property_named("age").unwrap().to_json();
        assert_eq!(
            age.get("format").map(Json::to_json).as_deref(),
            Some("\"int32\"")
        );

        #[derive(Entry)]
        struct Big {
            count: i64,
        }
        let schema = body_for::<Big>().parts.remove(0).schema;
        let count = schema.property_named("count").unwrap().to_json();
        assert_eq!(
            count.get("format").map(Json::to_json).as_deref(),
            Some("\"int64\"")
        );
    }

    #[test]
    fn a_list_carries_its_counts_and_its_item() {
        let schema = signup();
        let tags = schema.property_named("tags").unwrap();
        let json = tags.to_json();
        assert_eq!(
            json.get("minItems").map(Json::to_json).as_deref(),
            Some("1")
        );
        assert_eq!(
            json.get("maxItems").map(Json::to_json).as_deref(),
            Some("3")
        );

        let SchemaKind::Array(item) = tags.kind() else {
            panic!("tags is an array");
        };
        assert_eq!(
            item.to_json()
                .get("minLength")
                .map(Json::to_json)
                .as_deref(),
            Some("2")
        );
    }

    #[test]
    fn a_label_is_the_title_and_a_message_is_the_description() {
        let handle = signup().property_named("handle").unwrap().to_json();
        assert_eq!(
            handle.get("title").map(Json::to_json).as_deref(),
            Some("\"Handle\"")
        );
        assert_eq!(
            handle.get("description").map(Json::to_json).as_deref(),
            Some("\"Pick 3 to 16 lowercase letters, digits or hyphens\"")
        );
    }

    #[test]
    fn a_boolean_says_nothing_but_its_type() {
        let schema = signup();
        let json = schema
            .property_named("subscribed")
            .unwrap()
            .to_json()
            .to_json();
        assert_eq!(
            json.matches(':').count(),
            1,
            "one key, and it is the type: {json}"
        );
        assert!(json.contains("\"boolean\""));
    }

    #[test]
    fn a_nested_struct_is_a_reference_and_a_component_of_its_own() {
        #[derive(Entry)]
        struct Address {
            #[text(filled)]
            street: String,
        }

        #[derive(Entry)]
        struct Order {
            #[nested]
            ship_to: Address,
            #[check(zod::list(zod::nested::<Address>()))]
            also: Vec<Address>,
        }

        let body = body_for::<Order>();
        let names: Vec<&str> = body.parts.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Order", "Address"],
            "each one once, outermost first"
        );

        let order = &body.parts[0].schema;
        assert_eq!(
            order.property_named("ship_to").unwrap().as_reference(),
            Some("Address")
        );

        let SchemaKind::Array(item) = order.property_named("also").unwrap().kind() else {
            panic!("also is an array");
        };
        assert_eq!(
            item.as_reference(),
            Some("Address"),
            "and not a second copy"
        );
    }

    #[test]
    fn a_strict_struct_refuses_undeclared_properties_in_the_document_too() {
        #[derive(Entry)]
        #[entry(strict)]
        struct Tight {
            #[text(filled)]
            name: String,
        }
        let schema = body_for::<Tight>().parts.remove(0).schema;
        let json = schema.to_json().to_json();
        assert!(json.contains("\"additionalProperties\": false"), "{json}");
    }

    #[test]
    fn a_rule_the_caller_wrote_contributes_the_sentence_and_nothing_else() {
        // A predicate no description can express is not describable in JSON
        // Schema either, so what a client gets is the sentence.
        let even = Int::new().refine("must be even", |n| n % 2 == 0);
        let json = convert(&even.node()).to_json().to_json();
        assert!(json.contains("must be even"), "{json}");
        assert!(json.contains("\"integer\""), "{json}");
        assert!(
            !json.contains("minimum"),
            "an unbounded number says no bound: {json}"
        );
    }

    #[test]
    fn a_schema_built_by_hand_converts_the_same_way() {
        let schema = zod::object((
            ("name", Text::new().min(1)),
            ("tags", zod::list(Text::new()).max(3)),
        ));
        let converted = convert(&schema.node());
        assert!(converted.requires("name"));
        assert!(converted.property_named("tags").is_some());
    }

    #[test]
    fn the_builder_registers_every_part_in_one_call() {
        #[derive(Entry)]
        struct Address {
            #[text(filled)]
            street: String,
        }

        #[derive(Entry)]
        struct Order {
            #[nested]
            ship_to: Address,
        }

        let document = crate::spec::SpecBuilder::new("Shop", "1.0.0")
            .describing::<Order>()
            .build()
            .expect("the document is valid");

        let json = document.to_json().to_json();
        assert!(json.contains("\"Order\""));
        assert!(json.contains("\"Address\""));
    }
}
