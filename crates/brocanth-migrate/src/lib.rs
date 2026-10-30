//! Stable field identity and a rename-aware migration diff (PLAN.md §11).
//!
//! A struct diff cannot tell a rename from a drop plus an add. So every model
//! and every column field carries an id in the source text:
//!
//! ```ignore
//! #[brocanth_model::ids]
//! #[sea_orm::model]
//! #[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
//! #[sea_orm(table_name = "post")]
//! #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1A")]
//! pub struct Model {
//!     #[brocanth(id = "01JBQ7R8V4K2M9X3T6N5P0WZ1B")]
//!     #[sea_orm(primary_key)]
//!     pub id: i32,
//! }
//! ```
//!
//! `brocanth migrate ids` writes the ids. Nobody edits them by hand. Then
//! `brocanth migrate make <name>` compares the models with the last snapshot
//! by id, and writes a migration file plus a new snapshot. It applies nothing:
//! the generator proposes and the developer commits.
//!
//! The parts, each testable with no database:
//!
//! - [`id`] makes and checks ULID-shaped ids.
//! - [`scan`] reads model source text into tables and fields.
//! - [`schema`] is the snapshot, `migrations/schema.json`.
//! - [`diff`] compares two snapshots by id.
//! - [`render`] writes the SeaORM migration file.
//! - [`stamp`] writes missing ids into source text.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod diff;
pub mod id;
pub mod render;
pub mod scan;
pub mod schema;
pub mod stamp;

pub use diff::{diff, Change};
pub use scan::{scan_source, Model, Problem, ScannedField};
pub use schema::{Field, Schema, Table};
