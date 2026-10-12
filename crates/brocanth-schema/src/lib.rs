//! Validation for brocanth: a zod-shaped vocabulary whose *shape* the compiler
//! checks and whose *data* it cannot.
//!
//! # Read this part first
//!
//! The request that produced this crate was "compile-time validation using
//! zod, which can plug to OpenAPI specs". Two of those three words are
//! deliverable in Rust and one is not, and a validation library that is vague
//! about which is which is worse than no library at all. So, plainly:
//!
//! **Checked when you compile — the shape of the schema.**
//!
//! * A constraint applied to a type it cannot mean anything for is a compile
//!   error. `min_length` does not exist on a number schema, and
//!   `#[validate(string(..))]` on an `i64` field fails to build with a message
//!   that says why. It is not a rule that silently never fires.
//! * A schema that disagrees with the struct it validates is a compile error,
//!   because the derive reads the field list from the struct. There is no
//!   string to misspell, so "validating a field that does not exist" is not a
//!   mistake you can make.
//! * A field whose type has no description is a compile error, so the
//!   description an OpenAPI emitter reads cannot have a hole in it.
//! * A handler that takes a validated body does not compile unless the
//!   application registered the plugin. See [`plugin`].
//!
//! All four are strictly better than zod, which finds every one of them at run
//! time, on the first request that happens to exercise the path.
//!
//! **Checked when a request arrives — the data.**
//!
//! A request body is bytes that did not exist when you compiled, so no type
//! system anywhere can check them in advance. `"abc"` is too short, `-3` is
//! below the minimum, `notes[0].kind` is not one of the three allowed values:
//! every one of those is a run-time answer here, in zod, and in any library
//! that will ever exist.
//!
//! What the type system buys is not earlier checking of the data. It is that
//! **the checking code is generated from the same declaration the struct
//! uses**, so the two cannot drift. The form, the OpenAPI document, the admin
//! widget and the request check are four readings of one source.
//!
//! Anything that calls itself compile-time validation and means "run-time
//! validation with a derive macro" is a claim a user discovers is false in
//! production. This crate does not make it.
//!
//! # The vocabulary
//!
//! | | |
//! |---|---|
//! | [`string`] | `min_length` `max_length` `length` `non_empty` `pattern` `email` `url` `uuid` `starts_with` `ends_with` `contains` |
//! | [`int`] | `min` `max` `exclusive_min` `exclusive_max` `multiple_of` `positive` `non_negative` |
//! | [`float`] | the same, plus `integer` and `finite` |
//! | [`boolean`] | — |
//! | [`date`] [`time`] [`datetime`] | `min` `max` |
//! | [`one_of`] [`literal`] | enumerations and literal unions |
//! | [`array`] | `min_items` `max_items` `length` `non_empty` `unique`, and a schema per element |
//! | [`object`] | required and optional fields, nesting, and an unknown-key policy |
//! | combinators | [`optional`](Schema::optional) [`nullable`](Schema::nullable) [`with_default`](Schema::with_default) [`union`] [`intersection`] [`refine`](Schema::refine) [`transform`](Schema::transform) |
//!
//! # The two ways in
//!
//! A struct gets `#[derive(Validate)]`:
//!
//! ```
//! use brocanth_schema::{Validate, SchemaNode};
//!
//! #[derive(Validate)]
//! struct SignUp {
//!     #[validate(string(min_length = 3, max_length = 32))]
//!     username: String,
//!     #[validate(string(email))]
//!     email: String,
//!     #[validate(int(min = 13, max = 130))]
//!     age: i64,
//!     #[validate(array(min_items = 1, max_items = 5, unique), each(string(non_empty)))]
//!     tags: Vec<String>,
//! }
//!
//! let bad = SignUp {
//!     username: "ab".into(),
//!     email: "nope".into(),
//!     age: 9,
//!     tags: vec![String::new()],
//! };
//!
//! let issues = bad.validate().unwrap_err();
//! assert_eq!(issues.paths(), vec!["username", "email", "age", "tags[0]"]);
//! assert!(matches!(SignUp::schema(), SchemaNode::Object(_)));
//! ```
//!
//! Data with no struct behind it gets the builder:
//!
//! ```
//! use brocanth_schema::{object, string, Schema, Value};
//!
//! let schema = object().field("email", string().email());
//! let body = Value::object([("email", Value::text("a@b.co"))]);
//! assert!(schema.validate(&body).is_ok());
//! ```
//!
//! # What does not compile
//!
//! A length on a number:
//!
//! ```compile_fail
//! use brocanth_schema::int;
//! let _ = int().min_length(3);
//! ```
//!
//! A step on a string:
//!
//! ```compile_fail
//! use brocanth_schema::string;
//! let _ = string().multiple_of(2);
//! ```
//!
//! A string constraint on a whole-number field:
//!
//! ```compile_fail
//! use brocanth_schema::Validate;
//!
//! #[derive(Validate)]
//! struct Post {
//!     #[validate(string(min_length = 3))]
//!     views: i64,
//! }
//! ```
//!
//! An array constraint on something that is not a sequence:
//!
//! ```compile_fail
//! use brocanth_schema::Validate;
//!
//! #[derive(Validate)]
//! struct Post {
//!     #[validate(array(min_items = 1))]
//!     title: String,
//! }
//! ```
//!
//! A date literal that is not a date — rejected by the derive, before the
//! schema is ever built:
//!
//! ```compile_fail
//! use brocanth_schema::Validate;
//!
//! #[derive(Validate)]
//! struct Post {
//!     #[validate(date(min = "2026-02-30"))]
//!     published_on: String,
//! }
//! ```
//!
//! A field whose type nothing can describe:
//!
//! ```compile_fail
//! use brocanth_schema::Validate;
//!
//! struct Opaque;
//!
//! #[derive(Validate)]
//! struct Post {
//!     blob: Opaque,
//! }
//! ```
//!
//! # Nothing ships unless it is asked for
//!
//! Every dependency is optional. `default-features = false` leaves the
//! vocabulary, [`Issues`] and [`SchemaNode`] with nothing underneath them,
//! which is what `brocanth-openapi` and `brocanth-admin` want: neither
//! validates anything, so neither should link the framework or a regex
//! engine. The `pattern` feature is the regex engine, `plugin` is the
//! framework, `derive` is the proc macro, and with all three off this crate
//! has no dependencies at all.

#![forbid(unsafe_code)]

pub mod array;
pub mod datetime;
pub mod input;
pub mod issue;
pub mod node;
pub mod number;
pub mod object;
pub mod scalar;
pub mod schema;
pub mod string;
pub mod validate;
pub mod value;

#[cfg(feature = "plugin")]
pub mod plugin;

pub use crate::array::{array, ArraySchema};
pub use crate::datetime::{
    date, datetime, time, Date, DateError, DateSchema, DateTime, DateTimeSchema, Time, TimeSchema,
};
pub use crate::input::{AsBool, AsFloat, AsInt, AsSequence, AsText, Describe};
pub use crate::issue::{Issue, IssueCode, Issues, Path, Segment};
pub use crate::node::{
    ArrayNode, EnumNode, FieldNode, IntegerNode, LiteralValue, NumberNode, ObjectNode, RangeNode,
    SchemaNode, StringFormat, StringNode, UnknownKeys,
};
pub use crate::number::{float, int, FloatSchema, IntSchema};
pub use crate::object::{object, ObjectSchema};
pub use crate::scalar::{boolean, literal, one_of, BoolSchema, EnumSchema, LiteralSchema};
pub use crate::schema::{
    intersection, union, Intersection, Nullable, Optional, Refined, Schema, Transformed, Union,
    ValueSchema, WithDefault,
};
pub use crate::string::{string, StringSchema};
pub use crate::validate::Validate;
pub use crate::value::{ReadValue, Value};

#[cfg(feature = "plugin")]
pub use crate::plugin::{ReportStyle, Valid, Validation, ValidationConfig, ValidationPlugin};

/// `#[derive(Validate)]`.
///
/// The derive and the [`Validate`] trait share a name on purpose: one is in
/// the macro namespace and one is in the type namespace, and `use
/// brocanth_schema::Validate;` brings in both.
#[cfg(feature = "derive")]
pub use brocanth_schema_derive::Validate;

/// Everything an application that writes schemas needs, in one import.
pub mod prelude {
    pub use crate::{
        array, boolean, date, datetime, float, int, intersection, literal, object, one_of, string,
        time, union,
    };
    pub use crate::{Issue, IssueCode, Issues, Path, Schema, SchemaNode, Validate, Value};

    #[cfg(feature = "plugin")]
    pub use crate::{Valid, Validation, ValidationConfig, ValidationPlugin};
}
