//! `Validate`: the vocabulary attached to a struct.
//!
//! `#[derive(Validate)]` writes both halves of this trait from one
//! declaration, which is the whole point of the derive: the description a form
//! builder reads and the code that checks a request body come from the same
//! source, so they cannot drift apart.
//!
//! Implement it by hand when a rule needs logic the attributes cannot express.
//! The two compose, because the derive only ever writes one impl per type.

use crate::issue::{Issues, Path};
use crate::node::SchemaNode;

/// A type the vocabulary has been attached to.
pub trait Validate {
    /// The description of this type, for `brocanth-openapi`,
    /// `brocanth-admin`, and anything else that wants to know the shape.
    fn schema() -> SchemaNode
    where
        Self: Sized;

    /// Record every failure, with `at` as the path of this value.
    ///
    /// Nested types call this on their children after pushing a field or an
    /// index, which is what makes `notes[0].kind` come out right.
    fn validate_at(&self, at: &mut Path, issues: &mut Issues);

    /// Check this value from the root.
    ///
    /// # Errors
    ///
    /// Every issue found, never just the first.
    fn validate(&self) -> Result<(), Issues> {
        let mut at = Path::root();
        let mut issues = Issues::new();
        self.validate_at(&mut at, &mut issues);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(issues)
        }
    }
}

impl<T: Validate> Validate for Box<T> {
    fn schema() -> SchemaNode {
        T::schema()
    }

    fn validate_at(&self, at: &mut Path, issues: &mut Issues) {
        (**self).validate_at(at, issues);
    }
}
