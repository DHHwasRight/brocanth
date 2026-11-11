//! The SeaORM migration `rls make` writes in a project with `model`
//! compiles against the model layer, and names itself after its file.

use brocanth_model::sea_orm_migration::prelude::*;

#[path = "golden/m20261111_000000_rls.rs"]
mod m20261111_000000_rls;

#[test]
fn the_seaorm_migration_compiles_and_names_itself() {
    let migration: Box<dyn MigrationTrait> = Box::new(m20261111_000000_rls::Migration);
    assert_eq!(migration.name(), "m20261111_000000_rls");
}
