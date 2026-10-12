//! The kind traits: how the compiler knows a constraint can mean something for
//! a field.
//!
//! `#[derive(Validate)]` does not look at the type of a field. It cannot: a
//! proc macro sees tokens, and `type Name = String;` is a `Name` to it. So the
//! derive does not guess. For a string constraint it emits a call to
//! [`AsText::as_text`], for an integer constraint a call to
//! [`AsInt::as_int`], and the ordinary trait solver decides. Put
//! `#[validate(string(min_length = 3))]` on an `i64` and the build fails with
//! the message on [`AsText`], not with a rule that silently never fires.
//!
//! That is the whole mechanism, and it is why these traits are public: a
//! newtype that should accept string constraints implements [`AsText`] and it
//! does.
//!
//! ```
//! use brocanth_schema::{AsText, Describe, SchemaNode, StringNode};
//!
//! struct Slug(String);
//!
//! impl AsText for Slug {
//!     fn as_text(&self) -> &str { &self.0 }
//! }
//! impl Describe for Slug {
//!     fn describe() -> SchemaNode { SchemaNode::String(StringNode::default()) }
//! }
//! ```

use crate::node::{ArrayNode, IntegerNode, NumberNode, SchemaNode, StringNode};

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

/// A field a string constraint can be applied to.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a text field, so a string constraint cannot mean anything for it",
    label = "this field is a `{Self}`",
    note = "use the constraint for its own type — `int(..)`, `float(..)`, `bool`, `array(..)` — or implement `brocanth_schema::AsText` for `{Self}`"
)]
pub trait AsText {
    /// The text to check.
    fn as_text(&self) -> &str;
}

impl AsText for str {
    fn as_text(&self) -> &str {
        self
    }
}

impl AsText for String {
    fn as_text(&self) -> &str {
        self
    }
}

impl AsText for std::borrow::Cow<'_, str> {
    fn as_text(&self) -> &str {
        self
    }
}

impl AsText for Box<str> {
    fn as_text(&self) -> &str {
        self
    }
}

impl<T: AsText + ?Sized> AsText for &T {
    fn as_text(&self) -> &str {
        (**self).as_text()
    }
}

// ---------------------------------------------------------------------------
// Whole numbers
// ---------------------------------------------------------------------------

/// A field an integer constraint can be applied to.
///
/// `u64` and `usize` saturate at [`i64::MAX`]. A value that large failing an
/// upper bound is still the right answer; a lower bound above `i64::MAX` is
/// not expressible, which is the one case this flattening loses.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a whole-number field, so an integer constraint cannot mean anything for it",
    label = "this field is a `{Self}`",
    note = "use `float(..)` for a fractional number, `string(..)` for text, or implement `brocanth_schema::AsInt` for `{Self}`"
)]
pub trait AsInt {
    /// The number to check.
    fn as_int(&self) -> i64;
}

macro_rules! as_int {
    ($($t:ty),* $(,)?) => {
        $(impl AsInt for $t {
            fn as_int(&self) -> i64 {
                i64::from(*self)
            }
        })*
    };
}

as_int!(i8, i16, i32, i64, u8, u16, u32);

impl AsInt for isize {
    fn as_int(&self) -> i64 {
        *self as i64
    }
}

impl AsInt for u64 {
    fn as_int(&self) -> i64 {
        i64::try_from(*self).unwrap_or(i64::MAX)
    }
}

impl AsInt for usize {
    fn as_int(&self) -> i64 {
        i64::try_from(*self).unwrap_or(i64::MAX)
    }
}

impl<T: AsInt + ?Sized> AsInt for &T {
    fn as_int(&self) -> i64 {
        (**self).as_int()
    }
}

// ---------------------------------------------------------------------------
// Fractional numbers
// ---------------------------------------------------------------------------

/// A field a float constraint can be applied to.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a number field, so a float constraint cannot mean anything for it",
    label = "this field is a `{Self}`",
    note = "use `int(..)` for a whole number, `string(..)` for text, or implement `brocanth_schema::AsFloat` for `{Self}`"
)]
pub trait AsFloat {
    /// The number to check.
    fn as_float(&self) -> f64;
}

impl AsFloat for f32 {
    fn as_float(&self) -> f64 {
        f64::from(*self)
    }
}

impl AsFloat for f64 {
    fn as_float(&self) -> f64 {
        *self
    }
}

impl<T: AsFloat + ?Sized> AsFloat for &T {
    fn as_float(&self) -> f64 {
        (**self).as_float()
    }
}

// ---------------------------------------------------------------------------
// Booleans
// ---------------------------------------------------------------------------

/// A field a boolean constraint can be applied to.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a boolean field",
    label = "this field is a `{Self}`",
    note = "implement `brocanth_schema::AsBool` for `{Self}` if it should be treated as one"
)]
pub trait AsBool {
    /// The flag to check.
    fn as_bool(&self) -> bool;
}

impl AsBool for bool {
    fn as_bool(&self) -> bool {
        *self
    }
}

impl<T: AsBool + ?Sized> AsBool for &T {
    fn as_bool(&self) -> bool {
        (**self).as_bool()
    }
}

// ---------------------------------------------------------------------------
// Sequences
// ---------------------------------------------------------------------------

/// A field an array constraint can be applied to.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a sequence, so an array constraint cannot mean anything for it",
    label = "this field is a `{Self}`",
    note = "`array(..)` and `each(..)` need a `Vec<T>`, a slice or an array; implement `brocanth_schema::AsSequence` for `{Self}` otherwise"
)]
pub trait AsSequence {
    /// What the sequence holds.
    type Item;

    /// The elements, in order.
    fn items(&self) -> &[Self::Item];
}

impl<T> AsSequence for Vec<T> {
    type Item = T;

    fn items(&self) -> &[T] {
        self
    }
}

impl<T> AsSequence for [T] {
    type Item = T;

    fn items(&self) -> &[T] {
        self
    }
}

impl<T, const N: usize> AsSequence for [T; N] {
    type Item = T;

    fn items(&self) -> &[T] {
        self
    }
}

impl<T: AsSequence + ?Sized> AsSequence for &T {
    type Item = T::Item;

    fn items(&self) -> &[T::Item] {
        (**self).items()
    }
}

// ---------------------------------------------------------------------------
// Describe
// ---------------------------------------------------------------------------

/// The description a type has when nobody wrote a constraint for it.
///
/// `#[derive(Validate)]` needs a node for every field, including the ones with
/// no attribute, because the object description is what `brocanth-openapi` and
/// `brocanth-admin` read and a hole in it is a missing field in a form. A type
/// the derive cannot describe is a compile error naming this trait, which is
/// the right moment to find out.
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no schema description, so a field of this type cannot be described",
    note = "derive `Validate` on `{Self}`, implement `brocanth_schema::Describe` for it, or mark the field `#[validate(skip)]`"
)]
pub trait Describe {
    /// The node describing this type with no constraints on it.
    fn describe() -> SchemaNode;
}

macro_rules! describe_as {
    ($node:expr => $($t:ty),* $(,)?) => {
        $(impl Describe for $t {
            fn describe() -> SchemaNode { $node }
        })*
    };
}

describe_as!(SchemaNode::String(StringNode::default()) => String, str);
describe_as!(SchemaNode::Integer(IntegerNode::default()) => i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);
describe_as!(SchemaNode::Number(NumberNode::default()) => f32, f64);
describe_as!(SchemaNode::Boolean => bool);

impl<T: Describe + ?Sized> Describe for &T {
    fn describe() -> SchemaNode {
        T::describe()
    }
}

impl<T: Describe + ?Sized> Describe for Box<T> {
    fn describe() -> SchemaNode {
        T::describe()
    }
}

impl<T: Describe> Describe for Option<T> {
    fn describe() -> SchemaNode {
        SchemaNode::Optional(Box::new(T::describe()))
    }
}

impl<T: Describe> Describe for Vec<T> {
    fn describe() -> SchemaNode {
        SchemaNode::Array(Box::new(ArrayNode {
            items: T::describe(),
            min_items: None,
            max_items: None,
            exact_items: None,
            unique: false,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_nests_through_option_and_vec() {
        let node = <Option<Vec<String>> as Describe>::describe();
        assert_eq!(node.kind(), "optional");
        assert_eq!(node.unwrapped().kind(), "array");
    }

    #[test]
    fn unsigned_saturates_rather_than_wrapping() {
        assert_eq!(u64::MAX.as_int(), i64::MAX);
    }
}
