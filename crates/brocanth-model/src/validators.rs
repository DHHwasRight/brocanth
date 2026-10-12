//! The validation vocabulary.
//!
//! SeaORM's hooks are the enforcement point, not the vocabulary. These are the
//! rules `#[validates(..)]` expands into; call them by hand when a rule needs
//! logic a declaration cannot express.

use std::sync::OnceLock;

use sea_orm::sea_query::ValueTuple;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbErr, EntityTrait, Iterable, PaginatorTrait,
    PrimaryKeyToColumn, QueryFilter, Value,
};

use crate::Errors;

/// A field value flattened to the few shapes a validation cares about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Scalar<'a> {
    /// The column is `NotSet`: the write will not mention it at all.
    Absent,
    /// The column is set to SQL `NULL`.
    Null,
    /// A text value.
    Str(&'a str),
    /// An integer value.
    Int(i64),
    /// A floating point value.
    Float(f64),
    /// A boolean value.
    Bool(bool),
    /// Set to something none of the rules can inspect (a timestamp, a blob).
    Opaque,
}

impl Scalar<'_> {
    /// `true` when the column carries no value at all.
    pub fn is_missing(&self) -> bool {
        matches!(self, Scalar::Absent | Scalar::Null)
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            Scalar::Int(i) => Some(*i as f64),
            Scalar::Float(f) => Some(*f),
            _ => None,
        }
    }
}

/// Flatten a field value into a [`Scalar`].
pub trait AsScalar {
    /// Produce the scalar view of this value.
    fn as_scalar(&self) -> Scalar<'_>;
}

/// Shorthand for [`AsScalar::as_scalar`], so generated code stays short.
pub fn scalar<T: AsScalar + ?Sized>(value: &T) -> Scalar<'_> {
    value.as_scalar()
}

impl AsScalar for String {
    fn as_scalar(&self) -> Scalar<'_> {
        Scalar::Str(self)
    }
}

impl AsScalar for str {
    fn as_scalar(&self) -> Scalar<'_> {
        Scalar::Str(self)
    }
}

impl AsScalar for bool {
    fn as_scalar(&self) -> Scalar<'_> {
        Scalar::Bool(*self)
    }
}

macro_rules! scalar_int {
    ($($t:ty),*) => {$(
        impl AsScalar for $t {
            fn as_scalar(&self) -> Scalar<'_> { Scalar::Int(*self as i64) }
        }
    )*};
}
scalar_int!(i8, i16, i32, i64, u8, u16, u32, u64);

macro_rules! scalar_float {
    ($($t:ty),*) => {$(
        impl AsScalar for $t {
            fn as_scalar(&self) -> Scalar<'_> { Scalar::Float(*self as f64) }
        }
    )*};
}
scalar_float!(f32, f64);

macro_rules! scalar_opaque {
    ($($t:ty),*) => {$(
        impl AsScalar for $t {
            fn as_scalar(&self) -> Scalar<'_> { Scalar::Opaque }
        }
    )*};
}
scalar_opaque!(
    Vec<u8>,
    sea_orm::prelude::Date,
    sea_orm::prelude::Time,
    sea_orm::prelude::DateTime,
    sea_orm::prelude::DateTimeUtc,
    sea_orm::prelude::DateTimeLocal,
    sea_orm::prelude::DateTimeWithTimeZone
);

impl<T: AsScalar> AsScalar for Option<T> {
    fn as_scalar(&self) -> Scalar<'_> {
        match self {
            Some(v) => v.as_scalar(),
            None => Scalar::Null,
        }
    }
}

impl<T> AsScalar for sea_orm::ActiveValue<T>
where
    T: AsScalar + Into<Value>,
{
    fn as_scalar(&self) -> Scalar<'_> {
        match self {
            sea_orm::ActiveValue::Set(v) | sea_orm::ActiveValue::Unchanged(v) => v.as_scalar(),
            sea_orm::ActiveValue::NotSet => Scalar::Absent,
        }
    }
}

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// Rejects `NotSet`, `NULL`, and text that is empty or only whitespace.
pub fn presence(errors: &mut Errors, field: &str, value: Scalar<'_>, message: Option<&str>) {
    let blank = match value {
        Scalar::Absent | Scalar::Null => true,
        Scalar::Str(s) => s.trim().is_empty(),
        _ => false,
    };
    if blank {
        errors.add(field, "blank", message.unwrap_or("can't be blank"));
    }
}

/// Bounds for [`length`], counted in characters, not bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct Length {
    /// Shortest accepted length.
    pub min: Option<usize>,
    /// Longest accepted length.
    pub max: Option<usize>,
    /// Exact accepted length.
    pub is: Option<usize>,
}

/// Checks the character count of a text value. Skips missing values, which is
/// `presence`'s job.
pub fn length(
    errors: &mut Errors,
    field: &str,
    value: Scalar<'_>,
    bounds: Length,
    message: Option<&str>,
) {
    let Scalar::Str(s) = value else {
        return;
    };
    let n = s.chars().count();
    if let Some(is) = bounds.is {
        if n != is {
            errors.add(
                field,
                "wrong_length",
                message.map_or_else(
                    || format!("is the wrong length (should be {is} characters)"),
                    str::to_owned,
                ),
            );
            return;
        }
    }
    if let Some(min) = bounds.min {
        if n < min {
            errors.add(
                field,
                "too_short",
                message.map_or_else(
                    || format!("is too short (minimum is {min} characters)"),
                    str::to_owned,
                ),
            );
        }
    }
    if let Some(max) = bounds.max {
        if n > max {
            errors.add(
                field,
                "too_long",
                message.map_or_else(
                    || format!("is too long (maximum is {max} characters)"),
                    str::to_owned,
                ),
            );
        }
    }
}

/// A regular expression compiled on first use.
///
/// `#[validates(.., format(r"..."))]` puts one of these in a `static`, so the
/// pattern costs nothing until a record is validated and nothing again after.
#[derive(Debug)]
pub struct Pattern {
    source: &'static str,
    compiled: OnceLock<regex::Regex>,
}

impl Pattern {
    /// Hold a pattern without compiling it.
    pub const fn new(source: &'static str) -> Self {
        Self {
            source,
            compiled: OnceLock::new(),
        }
    }

    /// The pattern as written.
    pub fn source(&self) -> &'static str {
        self.source
    }

    /// Compile on first call, then match.
    ///
    /// # Panics
    ///
    /// If the pattern does not compile. The attribute macro checks what it can
    /// at compile time, but the full grammar is `regex`'s.
    pub fn is_match(&self, text: &str) -> bool {
        self.compiled
            .get_or_init(|| {
                regex::Regex::new(self.source)
                    .unwrap_or_else(|e| panic!("invalid validation pattern `{}`: {e}", self.source))
            })
            .is_match(text)
    }
}

/// Checks a text value against a pattern. Skips missing values.
pub fn format(
    errors: &mut Errors,
    field: &str,
    value: Scalar<'_>,
    pattern: &Pattern,
    message: Option<&str>,
) {
    let Scalar::Str(s) = value else {
        return;
    };
    if !pattern.is_match(s) {
        errors.add(field, "invalid", message.unwrap_or("is invalid"));
    }
}

/// Bounds for [`numericality`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Numericality {
    /// Value must be strictly greater than this.
    pub greater_than: Option<f64>,
    /// Value must be greater than or equal to this.
    pub greater_than_or_equal_to: Option<f64>,
    /// Value must be strictly less than this.
    pub less_than: Option<f64>,
    /// Value must be less than or equal to this.
    pub less_than_or_equal_to: Option<f64>,
    /// Value must equal this.
    pub equal_to: Option<f64>,
    /// Value must have no fractional part.
    pub only_integer: bool,
}

/// Checks a numeric value. Skips missing values; flags a non-number.
pub fn numericality(
    errors: &mut Errors,
    field: &str,
    value: Scalar<'_>,
    bounds: Numericality,
    message: Option<&str>,
) {
    if value.is_missing() {
        return;
    }
    let Some(n) = value.as_f64() else {
        errors.add(field, "not_a_number", message.unwrap_or("is not a number"));
        return;
    };
    let fail = |errors: &mut Errors, code: &str, default: String| {
        errors.add(field, code, message.map_or(default, str::to_owned));
    };
    if bounds.only_integer && n.fract() != 0.0 {
        fail(errors, "not_an_integer", "must be an integer".to_owned());
    }
    if let Some(b) = bounds.greater_than {
        if n <= b {
            fail(errors, "greater_than", format!("must be greater than {b}"));
        }
    }
    if let Some(b) = bounds.greater_than_or_equal_to {
        if n < b {
            fail(
                errors,
                "greater_than_or_equal_to",
                format!("must be greater than or equal to {b}"),
            );
        }
    }
    if let Some(b) = bounds.less_than {
        if n >= b {
            fail(errors, "less_than", format!("must be less than {b}"));
        }
    }
    if let Some(b) = bounds.less_than_or_equal_to {
        if n > b {
            fail(
                errors,
                "less_than_or_equal_to",
                format!("must be less than or equal to {b}"),
            );
        }
    }
    if let Some(b) = bounds.equal_to {
        if n != b {
            fail(errors, "equal_to", format!("must be equal to {b}"));
        }
    }
}

/// Checks that a value is one of `allowed`. Integers are compared by their
/// decimal spelling, so `inclusion(1, 2, 3)` works on an `i32` column.
pub fn inclusion(
    errors: &mut Errors,
    field: &str,
    value: Scalar<'_>,
    allowed: &[&str],
    message: Option<&str>,
) {
    let ok = match value {
        Scalar::Absent | Scalar::Null => return,
        Scalar::Str(s) => allowed.contains(&s),
        Scalar::Int(i) => allowed.iter().any(|a| a.parse::<i64>() == Ok(i)),
        Scalar::Bool(b) => allowed.iter().any(|a| a.parse::<bool>() == Ok(b)),
        Scalar::Float(f) => allowed.iter().any(|a| a.parse::<f64>() == Ok(f)),
        Scalar::Opaque => true,
    };
    if !ok {
        errors.add(
            field,
            "inclusion",
            message.unwrap_or("is not an allowed value"),
        );
    }
}

/// Checks that no other row holds this value.
///
/// This is the rule that needs a connection, which is the whole reason SeaORM
/// 2.0's hooks are usable as an enforcement point: `before_save` takes one.
/// `exclude` is the record's own primary key on an update, so a record does not
/// collide with itself.
pub async fn uniqueness<E, V, C>(
    errors: &mut Errors,
    field: &str,
    column: E::Column,
    value: V,
    exclude: Option<ValueTuple>,
    db: &C,
    message: Option<&str>,
) -> Result<(), DbErr>
where
    E: EntityTrait,
    E::Model: Send + Sync,
    V: Into<Value>,
    C: ConnectionTrait,
{
    let mut query = E::find().filter(column.eq(value));
    if let Some(pk) = exclude {
        let mut same_row = Condition::all();
        for (col, v) in E::PrimaryKey::iter()
            .map(PrimaryKeyToColumn::into_column)
            .zip(pk)
        {
            same_row = same_row.add(col.eq(v));
        }
        query = query.filter(same_row.not());
    }
    if query.count(db).await? > 0 {
        errors.add(field, "taken", message.unwrap_or("has already been taken"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_rejects_blank_text_and_missing_columns() {
        let mut e = Errors::new();
        presence(&mut e, "a", Scalar::Str("   "), None);
        presence(&mut e, "b", Scalar::Absent, None);
        presence(&mut e, "c", Scalar::Null, None);
        presence(&mut e, "d", Scalar::Str("x"), None);
        presence(&mut e, "f", Scalar::Int(0), None);
        assert_eq!(e.fields(), ["a", "b", "c"]);
    }

    #[test]
    fn length_counts_characters_not_bytes() {
        let mut e = Errors::new();
        length(
            &mut e,
            "t",
            Scalar::Str("éé"),
            Length {
                min: Some(3),
                ..Default::default()
            },
            None,
        );
        assert!(e.has_code("t", "too_short"));
    }

    #[test]
    fn numericality_bounds() {
        let mut e = Errors::new();
        numericality(
            &mut e,
            "n",
            Scalar::Int(-1),
            Numericality {
                greater_than_or_equal_to: Some(0.0),
                ..Default::default()
            },
            None,
        );
        assert!(e.has_code("n", "greater_than_or_equal_to"));

        let mut e = Errors::new();
        numericality(
            &mut e,
            "n",
            Scalar::Float(1.5),
            Numericality {
                only_integer: true,
                ..Default::default()
            },
            None,
        );
        assert!(e.has_code("n", "not_an_integer"));
    }

    #[test]
    fn inclusion_matches_integers_by_their_spelling() {
        let mut e = Errors::new();
        inclusion(&mut e, "s", Scalar::Int(2), &["1", "2"], None);
        inclusion(&mut e, "t", Scalar::Str("x"), &["a", "b"], None);
        assert_eq!(e.fields(), ["t"]);
    }

    #[test]
    fn a_pattern_compiles_once() {
        static P: Pattern = Pattern::new(r"^[a-z0-9-]+$");
        assert!(P.is_match("hello-world"));
        assert!(!P.is_match("Hello World"));
        assert_eq!(P.source(), "^[a-z0-9-]+$");
    }

    #[test]
    fn active_values_flatten_through_their_state() {
        use sea_orm::ActiveValue;
        let set: ActiveValue<String> = ActiveValue::Set("x".to_owned());
        let unchanged: ActiveValue<String> = ActiveValue::Unchanged("y".to_owned());
        let not_set: ActiveValue<String> = ActiveValue::NotSet;
        let null: ActiveValue<Option<String>> = ActiveValue::Set(None);
        assert_eq!(scalar(&set), Scalar::Str("x"));
        assert_eq!(scalar(&unchanged), Scalar::Str("y"));
        assert_eq!(scalar(&not_set), Scalar::Absent);
        assert_eq!(scalar(&null), Scalar::Null);
    }
}
