//! The brocanth model layer.
//!
//! SeaORM 2.0.4 is the ORM. This crate is the six things a Rails-shaped
//! framework needs that an ORM does not supply:
//!
//! 1. **Declarative validations** — [`validates`], [`Validate`], [`Errors`].
//!    SeaORM's hooks are the enforcement point, not the vocabulary.
//! 2. **Eager loading that counts parents** — [`eager`]. `.limit(n)` followed
//!    by `find_with_related` limits *joined rows*, so "ten users with their
//!    posts" silently returns fewer than ten users. [`Eager::with_related`]
//!    routes a limited query through the two-query loader instead.
//! 3. **Hook coverage for set-based writes** — [`bulk`]. Fan out by default,
//!    bypass only with a written reason.
//! 4. **N+1 detection** — [`Probe`], and [`Deferred`] for the louder half:
//!    an unloaded relation says so rather than quietly querying.
//! 5. **Dirty tracking** — [`Tracked`], with `changed()`, `was()`, `changes()`.
//! 6. **A transactional test harness** — [`testing::Harness`]: begin, run,
//!    never commit.
//!
//! Plus [`registry`], which enumerates the models in a binary, and
//! [`schema_sync`], which is the single adapter standing between brocanth and
//! SeaORM's semver-exempt `schema-sync`.
//!
//! Nothing here is global. A probe, a registry and a harness are values an
//! application holds, so two applications in one process cannot collide.
//!
//! # A model
//!
//! ```ignore
//! use brocanth_model::prelude::*;
//!
//! #[brocanth_model::validates(
//!     name(presence, length(min = 2, max = 80)),
//!     email(presence, format(r"^[^@\s]+@[^@\s]+$"), uniqueness),
//! )]
//! #[sea_orm::model]
//! #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
//! #[sea_orm(table_name = "user")]
//! pub struct Model {
//!     #[sea_orm(primary_key)]
//!     pub id: i32,
//!     pub name: String,
//!     #[sea_orm(unique)]
//!     pub email: String,
//!     #[sea_orm(has_many)]
//!     pub posts: HasMany<super::post::Entity>,
//! }
//! ```
//!
//! The attribute writes both the [`Validate`] impl and the
//! `ActiveModelBehavior` that calls it, so a write cannot skip validation and
//! so the `#[async_trait]` the hook needs is never forgotten — a bare
//! `async fn` in that impl fails with a lifetime error that does not explain
//! itself.
//!
//! # Rendering an error next to a form field
//!
//! ```ignore
//! match active.insert_checked(db).await {
//!     Ok(user) => redirect_to(user),
//!     Err(e) if e.is_invalid() => {
//!         let errors = e.errors().expect("invalid");
//!         // one input's messages:
//!         render_form(&form, errors.on("email"))
//!     }
//!     Err(e) => Err(e),
//! }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod bulk;
pub mod deferred;
pub mod eager;
pub mod errors;
pub mod prelude;
pub mod probe;
pub mod registry;
pub mod schema_sync;
pub mod testing;
pub mod validation;
pub mod validators;

mod dirty;

pub use dirty::{Change, Tracked};

pub use deferred::{Deferred, NotLoaded};
pub use eager::{Eager, EagerExt, EagerStrategy, WithRelated};
pub use errors::{Entry, Error, Errors};
pub use probe::{Probe, ProbeConfig, Report, Suspect, UnitOfWork};
pub use registry::{ModelInfo, ModelRegistry};
pub use validation::{enforce, Validate, Validated, Write};

/// Declare validations for a model.
///
/// Put it **above** `#[sea_orm::model]`, so it runs first and leaves the rest
/// of the attributes untouched. It writes two things into the same module: an
/// impl of [`Validate`] for `ActiveModel`, and the `ActiveModelBehavior` whose
/// `before_save` calls [`enforce`].
///
/// ```ignore
/// #[brocanth_model::validates(
///     title(presence, length(min = 3, max = 120)),
///     slug(presence, format(r"^[a-z0-9-]+$"), uniqueness),
///     views(numericality(min = 0, only_integer)),
///     status(inclusion("draft", "published"), message = "must be draft or published"),
/// )]
/// ```
///
/// Rules:
///
/// - `presence` — rejects `NotSet`, `NULL` and blank text.
/// - `length(min = N, max = N, is = N)` — characters, not bytes.
/// - `format(r"pattern")` — compiled on first use, held in a `static`.
/// - `numericality(greater_than, min, less_than, max, equal_to, only_integer)`.
/// - `inclusion("a", "b")` — integers compare by their decimal spelling.
/// - `uniqueness` — queries, excluding the record's own row on an update. Takes
///   `column = "Name"` when the `Column` variant is not the field name in
///   PascalCase.
///
/// Every rule takes `message = "..."` to replace the default text.
///
/// Add `no_hook` as the first argument to get the [`Validate`] impl without the
/// `ActiveModelBehavior`, when the model needs a hook of its own. Call
/// [`enforce`] from it, or validation stops being enforced.
pub use brocanth_model_derive::validates;

#[doc(hidden)]
pub use async_trait::async_trait;

/// Re-exported so a model file needs one dependency, and so the version this
/// crate was built against is the version a model compiles against.
pub use sea_orm;
/// Re-exported for the same reason as [`sea_orm`].
pub use sea_orm_migration;
