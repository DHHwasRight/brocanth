//! The subject's attributes: the facts about "who" that a rule reads.
//!
//! docs/policy-rls-bridge.md, decision 5. A rule that reads `who.user_id`
//! runs twice: in the application, with the subject's value, and in the
//! database, with the setting `brocanth.user_id`. So the subject says which
//! of its fields a rule may read, and what kind each one is.
//!
//! ```ignore
//! #[derive(brocanth_policy::Subject)]
//! pub struct Visitor {
//!     #[attr] pub user_id: Option<i64>,   // brocanth.user_id  bigint
//!     #[attr] pub admin: bool,            // brocanth.admin    boolean
//!     pub display_name: String,           // not an attribute
//! }
//! ```
//!
//! The derive (feature `macro`) implements [`Subject`]: [`Subject::ATTRS`]
//! lists each `#[attr]` field as an [`Attr`], and [`Subject::attributes`]
//! reads their values into [`Attributes`]. A field without `#[attr]` never
//! reaches the database, so an email does not leak into a setting.
//!
//! Plain data, no driver: `brocanth-rls` depends on this crate and turns
//! each [`Attr`] into a typed setting, never the reverse.

use crate::value::Value;

/// The database type of a subject attribute, and of its setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AttrKind {
    /// `boolean`, from `bool`.
    Boolean,
    /// `integer`, from `i32`.
    Integer,
    /// `bigint`, from `i64`.
    BigInt,
    /// `text`, from `String`.
    Text,
    /// `uuid`, from `uuid::Uuid` (feature `uuid`).
    Uuid,
    /// `timestamptz`, from `chrono::DateTime<Utc>` (feature `chrono`).
    Timestamptz,
}

impl AttrKind {
    /// The PostgreSQL type name, as a cast writes it.
    pub const fn sql_type(&self) -> &'static str {
        match self {
            AttrKind::Boolean => "boolean",
            AttrKind::Integer => "integer",
            AttrKind::BigInt => "bigint",
            AttrKind::Text => "text",
            AttrKind::Uuid => "uuid",
            AttrKind::Timestamptz => "timestamptz",
        }
    }
}

/// One subject attribute: a field a rule may read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Attr {
    /// The field name. The setting is `brocanth.<key>`.
    pub key: &'static str,
    /// The database type.
    pub kind: AttrKind,
    /// `true` for an `Option` field: the subject can lack the value.
    pub optional: bool,
}

/// The values of a subject's attributes, for one request.
///
/// `None` is an absent value: an `Option` field that is `None`. A key that
/// is not here at all reads the same way. A rule then applies its absence
/// mode (`else deny`, `else null`), as the database does with an empty
/// setting.
///
/// An empty text is absent too: [`Attributes::set`] stores
/// `Some(Value::Text(""))` as `None`. Reason: the database cannot tell them
/// apart. A transaction cannot unset a setting, so an absent value is set
/// to `''`, and a policy reads `''` as `NULL`. Without this rule, a subject
/// with `role: String::new()` passes `who.role != "admin"` in the
/// application and fails it in the database (the agreement test of
/// docs/policy-rls-bridge.md decision 10 found it). Use a non-empty value,
/// such as `"guest"`, for a role that a rule must see.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attributes {
    values: Vec<(&'static str, Option<Value>)>,
}

impl Attributes {
    /// No attributes.
    pub fn new() -> Self {
        Self::default()
    }

    /// The same attributes, with `key` set to `value`. A second call with
    /// the same key replaces the value.
    pub fn with(mut self, key: &'static str, value: Option<Value>) -> Self {
        self.set(key, value);
        self
    }

    /// Set `key` to `value`, in place. An empty text is `None`.
    pub fn set(&mut self, key: &'static str, value: Option<Value>) {
        let value = value.filter(|v| !matches!(v, Value::Text(t) if t.is_empty()));
        match self.values.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.values.push((key, value)),
        }
    }

    /// The value of `key`. `None` when it is absent or not set at all.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.as_ref())
    }

    /// Each key and its value, in the order they were set.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, Option<&Value>)> {
        self.values.iter().map(|(k, v)| (*k, v.as_ref()))
    }
}

/// A type whose values a rule may read as subject attributes.
///
/// `#[derive(Subject)]` implements it. By hand, keep [`Subject::ATTRS`] and
/// [`Subject::attributes`] in step: one entry per key, in both.
pub trait Subject {
    /// Each attribute, in field order.
    const ATTRS: &'static [Attr];

    /// The values of [`Subject::ATTRS`] for this subject.
    fn attributes(&self) -> Attributes;
}

/// A Rust type a subject attribute can have. `#[derive(Subject)]` reads it.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a type a subject attribute can have",
    label = "unsupported attribute type",
    note = "Use bool, i32, i64, String, uuid::Uuid (feature `uuid`) or chrono::DateTime<Utc> (feature `chrono`), or an `Option` of one of them. Or remove `#[attr]`: a field without it never reaches the database."
)]
pub trait AttrType {
    /// The database type.
    const KIND: AttrKind;

    /// `true` for `Option<T>`.
    const OPTIONAL: bool;

    /// The value, or `None` when it is absent.
    fn attr_value(&self) -> Option<Value>;
}

macro_rules! attr_types {
    ($($ty:ty => $kind:ident;)*) => {$(
        impl AttrType for $ty {
            const KIND: AttrKind = AttrKind::$kind;
            const OPTIONAL: bool = false;
            fn attr_value(&self) -> Option<Value> {
                Some(Value::from(self.clone()))
            }
        }
        impl AttrType for Option<$ty> {
            const KIND: AttrKind = AttrKind::$kind;
            const OPTIONAL: bool = true;
            fn attr_value(&self) -> Option<Value> {
                self.clone().map(Value::from)
            }
        }
    )*};
}

attr_types! {
    bool => Boolean;
    i32 => Integer;
    i64 => BigInt;
    String => Text;
}

#[cfg(feature = "uuid")]
attr_types! {
    uuid::Uuid => Uuid;
}

#[cfg(feature = "chrono")]
attr_types! {
    chrono::DateTime<chrono::Utc> => Timestamptz;
}
