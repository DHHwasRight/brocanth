//! Enumerating the models in a binary.
//!
//! Two halves, because SeaORM gives two halves.
//!
//! [`modules`] reads the `entity-registry` inventory, which `DeriveEntityModel`
//! fills in with one entry per model. That answers "which models exist", which
//! is what a `routes`-style listing and a migration diff need.
//!
//! [`describe`] takes a concrete entity and reports its columns, their types,
//! nullability, uniqueness and primary key. That is what a scaffold generator
//! and an admin form builder need. SeaORM's registry entry carries no column
//! information, so this half needs the type.
//!
//! [`ModelRegistry`] is the two put together, built explicitly by an
//! application: no global, no ordering surprise, two applications in one
//! process cannot collide.

use sea_orm::{DbBackend, EntityTrait, Schema};

/// Module paths of every model registered in this binary, sorted.
///
/// Comes from `inventory`, so it covers every linked model whether or not the
/// application asked for it.
pub fn modules() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = inventory::iter::<sea_orm::EntityRegistry>()
        .map(|e| e.module_path)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Module paths under one prefix, sorted. `"my_app::models"` matches
/// `my_app::models::user`.
pub fn modules_under(prefix: &str) -> Vec<&'static str> {
    modules()
        .into_iter()
        .filter(|m| m.starts_with(prefix))
        .collect()
}

/// One column, as a generator sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnInfo {
    /// Column name as the database spells it.
    pub name: String,
    /// Coarse type: `string`, `integer`, `real`, `decimal`, `datetime`, and so
    /// on. SeaORM's own spelling, kept rather than re-invented.
    pub type_name: String,
    /// Whether the column accepts `NULL`.
    pub nullable: bool,
    /// Whether the column carries a unique constraint.
    pub unique: bool,
    /// Whether the column is part of the primary key.
    pub primary_key: bool,
}

/// One model, as a generator sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// Table name.
    pub table: String,
    /// Columns, in declaration order.
    pub columns: Vec<ColumnInfo>,
}

impl ModelInfo {
    /// A column by name.
    pub fn column(&self, name: &str) -> Option<&ColumnInfo> {
        self.columns.iter().find(|c| c.name == name)
    }

    /// The primary key columns.
    pub fn primary_key(&self) -> Vec<&ColumnInfo> {
        self.columns.iter().filter(|c| c.primary_key).collect()
    }
}

/// Describe one entity's columns.
pub fn describe<E: EntityTrait>(entity: E, backend: DbBackend) -> ModelInfo {
    let schema = Schema::new(backend);
    let json = schema.json_schema_from_entity(entity);
    let pk: Vec<String> = json
        .get("primary_key")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let columns = json
        .get("columns")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .map(|c| {
                    let name = c
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_owned();
                    ColumnInfo {
                        primary_key: pk.contains(&name),
                        type_name: c
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_owned(),
                        nullable: c
                            .get("nullable")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        unique: c
                            .get("unique")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        name,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    ModelInfo {
        table: entity.table_name().to_owned(),
        columns,
    }
}

/// The models an application declares, with their columns.
///
/// Built by hand rather than collected from a global, so the set is what the
/// application said it was.
#[derive(Debug, Clone, Default)]
pub struct ModelRegistry {
    entries: Vec<(&'static str, ModelInfo)>,
}

impl ModelRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one entity, under a name of the application's choosing.
    pub fn with<E: EntityTrait>(
        mut self,
        name: &'static str,
        entity: E,
        backend: DbBackend,
    ) -> Self {
        self.entries.push((name, describe(entity, backend)));
        self
    }

    /// Registered names, in insertion order.
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.iter().map(|(n, _)| *n).collect()
    }

    /// Registered tables, in insertion order.
    pub fn tables(&self) -> Vec<&str> {
        self.entries.iter().map(|(_, m)| m.table.as_str()).collect()
    }

    /// One model by registered name.
    pub fn get(&self, name: &str) -> Option<&ModelInfo> {
        self.entries
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, m)| m)
    }

    /// Every model.
    pub fn models(&self) -> impl Iterator<Item = (&'static str, &ModelInfo)> {
        self.entries.iter().map(|(n, m)| (*n, m))
    }

    /// Module paths from the SeaORM inventory that this registry does not
    /// mention. A non-empty answer means a model is linked but unregistered,
    /// which is what a generator wants to warn about.
    pub fn unregistered(&self) -> Vec<&'static str> {
        let known: Vec<&str> = self.names().into_iter().collect();
        modules()
            .into_iter()
            .filter(|m| {
                let leaf = m.rsplit("::").next().unwrap_or(m);
                !known.contains(&leaf)
            })
            .collect()
    }
}
