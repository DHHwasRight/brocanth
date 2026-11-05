//! Written by `brocanth`. Do not edit -- the `migrate` tool rewrites this file.
//! A migration is a file beside this one; `migrate make` and `migrate sync` list it here.
//!
//! The migrations in this directory, oldest first.
//!
//! A file name starts with its UTC time, so the order of the names is the
//! order the migrations were written. `src/commands/migrate.rs` applies this
//! list, and `src/main.rs` reaches this file through `#[path]`.

use brocanth_model::sea_orm_migration::prelude::*;

/// Every migration this binary applies, oldest first.
pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![]
    }
}

// brocanth-digest: ce7abb4673e08591
