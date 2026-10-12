//! Numbers, in two schemas rather than one.
//!
//! [`int`] and [`float`] are separate types because `multiple_of` means
//! something exact for whole numbers and something approximate for floats, and
//! because `integer()` is a constraint on a float and a tautology on an
//! integer. One schema covering both would have to accept `int().integer()`
//! and do nothing with it; two do not.

use crate::issue::{render_f64, Issues, Path};
use crate::node::{IntegerNode, NumberNode, SchemaNode};
use crate::schema::Schema;
use crate::IssueCode;

// ---------------------------------------------------------------------------
// Whole numbers
// ---------------------------------------------------------------------------

/// A whole-number schema. Built by [`int`].
#[derive(Clone, Copy, Debug, Default)]
pub struct IntSchema {
    node: IntegerNode,
}

/// A whole-number schema with no constraints yet.
///
/// ```
/// use brocanth_schema::{int, Schema};
///
/// let age = int().min(13).max(130);
/// assert!(age.validate(&30).is_ok());
/// assert!(age.validate(&12).is_err());
/// ```
#[must_use]
pub fn int() -> IntSchema {
    IntSchema::default()
}

impl IntSchema {
    /// At least `n`.
    #[must_use]
    pub fn min(mut self, n: i64) -> Self {
        self.node.min = Some(n);
        self
    }

    /// At most `n`.
    #[must_use]
    pub fn max(mut self, n: i64) -> Self {
        self.node.max = Some(n);
        self
    }

    /// Strictly greater than `n`.
    #[must_use]
    pub fn exclusive_min(mut self, n: i64) -> Self {
        self.node.exclusive_min = Some(n);
        self
    }

    /// Strictly less than `n`.
    #[must_use]
    pub fn exclusive_max(mut self, n: i64) -> Self {
        self.node.exclusive_max = Some(n);
        self
    }

    /// A multiple of `n`.
    ///
    /// # Panics
    ///
    /// If `n` is zero, which no value can be a multiple of.
    #[must_use]
    pub fn multiple_of(mut self, n: i64) -> Self {
        assert!(n != 0, "multiple_of(0) can never be satisfied");
        self.node.multiple_of = Some(n);
        self
    }

    /// Greater than zero.
    #[must_use]
    pub fn positive(self) -> Self {
        self.exclusive_min(0)
    }

    /// Zero or greater.
    #[must_use]
    pub fn non_negative(self) -> Self {
        self.min(0)
    }
}

impl Schema for IntSchema {
    type Input = i64;
    type Output = i64;

    fn describe(&self) -> SchemaNode {
        SchemaNode::Integer(self.node)
    }

    fn check(&self, input: &i64, at: &mut Path, issues: &mut Issues) {
        let value = *input;
        if let Some(min) = self.node.min {
            if value < min {
                issues.report(at, IssueCode::TooSmall, |i| {
                    i.expected(format!("at least {min}")).received(value.to_string())
                });
            }
        }
        if let Some(max) = self.node.max {
            if value > max {
                issues.report(at, IssueCode::TooLarge, |i| {
                    i.expected(format!("at most {max}")).received(value.to_string())
                });
            }
        }
        if let Some(bound) = self.node.exclusive_min {
            if value <= bound {
                issues.report(at, IssueCode::TooSmall, |i| {
                    i.expected(format!("greater than {bound}"))
                        .received(value.to_string())
                });
            }
        }
        if let Some(bound) = self.node.exclusive_max {
            if value >= bound {
                issues.report(at, IssueCode::TooLarge, |i| {
                    i.expected(format!("less than {bound}"))
                        .received(value.to_string())
                });
            }
        }
        if let Some(step) = self.node.multiple_of {
            if value % step != 0 {
                issues.report(at, IssueCode::NotMultipleOf, |i| {
                    i.expected(format!("a multiple of {step}"))
                        .received(value.to_string())
                });
            }
        }
    }

    fn parse_at(&self, input: &i64, at: &mut Path, issues: &mut Issues) -> Option<i64> {
        let before = issues.len();
        self.check(input, at, issues);
        (issues.len() == before).then_some(*input)
    }
}

// ---------------------------------------------------------------------------
// Fractional numbers
// ---------------------------------------------------------------------------

/// A fractional-number schema. Built by [`float`].
#[derive(Clone, Copy, Debug, Default)]
pub struct FloatSchema {
    node: NumberNode,
}

/// A fractional-number schema with no constraints yet.
///
/// ```
/// use brocanth_schema::{float, Schema};
///
/// let ratio = float().min(0.0).max(1.0).finite();
/// assert!(ratio.validate(&0.5).is_ok());
/// assert!(ratio.validate(&f64::NAN).is_err());
/// ```
#[must_use]
pub fn float() -> FloatSchema {
    FloatSchema::default()
}

impl FloatSchema {
    /// At least `n`.
    #[must_use]
    pub fn min(mut self, n: f64) -> Self {
        self.node.min = Some(n);
        self
    }

    /// At most `n`.
    #[must_use]
    pub fn max(mut self, n: f64) -> Self {
        self.node.max = Some(n);
        self
    }

    /// Strictly greater than `n`.
    #[must_use]
    pub fn exclusive_min(mut self, n: f64) -> Self {
        self.node.exclusive_min = Some(n);
        self
    }

    /// Strictly less than `n`.
    #[must_use]
    pub fn exclusive_max(mut self, n: f64) -> Self {
        self.node.exclusive_max = Some(n);
        self
    }

    /// A multiple of `n`, within the tolerance floating point allows.
    ///
    /// # Panics
    ///
    /// If `n` is zero or not finite.
    #[must_use]
    pub fn multiple_of(mut self, n: f64) -> Self {
        assert!(
            n != 0.0 && n.is_finite(),
            "multiple_of({n}) can never be satisfied"
        );
        self.node.multiple_of = Some(n);
        self
    }

    /// No fractional part. `2.0` passes, `2.5` does not.
    #[must_use]
    pub fn integer(mut self) -> Self {
        self.node.integer = true;
        self
    }

    /// Neither `NaN` nor an infinity.
    #[must_use]
    pub fn finite(mut self) -> Self {
        self.node.finite = true;
        self
    }
}

impl Schema for FloatSchema {
    type Input = f64;
    type Output = f64;

    fn describe(&self) -> SchemaNode {
        SchemaNode::Number(self.node)
    }

    fn check(&self, input: &f64, at: &mut Path, issues: &mut Issues) {
        let value = *input;

        if self.node.finite && !value.is_finite() {
            issues.report(at, IssueCode::NotFinite, |i| {
                i.expected("a finite number").received(render_f64(value))
            });
        }
        if self.node.integer && (!value.is_finite() || value.fract() != 0.0) {
            issues.report(at, IssueCode::NotInteger, |i| {
                i.expected("a whole number").received(render_f64(value))
            });
        }

        // A comparison against NaN is false either way, so an unordered value
        // would quietly pass every bound. Report it once instead.
        if value.is_nan() {
            if !self.node.finite && self.has_bounds() {
                issues.report(at, IssueCode::NotFinite, |i| {
                    i.expected("a comparable number").received("NaN")
                });
            }
            return;
        }

        if let Some(min) = self.node.min {
            if value < min {
                issues.report(at, IssueCode::TooSmall, |i| {
                    i.expected(format!("at least {}", render_f64(min)))
                        .received(render_f64(value))
                });
            }
        }
        if let Some(max) = self.node.max {
            if value > max {
                issues.report(at, IssueCode::TooLarge, |i| {
                    i.expected(format!("at most {}", render_f64(max)))
                        .received(render_f64(value))
                });
            }
        }
        if let Some(bound) = self.node.exclusive_min {
            if value <= bound {
                issues.report(at, IssueCode::TooSmall, |i| {
                    i.expected(format!("greater than {}", render_f64(bound)))
                        .received(render_f64(value))
                });
            }
        }
        if let Some(bound) = self.node.exclusive_max {
            if value >= bound {
                issues.report(at, IssueCode::TooLarge, |i| {
                    i.expected(format!("less than {}", render_f64(bound)))
                        .received(render_f64(value))
                });
            }
        }
        if let Some(step) = self.node.multiple_of {
            let quotient = value / step;
            let off = (quotient - quotient.round()).abs();
            if !value.is_finite() || off > 1e-9 {
                issues.report(at, IssueCode::NotMultipleOf, |i| {
                    i.expected(format!("a multiple of {}", render_f64(step)))
                        .received(render_f64(value))
                });
            }
        }
    }

    fn parse_at(&self, input: &f64, at: &mut Path, issues: &mut Issues) -> Option<f64> {
        let before = issues.len();
        self.check(input, at, issues);
        (issues.len() == before).then_some(*input)
    }
}

impl FloatSchema {
    fn has_bounds(&self) -> bool {
        self.node.min.is_some()
            || self.node.max.is_some()
            || self.node.exclusive_min.is_some()
            || self.node.exclusive_max.is_some()
            || self.node.multiple_of.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_broken_bound_is_reported_not_just_the_first() {
        let schema = int().min(10).multiple_of(3);
        let issues = schema.validate(&4).unwrap_err();
        assert_eq!(issues.len(), 2);
        assert!(issues.has_code("", &IssueCode::TooSmall));
        assert!(issues.has_code("", &IssueCode::NotMultipleOf));
    }

    #[test]
    fn exclusive_bounds_exclude() {
        assert!(int().exclusive_min(0).validate(&0).is_err());
        assert!(int().exclusive_min(0).validate(&1).is_ok());
        assert!(int().exclusive_max(10).validate(&10).is_err());
        assert!(int().exclusive_max(10).validate(&9).is_ok());
    }

    #[test]
    fn nan_cannot_slip_past_a_bound() {
        let issues = float().min(0.0).validate(&f64::NAN).unwrap_err();
        assert_eq!(issues.len(), 1);
    }

    #[test]
    fn float_multiple_of_tolerates_representation() {
        assert!(float().multiple_of(0.1).validate(&0.3).is_ok());
        assert!(float().multiple_of(0.1).validate(&0.35).is_err());
    }
}
