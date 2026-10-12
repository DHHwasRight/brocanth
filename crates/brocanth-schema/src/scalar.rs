//! Booleans, enumerations and literals.

use std::borrow::Cow;

use crate::issue::{quote_received, Issues, Path};
use crate::node::{EnumNode, LiteralValue, SchemaNode};
use crate::schema::Schema;
use crate::value::Value;
use crate::IssueCode;

// ---------------------------------------------------------------------------
// Booleans
// ---------------------------------------------------------------------------

/// A boolean schema. Built by [`boolean`].
///
/// There are no constraints on it, and that is not an omission: the only
/// constraint a boolean can carry is "it must be `true`", which is
/// [`literal`] with a `true` in it.
#[derive(Clone, Copy, Debug, Default)]
pub struct BoolSchema;

/// A boolean schema.
#[must_use]
pub fn boolean() -> BoolSchema {
    BoolSchema
}

impl Schema for BoolSchema {
    type Input = bool;
    type Output = bool;

    fn describe(&self) -> SchemaNode {
        SchemaNode::Boolean
    }

    fn check(&self, _input: &bool, _at: &mut Path, _issues: &mut Issues) {}

    fn parse_at(&self, input: &bool, _at: &mut Path, _issues: &mut Issues) -> Option<bool> {
        Some(*input)
    }
}

// ---------------------------------------------------------------------------
// Enumerations
// ---------------------------------------------------------------------------

/// A fixed set of strings. Built by [`one_of`].
#[derive(Clone, Debug, Default)]
pub struct EnumSchema {
    node: EnumNode,
}

/// A schema accepting exactly the listed strings.
///
/// ```
/// use brocanth_schema::{one_of, Schema};
///
/// let kind = one_of(["todo", "doing", "done"]);
/// assert!(kind.validate("done").is_ok());
///
/// let issues = kind.validate("maybe").unwrap_err();
/// assert_eq!(
///     issues.messages_at(""),
///     vec![r#"must be one of "todo", "doing", "done""#]
/// );
/// ```
pub fn one_of<V: Into<Cow<'static, str>>>(variants: impl IntoIterator<Item = V>) -> EnumSchema {
    EnumSchema {
        node: EnumNode {
            variants: variants.into_iter().map(Into::into).collect(),
        },
    }
}

impl EnumSchema {
    /// Every accepted value, in declaration order.
    #[must_use]
    pub fn variants(&self) -> &[Cow<'static, str>] {
        &self.node.variants
    }

    fn expected(&self) -> String {
        format!(
            "one of {}",
            self.node
                .variants
                .iter()
                .map(|v| format!("{v:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl Schema for EnumSchema {
    type Input = str;
    type Output = String;

    fn describe(&self) -> SchemaNode {
        SchemaNode::Enum(self.node.clone())
    }

    fn check(&self, input: &str, at: &mut Path, issues: &mut Issues) {
        if !self.node.variants.iter().any(|v| v == input) {
            issues.report(at, IssueCode::NotInEnum, |i| {
                i.expected(self.expected()).received(quote_received(input))
            });
        }
    }

    fn parse_at(&self, input: &str, at: &mut Path, issues: &mut Issues) -> Option<String> {
        let before = issues.len();
        self.check(input, at, issues);
        (issues.len() == before).then(|| input.to_string())
    }
}

// ---------------------------------------------------------------------------
// Literals
// ---------------------------------------------------------------------------

/// Exactly one value. Built by [`literal`].
#[derive(Clone, Debug)]
pub struct LiteralSchema {
    value: LiteralValue,
}

/// A schema accepting exactly `value` and nothing else.
///
/// Put several in a [`union`](crate::union) for a discriminated union:
/// `union().or(literal("circle")).or(literal("square"))`.
pub fn literal(value: impl Into<LiteralValue>) -> LiteralSchema {
    LiteralSchema {
        value: value.into(),
    }
}

impl LiteralSchema {
    /// The one accepted value.
    #[must_use]
    pub fn value(&self) -> &LiteralValue {
        &self.value
    }

    fn matches(&self, input: &Value) -> bool {
        match (&self.value, input) {
            (LiteralValue::String(a), Value::Text(b)) => a == b,
            (LiteralValue::Integer(a), Value::Int(b)) => a == b,
            (LiteralValue::Number(a), Value::Float(b)) => a == b,
            (LiteralValue::Number(a), Value::Int(b)) => *a == *b as f64,
            (LiteralValue::Boolean(a), Value::Bool(b)) => a == b,
            (LiteralValue::Null, Value::Null) => true,
            _ => false,
        }
    }
}

impl Schema for LiteralSchema {
    type Input = Value;
    type Output = ();

    fn describe(&self) -> SchemaNode {
        SchemaNode::Literal(self.value.clone())
    }

    fn check(&self, input: &Value, at: &mut Path, issues: &mut Issues) {
        if !self.matches(input) {
            issues.report(at, IssueCode::NotLiteral, |i| {
                i.expected(self.value.to_json()).received(input.render())
            });
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
    use super::*;

    #[test]
    fn enum_accepts_only_what_it_lists() {
        let kind = one_of(["todo", "done"]);
        assert!(kind.validate("todo").is_ok());
        assert!(kind.validate("Todo").is_err());
    }

    #[test]
    fn literal_is_type_aware() {
        assert!(literal(1i64).validate(&Value::Int(1)).is_ok());
        assert!(literal(1i64).validate(&Value::Text("1".into())).is_err());
        assert!(literal(true).validate(&Value::Bool(false)).is_err());
    }
}
