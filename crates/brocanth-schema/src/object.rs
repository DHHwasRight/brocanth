//! Objects: named fields, required or not, nested, and a policy for the keys
//! nobody declared.
//!
//! This is the builder for data that is **not** a struct — a JSON body whose
//! shape is decided at run time, a form whose fields come from the database.
//! When there *is* a struct, use `#[derive(Validate)]` instead: it gets the
//! field list from the struct, so a field name cannot be misspelled and a
//! field cannot be forgotten.
//!
//! That difference is worth being plain about. A name written here is a
//! string, and a string is checked when the data arrives. A name written on a
//! struct is a field, and a field is checked when the crate is compiled.

use std::borrow::Cow;

use crate::issue::{Issues, Path};
use crate::node::{FieldNode, ObjectNode, SchemaNode, UnknownKeys};
use crate::schema::{wrong_type, Schema, ValueSchema};
use crate::value::{ReadValue, Value};
use crate::IssueCode;

/// One declared field.
struct Field {
    name: Cow<'static, str>,
    required: bool,
    schema: Box<dyn ValueSchema>,
    description: Option<Cow<'static, str>>,
}

/// An object schema. Built by [`object`].
pub struct ObjectSchema {
    fields: Vec<Field>,
    unknown_keys: UnknownKeys,
}

/// An object schema with no fields yet.
///
/// Unknown keys are rejected unless you say otherwise; see [`UnknownKeys`].
///
/// ```
/// use brocanth_schema::{int, object, string, Schema, Value};
///
/// let account = object()
///     .field("email", string().email())
///     .field("age", int().min(13).optional());
///
/// let ok = Value::object([("email", Value::text("a@b.co"))]);
/// assert!(account.validate(&ok).is_ok());
///
/// let bad = Value::object([("email", Value::text("nope")), ("admin", Value::Bool(true))]);
/// let issues = account.validate(&bad).unwrap_err();
/// assert_eq!(issues.paths(), vec!["email", "admin"]);
/// ```
#[must_use]
pub fn object() -> ObjectSchema {
    ObjectSchema {
        fields: Vec::new(),
        unknown_keys: UnknownKeys::Reject,
    }
}

impl ObjectSchema {
    /// Declare a field.
    ///
    /// Whether it is required is read from the schema, not passed separately:
    /// an `optional` or `default` schema makes the field optional, and nothing
    /// else does. One decision, in one place.
    #[must_use]
    pub fn field<S>(self, name: impl Into<Cow<'static, str>>, schema: S) -> Self
    where
        S: Schema + Send + Sync + 'static,
        S::Input: ReadValue,
    {
        self.described_field(name, schema, None::<&str>)
    }

    /// Declare a field with one line of human-readable description, which
    /// becomes an OpenAPI `description` and an admin form's hint.
    #[must_use]
    pub fn described_field<S>(
        mut self,
        name: impl Into<Cow<'static, str>>,
        schema: S,
        description: Option<impl Into<Cow<'static, str>>>,
    ) -> Self
    where
        S: Schema + Send + Sync + 'static,
        S::Input: ReadValue,
    {
        let required = !schema.describe().is_optional();
        self.fields.push(Field {
            name: name.into(),
            required,
            schema: Box::new(schema),
            description: description.map(Into::into),
        });
        self
    }

    /// Drop a key nobody declared, as zod does.
    #[must_use]
    pub fn unknown_keys_ignored(mut self) -> Self {
        self.unknown_keys = UnknownKeys::Ignore;
        self
    }

    /// Keep a key nobody declared, unvalidated.
    #[must_use]
    pub fn unknown_keys_passed_through(mut self) -> Self {
        self.unknown_keys = UnknownKeys::Passthrough;
        self
    }

    /// Set the policy explicitly.
    #[must_use]
    pub fn unknown_keys(mut self, policy: UnknownKeys) -> Self {
        self.unknown_keys = policy;
        self
    }

    /// The declared field names, in declaration order.
    #[must_use]
    pub fn field_names(&self) -> Vec<&str> {
        self.fields.iter().map(|f| f.name.as_ref()).collect()
    }
}

impl std::fmt::Debug for ObjectSchema {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectSchema")
            .field("fields", &self.field_names())
            .field("unknown_keys", &self.unknown_keys.as_str())
            .finish()
    }
}

impl Schema for ObjectSchema {
    type Input = Value;
    type Output = ();

    fn describe(&self) -> SchemaNode {
        SchemaNode::Object(Box::new(ObjectNode {
            fields: self
                .fields
                .iter()
                .map(|f| FieldNode {
                    name: f.name.clone(),
                    required: f.required,
                    schema: f.schema.describe_value(),
                    description: f.description.clone(),
                })
                .collect(),
            unknown_keys: self.unknown_keys,
        }))
    }

    fn check(&self, input: &Value, at: &mut Path, issues: &mut Issues) {
        let Value::Object(entries) = input else {
            wrong_type(input, "an object", at, issues);
            return;
        };

        for field in &self.fields {
            match entries.iter().find(|(key, _)| key == &field.name) {
                Some((_, value)) => {
                    at.with_field(field.name.clone(), |at| {
                        field.schema.check_value(value, at, issues);
                    });
                }
                None if field.required => {
                    at.with_field(field.name.clone(), |at| {
                        issues.report(at, IssueCode::Missing, |i| {
                            i.expected("a value")
                                .received("nothing")
                                .message("is required")
                        });
                    });
                }
                None => {}
            }
        }

        if self.unknown_keys == UnknownKeys::Reject {
            for (key, _) in entries {
                if !self.fields.iter().any(|f| f.name == key.as_str()) {
                    at.with_field(key.clone(), |at| {
                        issues.report(at, IssueCode::UnknownKey, |i| {
                            i.expected("a declared field")
                                .received(format!("the unexpected key `{key}`"))
                                .message("is not a field of this object")
                        });
                    });
                }
            }
        }
    }

    fn parse_at(&self, input: &Value, at: &mut Path, issues: &mut Issues) -> Option<()> {
        let before = issues.len();
        self.check(input, at, issues);
        (issues.len() == before).then_some(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{array, int, object, string};

    use super::*;

    fn note(kind: &str) -> Value {
        Value::object([("kind", Value::text(kind))])
    }

    #[test]
    fn a_missing_required_field_is_reported_at_its_own_path() {
        let schema = object().field("email", string());
        let issues = schema.validate(&Value::Object(Vec::new())).unwrap_err();
        assert!(issues.has_code("email", &IssueCode::Missing));
        assert_eq!(issues.messages_at("email"), vec!["is required"]);
    }

    #[test]
    fn an_optional_field_may_be_absent_and_is_still_checked_when_present() {
        let schema = object().field("age", int().min(13).optional());
        assert!(schema.validate(&Value::Object(Vec::new())).is_ok());
        assert!(schema
            .validate(&Value::object([("age", Value::Int(12))]))
            .is_err());
    }

    #[test]
    fn nested_paths_carry_field_and_index() {
        let schema = object().field(
            "notes",
            array(object().field("kind", crate::one_of(["todo", "done"]))),
        );
        let value = Value::object([("notes", Value::array([note("todo"), note("maybe")]))]);
        let issues = schema.validate(&value).unwrap_err();
        assert_eq!(issues.paths(), vec!["notes[1].kind"]);
    }

    #[test]
    fn unknown_keys_are_rejected_by_default_and_ignorable_on_request() {
        let strict = object().field("a", string());
        let loose = object().field("a", string()).unknown_keys_ignored();
        let value = Value::object([("a", Value::text("x")), ("b", Value::text("y"))]);
        assert!(strict.validate(&value).is_err());
        assert!(loose.validate(&value).is_ok());
    }

    #[test]
    fn every_bad_field_is_reported_not_only_the_first() {
        let schema = object()
            .field("a", string().min_length(2))
            .field("b", int().min(1))
            .field("c", string().email());
        let value = Value::object([
            ("a", Value::text("x")),
            ("b", Value::Int(0)),
            ("c", Value::text("nope")),
        ]);
        let issues = schema.validate(&value).unwrap_err();
        assert_eq!(issues.paths(), vec!["a", "b", "c"]);
    }
}
