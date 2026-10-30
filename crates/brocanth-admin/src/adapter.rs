//! The one module left to wire up, and exactly what it has to do.
//!
//! # What this crate assumed about the model layer
//!
//! `brocanth-model` is being written in parallel and SeaORM 2.0 is three
//! months old, so this crate was built against [`crate::fixture::Fixture`]
//! rather than against either. These are the assumptions, stated rather than
//! buried:
//!
//! 1. **The entity registry enumerates models with their module paths.**
//!    `brocanth_model::registry::modules()` returns `Vec<&'static str>` of
//!    module paths, from `inventory::iter::<sea_orm::EntityRegistry>()`. That
//!    is what [`crate::discovery`] filters, and it is why
//!    [`crate::discovery::RegistryEntry::module_path`] exists.
//! 2. **Columns need a concrete entity type.** SeaORM's registry entry carries
//!    a module path and nothing else, so "walk every model's columns" cannot
//!    be written as a loop over the registry. `brocanth-model` already says
//!    so, and already splits `modules()` from `describe::<E>()` for exactly
//!    this reason. **The application therefore names its entities once**, as
//!    it already does for `ModelRegistry::with`, and the admin reads that.
//!    This is the single thing that is not automatic, and it cannot be made
//!    automatic without a macro that sees every entity at once.
//! 3. **`json_schema_from_entity` reports `name`, `type`, `nullable`,
//!    `unique` and a `primary_key` array.** That is the shape
//!    `brocanth_model::registry::describe` parses, and
//!    [`column_from_schema`] takes the same five fields.
//! 4. **Column type spellings** are SeaORM's own, folded by
//!    [`crate::model::ColumnKind::from_type_name`]. An unrecognised spelling
//!    becomes [`crate::model::ColumnKind::Unknown`] and gets a text input, so
//!    a spelling this crate guessed wrong is a cosmetic bug, never a panic.
//!
//! # What SeaORM 2.0.4 does not give, and what is done instead
//!
//! * **Foreign keys are not in the column schema.** `json_schema_from_entity`
//!   describes columns; relations live on `EntityTrait::Relation`, a separate
//!   enum reached through `RelationTrait::def()`, and `brocanth-model`'s
//!   `ColumnInfo` does not carry them either. So the admin cannot infer that
//!   `author_id` points at `user`. It is declared instead, once per column,
//!   with [`crate::config::ModelAdmin::foreign_key`]. This is the one place
//!   where the admin asks for something Django's introspection gets free.
//! * **Enum variants are not in the column schema either.** A SeaORM
//!   `ActiveEnum` reports its values through `ActiveEnum::values()`, which
//!   needs the concrete type. Declare them with
//!   [`crate::model::ColumnDescriptor::variants`] when building the entry, or
//!   force [`crate::form::Widget::Select`] on the column.
//! * **`max_length` is not reported.** `ColumnDef` holds it internally but the
//!   JSON schema does not expose it, so a `String(Some(80))` column arrives
//!   here with no limit. Set it with
//!   [`crate::model::ColumnDescriptor::max_length`] if you want the admin to
//!   check length before the database does.
//! * **There is no "is this column auto-generated" flag.** The admin treats a
//!   single-column integer primary key as generated, which is right for
//!   `#[sea_orm(primary_key)]` and wrong for a natural key you assign
//!   yourself. [`entry_from_schema`] takes that decision and it is one line to
//!   change.
//!
//! # The wiring
//!
//! ```ignore
//! use std::sync::Arc;
//!
//! use brocanth_admin::adapter::{entry_from_schema, SchemaColumn};
//! use brocanth_admin::data::{AdminData, DataError, ListRequest, Search};
//! use brocanth_admin::discovery::RegistryEntry;
//! use brocanth_model::registry::{describe, ModelInfo};
//! use sea_orm::{DatabaseConnection, DbBackend, EntityTrait};
//!
//! struct SeaOrmSource {
//!     db: DatabaseConnection,
//!     entries: Vec<RegistryEntry>,
//! }
//!
//! /// One line per entity. SeaORM's registry has the module path but not the
//! /// columns, so the entity type has to be named once — see assumption 2.
//! macro_rules! describe_entity {
//!     ($module:literal, $entity:expr) => {{
//!         let info: ModelInfo = describe($entity, DbBackend::Postgres);
//!         let columns: Vec<SchemaColumn<'_>> = info
//!             .columns
//!             .iter()
//!             .map(|c| SchemaColumn {
//!                 name: &c.name,
//!                 type_name: &c.type_name,
//!                 nullable: c.nullable,
//!                 unique: c.unique,
//!                 primary_key: c.primary_key,
//!             })
//!             .collect();
//!         entry_from_schema($module, &info.table, &columns)
//!     }};
//! }
//!
//! let entries = vec![
//!     describe_entity!("blog::models::user", blog::models::user::Entity),
//!     describe_entity!("blog::models::post", blog::models::post::Entity),
//! ];
//! ```
//!
//! The six write and read methods are then SeaORM calls against
//! `self.db`, with three rules. The first is PLAN §9's; the other two come
//! straight out of PLAN §7:
//!
//! * **`AND` every [`crate::data::Scope`] into the statement**, on reads and
//!   on writes, and report zero rows for a write the scope refuses.
//!   `brocanth-admin-model` is the worked example.
//! * **Never `update_many` / `delete_many` / `insert_many`** (trap 2): they
//!   bypass the hooks, and the hooks are where validations run. Go through
//!   `ActiveModel::insert` / `::update` / `::delete` one record at a time, or
//!   through `brocanth_model::bulk`, which fans out by default.
//! * **Map a validation failure to [`DataError::Invalid`](crate::data::DataError::Invalid)**, not to
//!   [`DataError::Backend`](crate::data::DataError::Backend). `brocanth_model::Error::errors()` returns the
//!   per-field entries; copying them into [`crate::data::FieldErrors`] is what
//!   puts each message beside its own input instead of in a banner.

use crate::discovery::RegistryEntry;
use crate::model::{ColumnDescriptor, ColumnKind};

/// One column as a schema reports it, before this crate interprets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaColumn<'a> {
    /// Column name.
    pub name: &'a str,
    /// The type's spelling, as SeaORM writes it.
    pub type_name: &'a str,
    /// Whether it accepts `NULL`.
    pub nullable: bool,
    /// Whether it carries a unique constraint.
    pub unique: bool,
    /// Whether it is part of the primary key.
    pub primary_key: bool,
}

/// Turns one schema column into a descriptor.
pub fn column_from_schema(column: &SchemaColumn<'_>) -> ColumnDescriptor {
    let kind = ColumnKind::from_type_name(column.type_name);
    ColumnDescriptor {
        name: column.name.to_string(),
        kind,
        nullable: column.nullable,
        unique: column.unique,
        primary_key: column.primary_key,
        auto_generated: false,
        max_length: None,
        variants: Vec::new(),
        relation: None,
    }
}

/// Turns one entity's schema into a registry entry.
///
/// A single integer primary key is taken to be database-generated, because
/// that is what `#[sea_orm(primary_key)]` means unless `auto_increment =
/// false` was written. A composite key, or a key of any other type, is not —
/// the form shows it and asks for it.
pub fn entry_from_schema(
    module_path: &str,
    table: &str,
    columns: &[SchemaColumn<'_>],
) -> RegistryEntry {
    let key_count = columns.iter().filter(|c| c.primary_key).count();
    let columns = columns
        .iter()
        .map(|column| {
            let mut descriptor = column_from_schema(column);
            descriptor.auto_generated =
                descriptor.primary_key && key_count == 1 && descriptor.kind == ColumnKind::Integer;
            descriptor
        })
        .collect();
    RegistryEntry {
        module_path: module_path.to_string(),
        table: table.to_string(),
        columns,
    }
}
