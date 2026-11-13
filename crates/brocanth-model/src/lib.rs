#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! The brocanth model layer.
//!
//! SeaORM 2.0.4 is the ORM, and **Postgres is the only database**
//! (2026-11-11): RLS and one database keep the framework simple. The
//! `postgres` feature is the default and must be on.
//!
//! This crate is the six things a Rails-shaped framework needs that an ORM
//! does not supply:
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
//! 7. **Publishing** — [`changes`]. A write reports one
//!    [`RowChange`](brocanth_events::RowChange) to a
//!    [`ChangeSink`](brocanth_events::ChangeSink), so a row written through
//!    the ORM reaches realtime, an audit log or a plugin hook. PLAN.md §10's
//!    publisher half.
//!
//! 8. **Fixtures** — [`fixtures`]. A known dataset from a JSON file, checked
//!    against the entities and written through their hooks (Django D27).
//!
//! Plus [`registry`], which enumerates the models in a binary, and
//! [`migrating`], which applies migrations under a lock. `migrate make`
//! (`brocanth-migrate`) writes the migrations from a diff of two snapshots
//! of the models; SeaORM's semver-exempt `schema-sync` is not used.
//!
//! With the `db` feature, `Orm` is SeaORM over the pool a `brocanth_db::Db`
//! opened, so the ORM and sea-query statements share one pool (`orm`).
//! With the `policy` feature, `policy::condition` turns a `brocanth-policy`
//! condition into a SeaORM `WHERE`, so a list filters before it paginates.
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

#[cfg(all(feature = "runtime", not(feature = "postgres")))]
compile_error!(
    "brocanth-model needs the Postgres driver. Enable the `postgres` feature \
     (the default). The driver is a feature on `sea-orm` itself, so this crate \
     cannot pick it for you after the fact."
);

#[cfg(feature = "runtime")]
pub mod bulk;
#[cfg(feature = "runtime")]
pub mod changes;
#[cfg(feature = "runtime")]
pub mod deferred;
#[cfg(feature = "runtime")]
pub mod eager;
#[cfg(feature = "runtime")]
pub mod errors;
#[cfg(feature = "runtime")]
pub mod fixtures;
#[cfg(feature = "runtime")]
pub mod migrating;
#[cfg(feature = "db")]
pub mod orm;
#[cfg(feature = "plugin")]
pub mod plugin;
#[cfg(feature = "policy")]
pub mod policy;
#[cfg(feature = "runtime")]
pub mod prelude;
#[cfg(feature = "runtime")]
pub mod probe;
#[cfg(feature = "runtime")]
pub mod registry;
#[cfg(feature = "runtime")]
pub mod testing;
#[cfg(feature = "runtime")]
pub mod validation;
#[cfg(feature = "runtime")]
pub mod validators;
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
mod dirty;

#[cfg(feature = "runtime")]
pub use changes::{Pending, Publish, Writing};
#[cfg(feature = "runtime")]
pub use dirty::{Change, Tracked};

#[cfg(feature = "runtime")]
pub use deferred::{Deferred, NotLoaded};
#[cfg(feature = "runtime")]
pub use eager::{Eager, EagerExt, EagerStrategy, WithRelated};
#[cfg(feature = "runtime")]
pub use errors::{Entry, Error, Errors};
#[cfg(feature = "runtime")]
pub use fixtures::{FixtureError, Fixtures};
#[cfg(feature = "db")]
pub use orm::Orm;
#[cfg(feature = "runtime")]
pub use probe::{Probe, ProbeConfig, Report, Suspect, UnitOfWork};
#[cfg(feature = "runtime")]
pub use registry::{ModelInfo, ModelRegistry};
#[cfg(feature = "runtime")]
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
#[cfg(feature = "runtime")]
pub use brocanth_model_derive::validates;

/// Remove the stable ids from a model before SeaORM sees them.
///
/// `brocanth migrate` identifies a table and a column by an id written in the
/// source text, so that a rename is a rename and not a drop plus an add
/// (PLAN.md §11). The compiler must not see those attributes, so this one
/// takes them off. Put it **above** `#[sea_orm::model]`. [`validates`] does
/// the same, so a model with validations needs no second attribute.
///
/// ```ignore
/// #[brocanth_model::ids]
/// #[sea_orm::model]
/// #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
/// #[sea_orm(table_name = "post")]
/// #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1A")]
/// pub struct Model {
///     #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1B")]
///     #[sea_orm(primary_key)]
///     pub id: i32,
/// }
/// ```
///
/// `brocanth migrate ids` writes the ids and this attribute. Do not write an
/// id by hand, and do not copy one: an id that occurs two times is refused.
#[cfg(feature = "runtime")]
pub use brocanth_model_derive::ids;

#[doc(hidden)]
#[cfg(feature = "runtime")]
pub use async_trait::async_trait;

/// Re-exported so an application can hold a sink, and name the one type the
/// write path publishes, without a second dependency line.
#[cfg(feature = "runtime")]
pub use brocanth_events;

/// Re-exported so a model file needs one dependency, and so the version this
/// crate was built against is the version a model compiles against.
#[cfg(feature = "runtime")]
pub use sea_orm;
/// Re-exported for the same reason as [`sea_orm`].
#[cfg(feature = "runtime")]
pub use sea_orm_migration;
