//! Column types, so a value of the wrong type is a compile error.
//!
//! The algebra itself is untyped on purpose: [`crate::Pred::eq`] takes any
//! `impl Into<Bound>`, because a predicate is a column name and a bind list
//! and nothing else. A hand-written policy can compare a text column with an
//! integer, and only the database notices.
//!
//! The `policy!` macro declares each column with its Rust type, and this
//! module is what it checks against. Every function here is the identity on
//! meaning: [`bind`] returns the same [`Bound`] that `value.into()` returns,
//! [`absent`] the same one [`Bound::or`] returns, and [`set`] the same values
//! [`crate::Pred::one_of`] collects. The macro output is the hand-written
//! policy, with the type of each value checked on the way in.
//!
//! # Three checks
//!
//! 1. **The column type is one this crate binds.** [`ColumnType`] is
//!    implemented for the integers, the floats, `bool`, `String`, `Vec<u8>`
//!    and `Option` of each. Another type is an error at the declaration.
//! 2. **The value has the column's kind.** [`Operand`] connects a Rust type to
//!    a [`Kind`]. An `i32` compares with an `i64` column (both are
//!    [`Int`]); a `&str` does not.
//! 3. **An `Option` is never a value.** `Option<T>` implements [`Operand`]
//!    only when `T: AbsenceIsExplicit`, and no type implements that trait.
//!    So the error names the real problem: the rule does not say what
//!    absence means. PLAN.md §9 is the reason.

use crate::entity::Column;
use crate::value::{Bound, Missing, Value};

/// The kind of value a column holds: [`Int`], [`Float`], [`Text`], [`Bool`]
/// or [`Bytes`].
pub trait Kind {
    /// The name in an error message.
    const NAME: &'static str;
}

/// Integer columns.
pub enum Int {}
/// Floating-point columns.
pub enum Float {}
/// Text columns.
pub enum Text {}
/// Boolean columns.
pub enum Bool {}
/// Blob columns.
pub enum Bytes {}

impl Kind for Int {
    const NAME: &'static str = "integer";
}
impl Kind for Float {
    const NAME: &'static str = "float";
}
impl Kind for Text {
    const NAME: &'static str = "text";
}
impl Kind for Bool {
    const NAME: &'static str = "bool";
}
impl Kind for Bytes {
    const NAME: &'static str = "bytes";
}

/// A Rust type a column can have.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a column type that `policy!` can bind",
    label = "unsupported column type",
    note = "Use i8, i16, i32, i64, u8, u16, u32, f32, f64, bool, String or Vec<u8>, or an `Option` of one of them."
)]
pub trait ColumnType {
    /// What kind of value the column holds.
    type Kind: Kind;

    /// `true` for `Option<T>`: the column can hold `NULL`.
    const NULLABLE: bool;

    /// A few values of this type, for the agreement test of PLAN.md §9
    /// obligation 1. Includes [`Value::Null`] when the column is nullable.
    fn samples() -> Vec<Value>;

    /// The `n`th primary key, when this is the key column.
    fn key(n: i64) -> Value;
}

macro_rules! column_types {
    ($($ty:ty => $kind:ty, [$($sample:expr),*], |$n:ident| $key:expr;)*) => {$(
        impl ColumnType for $ty {
            type Kind = $kind;
            const NULLABLE: bool = false;
            fn samples() -> Vec<Value> {
                vec![$(Value::from($sample)),*]
            }
            fn key($n: i64) -> Value {
                $key
            }
        }
    )*};
}

column_types! {
    i8 => Int, [0i8, 1i8, 2i8], |n| Value::Int(n);
    i16 => Int, [0i16, 1i16, 2i16], |n| Value::Int(n);
    i32 => Int, [0i32, 1i32, 2i32], |n| Value::Int(n);
    i64 => Int, [0i64, 1i64, 2i64], |n| Value::Int(n);
    u8 => Int, [0u8, 1u8, 2u8], |n| Value::Int(n);
    u16 => Int, [0u16, 1u16, 2u16], |n| Value::Int(n);
    u32 => Int, [0u32, 1u32, 2u32], |n| Value::Int(n);
    f32 => Float, [0.0f32, 1.5f32], |n| Value::Float(n as f64);
    f64 => Float, [0.0f64, 1.5f64], |n| Value::Float(n as f64);
    bool => Bool, [false, true], |n| Value::Bool(n % 2 == 1);
    String => Text, ["", "a"], |n| Value::Text(n.to_string());
    Vec<u8> => Bytes, [Vec::<u8>::new(), vec![1u8]], |n| Value::Bytes(n.to_be_bytes().to_vec());
}

impl<T: ColumnType> ColumnType for Option<T> {
    type Kind = T::Kind;
    const NULLABLE: bool = true;
    fn samples() -> Vec<Value> {
        let mut out = T::samples();
        out.push(Value::Null);
        out
    }
    fn key(n: i64) -> Value {
        T::key(n)
    }
}

/// A Rust value that may stand on the right of a comparison with a column of
/// kind `K`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` does not compare with this column",
    label = "wrong type for the column",
    note = "Give a value of the column's own type. This column holds `{K}` values."
)]
pub trait Operand<K> {
    /// The value, as the algebra binds it. The same [`Value`] that
    /// `Value::from(self)` gives.
    fn into_value(self) -> Value;
}

macro_rules! operands {
    ($kind:ty => $($ty:ty),*) => {$(
        impl Operand<$kind> for $ty {
            fn into_value(self) -> Value {
                Value::from(self)
            }
        }
    )*};
}

operands!(Int => i8, i16, i32, i64, u8, u16, u32);
operands!(Float => f32, f64);
operands!(Bool => bool);
operands!(Text => String, &str);
operands!(Bytes => Vec<u8>);

macro_rules! copied_operands {
    ($kind:ty => $($ty:ty),*) => {$(
        impl Operand<$kind> for &$ty {
            fn into_value(self) -> Value {
                Value::from(*self)
            }
        }
    )*};
}

copied_operands!(Int => i8, i16, i32, i64, u8, u16, u32);
copied_operands!(Float => f32, f64);
copied_operands!(Bool => bool);

impl Operand<Text> for &String {
    fn into_value(self) -> Value {
        Value::from(self.as_str())
    }
}

impl Operand<Bytes> for &[u8] {
    fn into_value(self) -> Value {
        Value::from(self.to_vec())
    }
}

/// The bound that refuses an `Option` as a value. No type implements it.
///
/// It exists for its error message. `Option<T>` implements [`Operand`] only
/// under this bound, so the compiler reports the missing decision about
/// absence, not a type mismatch.
#[diagnostic::on_unimplemented(
    message = "this value can be absent. The rule does not say what absence means",
    label = "`Option` value",
    note = "Write `<value> else deny`, `<value> else allow` or `<value> else null`."
)]
pub trait AbsenceIsExplicit {
    /// Never called.
    fn refuse(value: Option<Self>) -> Value
    where
        Self: Sized;
}

impl<K, T: AbsenceIsExplicit> Operand<K> for Option<T> {
    fn into_value(self) -> Value {
        T::refuse(self)
    }
}

/// One value, checked against the column's kind.
///
/// The same [`Bound`] as `value.into()`.
pub fn bind<K: Kind, V: Operand<K>>(value: V) -> Bound {
    Bound::of(value.into_value())
}

/// A value that can be absent, with the author's decision about absence.
///
/// The same [`Bound`] as [`Bound::or`] gives.
pub fn absent<K: Kind, V: Operand<K>>(value: Option<V>, missing: Missing) -> Bound {
    Bound::or(value.map(Operand::into_value), missing)
}

/// A set of values, each checked against the column's kind.
pub fn set<K: Kind, I>(values: I) -> Vec<Value>
where
    I: IntoIterator,
    I::Item: Operand<K>,
{
    values.into_iter().map(Operand::into_value).collect()
}

/// A column enum whose columns have Rust types. `policy!` implements it.
///
/// The agreement test of PLAN.md §9 obligation 1 reads it to build rows.
pub trait TypedColumn: Column {
    /// A few values this column can hold, with [`Value::Null`] when the
    /// column is nullable.
    fn samples(&self) -> Vec<Value>;

    /// The `n`th value of this column when it is the primary key.
    fn key(&self, n: i64) -> Value;
}
