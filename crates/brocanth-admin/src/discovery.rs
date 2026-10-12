//! Finding the models the admin should show.
//!
//! PLAN.md §7 is the reason this is tractable: with SeaORM's `entity-registry`
//! feature every model in the binary is enumerable at runtime, and
//! `Schema::json_schema_from_entity` reports each entity's columns. So the
//! admin does not ask the application to list its models — it reads the list.
//!
//! Two things make the raw list unusable as it stands.
//!
//! The registry covers **every linked entity**, including SeaORM's own
//! migration table, `seaql_migrations`. An admin that offers to edit the
//! migration log is not a feature. So the migration tables are filtered out,
//! by name, always.
//!
//! And a library's models are registered too, because `inventory` collects
//! across the whole binary. The module-path prefix filter is how an
//! application says *these are mine*.

use crate::model::{ColumnDescriptor, ModelDescriptor};

/// Tables that keep a migration tool's bookkeeping, never a user's data.
///
/// `seaql_migrations` is SeaORM's and is the one that actually appears in a
/// brocanth binary. The rest are here because a database reached by this admin
/// may have been migrated by something else first, and a bookkeeping table is
/// never a thing to edit by hand whichever tool wrote it.
pub const MIGRATION_TABLES: &[&str] = &[
    "seaql_migrations",
    "schema_migrations",
    "_sqlx_migrations",
    "refinery_schema_history",
    "__diesel_schema_migrations",
];

/// Whether a table name is a migration tool's bookkeeping.
pub fn is_migration_table(table: &str) -> bool {
    MIGRATION_TABLES.contains(&table)
}

/// One entry as an entity registry reports it.
///
/// This is the whole of what [`discover`] needs, and it is plain data, so the
/// adapter in [`crate::adapter`] can fill it from SeaORM without this crate
/// depending on SeaORM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryEntry {
    /// Rust module path, as `inventory` reports it: `my_app::models::user`.
    pub module_path: String,
    /// Table name.
    pub table: String,
    /// Columns, in declaration order.
    pub columns: Vec<ColumnDescriptor>,
}

impl RegistryEntry {
    /// An entry with no columns.
    pub fn new(module_path: impl Into<String>, table: impl Into<String>) -> Self {
        RegistryEntry {
            module_path: module_path.into(),
            table: table.into(),
            columns: Vec::new(),
        }
    }

    /// Adds a column.
    pub fn with(mut self, column: ColumnDescriptor) -> Self {
        self.columns.push(column);
        self
    }

    /// The slug this entry is addressed by: the module path's last segment,
    /// or the table name when there is no module path.
    pub fn key(&self) -> String {
        match self.module_path.rsplit("::").next() {
            Some(leaf) if !leaf.is_empty() => leaf.to_string(),
            _ => self.table.clone(),
        }
    }
}

/// How a registry is narrowed to the models an admin shows.
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    prefix: Option<String>,
    excluded_tables: Vec<String>,
    excluded_keys: Vec<String>,
    keep_migration_tables: bool,
}

impl Discovery {
    /// Everything in the registry except the migration tables.
    pub fn new() -> Self {
        Discovery::default()
    }

    /// Only models whose module path starts with `prefix`.
    ///
    /// `"my_app::models"` matches `my_app::models::user`. This is the filter
    /// that keeps a dependency's entities out of your admin.
    pub fn under(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Some(prefix.into());
        self
    }

    /// Drops a table by name, on top of the migration tables.
    pub fn without_table(mut self, table: impl Into<String>) -> Self {
        self.excluded_tables.push(table.into());
        self
    }

    /// Drops a model by key, on top of the migration tables.
    pub fn without(mut self, key: impl Into<String>) -> Self {
        self.excluded_keys.push(key.into());
        self
    }

    /// Keeps the migration tables, for an admin whose whole job is to look at
    /// them. Off by default, and it has to be asked for in writing.
    pub fn keeping_migration_tables(mut self) -> Self {
        self.keep_migration_tables = true;
        self
    }

    /// Whether this entry survives the filter.
    pub fn admits(&self, entry: &RegistryEntry) -> bool {
        if !self.keep_migration_tables && is_migration_table(&entry.table) {
            return false;
        }
        if self.excluded_tables.contains(&entry.table) {
            return false;
        }
        if self.excluded_keys.contains(&entry.key()) {
            return false;
        }
        match &self.prefix {
            Some(prefix) => entry.module_path.starts_with(prefix.as_str()),
            None => true,
        }
    }
}

/// Turns a registry into model descriptors, sorted by key.
///
/// Duplicate keys are dropped, keeping the first: two entities under the same
/// leaf module name would otherwise claim the same URL, and a silently
/// shadowed model is worse than a missing one.
pub fn discover<I>(entries: I, filter: &Discovery) -> Vec<ModelDescriptor>
where
    I: IntoIterator<Item = RegistryEntry>,
{
    let mut out: Vec<ModelDescriptor> = Vec::new();
    for entry in entries {
        if !filter.admits(&entry) {
            continue;
        }
        let key = entry.key();
        if out.iter().any(|m| m.key == key) {
            continue;
        }
        out.push(ModelDescriptor {
            key,
            module_path: entry.module_path,
            table: entry.table,
            columns: entry.columns,
        });
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    out
}
