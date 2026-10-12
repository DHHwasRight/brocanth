//! Arrays: how many, all different, and one schema for every element.
//!
//! The element schema is a real schema, so an issue inside element 3 of field
//! `notes` comes back at `notes[3].kind` and not at `notes`.

use std::borrow::Borrow;

use crate::issue::{count, Issues, Path};
use crate::node::{ArrayNode, SchemaNode};
use crate::schema::{wrong_type, Schema};
use crate::value::{ReadValue, Value};
use crate::IssueCode;

/// A sequence schema. Built by [`array`].
#[derive(Clone, Debug)]
pub struct ArraySchema<S> {
    items: S,
    min_items: Option<usize>,
    max_items: Option<usize>,
    exact_items: Option<usize>,
    unique: bool,
}

/// A sequence whose every element must satisfy `items`.
///
/// ```
/// use brocanth_schema::{array, string, Schema, Value};
///
/// let tags = array(string().min_length(2)).max_items(3).unique();
///
/// let bad = [Value::text("ok"), Value::text("x"), Value::text("ok")];
/// let issues = tags.validate(&bad).unwrap_err();
///
/// assert_eq!(issues.paths(), vec!["[1]", "[2]"]);
/// ```
pub fn array<S: Schema>(items: S) -> ArraySchema<S> {
    ArraySchema {
        items,
        min_items: None,
        max_items: None,
        exact_items: None,
        unique: false,
    }
}

impl<S> ArraySchema<S> {
    /// At least `n` elements.
    #[must_use]
    pub fn min_items(mut self, n: usize) -> Self {
        self.min_items = Some(n);
        self
    }

    /// At most `n` elements.
    #[must_use]
    pub fn max_items(mut self, n: usize) -> Self {
        self.max_items = Some(n);
        self
    }

    /// Exactly `n` elements.
    #[must_use]
    pub fn length(mut self, n: usize) -> Self {
        self.exact_items = Some(n);
        self
    }

    /// At least one element.
    #[must_use]
    pub fn non_empty(self) -> Self {
        self.min_items(1)
    }

    /// No two elements may be equal.
    #[must_use]
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// The element schema.
    pub fn items(&self) -> &S {
        &self.items
    }

    /// Report only the length constraints.
    ///
    /// `#[derive(Validate)]` calls [`check_length`] directly, because a
    /// `Vec<Note>` of nested structs is not a sequence of `Value`s and so has
    /// no `ArraySchema` to hang off.
    pub fn check_length(&self, len: usize, at: &mut Path, issues: &mut Issues) {
        check_length(
            len,
            self.min_items,
            self.max_items,
            self.exact_items,
            at,
            issues,
        );
    }

    /// Report a repeated element, at the index of the repeat.
    ///
    /// Does nothing unless [`ArraySchema::unique`] was asked for.
    pub fn check_unique<T: PartialEq>(&self, items: &[T], at: &mut Path, issues: &mut Issues) {
        if self.unique {
            check_unique(items, at, issues);
        }
    }

    /// Whether uniqueness was asked for.
    #[must_use]
    pub fn is_unique(&self) -> bool {
        self.unique
    }
}

impl<S: Schema> ArraySchema<S> {
    /// Check a slice whose elements already have the element schema's input
    /// type: `&[String]` against `array(string())`.
    pub fn check_slice<T>(&self, items: &[T], at: &mut Path, issues: &mut Issues)
    where
        T: Borrow<S::Input> + PartialEq,
    {
        self.check_length(items.len(), at, issues);
        for (index, item) in items.iter().enumerate() {
            at.with_index(index, |at| self.items.check(item.borrow(), at, issues));
        }
        self.check_unique(items, at, issues);
    }

    fn node(&self) -> SchemaNode {
        SchemaNode::Array(Box::new(ArrayNode {
            items: self.items.describe(),
            min_items: self.min_items,
            max_items: self.max_items,
            exact_items: self.exact_items,
            unique: self.unique,
        }))
    }
}

impl<S> Schema for ArraySchema<S>
where
    S: Schema,
    S::Input: ReadValue,
{
    type Input = [Value];
    type Output = Vec<S::Output>;

    fn describe(&self) -> SchemaNode {
        self.node()
    }

    fn check(&self, input: &[Value], at: &mut Path, issues: &mut Issues) {
        self.check_length(input.len(), at, issues);
        for (index, element) in input.iter().enumerate() {
            at.with_index(index, |at| {
                match <S::Input as ReadValue>::with(element, |v| self.items.check(v, at, issues)) {
                    Ok(()) => {}
                    Err(expected) => wrong_type(element, expected, at, issues),
                }
            });
        }
        self.check_unique(input, at, issues);
    }

    fn parse_at(
        &self,
        input: &[Value],
        at: &mut Path,
        issues: &mut Issues,
    ) -> Option<Vec<S::Output>> {
        let before = issues.len();
        self.check_length(input.len(), at, issues);
        let mut out = Vec::with_capacity(input.len());
        for (index, element) in input.iter().enumerate() {
            at.with_index(index, |at| {
                match <S::Input as ReadValue>::with(element, |v| self.items.parse_at(v, at, issues))
                {
                    Ok(Some(value)) => out.push(value),
                    Ok(None) => {}
                    Err(expected) => wrong_type(element, expected, at, issues),
                }
            });
        }
        self.check_unique(input, at, issues);
        (issues.len() == before).then_some(out)
    }
}

// ---------------------------------------------------------------------------
// The free functions the derive emits
// ---------------------------------------------------------------------------

/// Report the length constraints on a sequence of anything.
///
/// Public because `#[derive(Validate)]` emits a call to it: the derived code
/// walks a `Vec<T>` of nested structs, which no [`ArraySchema`] can hold.
pub fn check_length(
    len: usize,
    min: Option<usize>,
    max: Option<usize>,
    exact: Option<usize>,
    at: &mut Path,
    issues: &mut Issues,
) {
    if let Some(min) = min {
        if len < min {
            issues.report(at, IssueCode::TooShort, |i| {
                let expected = format!("at least {}", count(min, "element", "elements"));
                let message = format!("must have {expected}");
                i.expected(expected)
                    .received(count(len, "element", "elements"))
                    .message(message)
            });
        }
    }
    if let Some(max) = max {
        if len > max {
            issues.report(at, IssueCode::TooLong, |i| {
                let expected = format!("at most {}", count(max, "element", "elements"));
                let message = format!("must have {expected}");
                i.expected(expected)
                    .received(count(len, "element", "elements"))
                    .message(message)
            });
        }
    }
    if let Some(exact) = exact {
        if len != exact {
            issues.report(at, IssueCode::WrongLength, |i| {
                let expected = format!("exactly {}", count(exact, "element", "elements"));
                let message = format!("must have {expected}");
                i.expected(expected)
                    .received(count(len, "element", "elements"))
                    .message(message)
            });
        }
    }
}

/// Report every repeated element, each at the index of the repeat.
pub fn check_unique<T: PartialEq>(items: &[T], at: &mut Path, issues: &mut Issues) {
    for (index, item) in items.iter().enumerate() {
        if let Some(first) = items[..index].iter().position(|other| other == item) {
            at.with_index(index, |at| {
                issues.report(at, IssueCode::NotUnique, |i| {
                    i.expected("a value that appears once")
                        .received(format!("a repeat of element {first}"))
                        .message("must not be repeated")
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{array, string};

    use super::*;

    #[test]
    fn an_issue_inside_an_element_keeps_its_index() {
        let schema = array(string().min_length(2));
        let value = [Value::text("ok"), Value::text("x")];
        let issues = schema.validate(&value).unwrap_err();
        assert_eq!(issues.paths(), vec!["[1]"]);
    }

    #[test]
    fn every_bad_element_is_reported() {
        let schema = array(string().min_length(2));
        let value = [Value::text("a"), Value::text("b"), Value::text("ok")];
        let issues = schema.validate(&value).unwrap_err();
        assert_eq!(issues.len(), 2);
    }

    #[test]
    fn a_wrong_kind_of_element_is_a_type_issue_not_a_panic() {
        let schema = array(string());
        let value = [Value::Int(1)];
        let issues = schema.validate(&value).unwrap_err();
        assert!(issues.has_code("[0]", &IssueCode::WrongType));
    }

    #[test]
    fn check_slice_takes_a_vec_of_owned_strings() {
        let schema = array(string().min_length(2)).min_items(2);
        let values = vec!["ok".to_string(), "x".to_string()];
        let mut at = Path::root();
        let mut issues = Issues::new();
        schema.check_slice(&values, &mut at, &mut issues);
        assert_eq!(issues.paths(), vec!["[1]"]);
    }
}
