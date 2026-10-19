//! Discovery: the registry is not the model list until the filter has run.

use brocanth_admin::discovery::{discover, is_migration_table, Discovery, RegistryEntry};
use brocanth_admin::{Admin, AdminConfig, ColumnDescriptor, ColumnKind, Fixture};
use std::sync::Arc;

fn registry() -> Vec<RegistryEntry> {
    vec![
        RegistryEntry::new("blog::models::user", "user")
            .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key()),
        RegistryEntry::new("blog::models::post", "post")
            .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key()),
        // SeaORM's own, which `entity-registry` reports like any other entity.
        RegistryEntry::new("sea_orm_migration::seaql_migrations", "seaql_migrations")
            .with(ColumnDescriptor::new("version", ColumnKind::Text).primary_key()),
        // A dependency's entity, linked into the same binary.
        RegistryEntry::new("some_crate::models::session", "session")
            .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key()),
    ]
}

#[test]
fn the_migration_table_is_never_a_model() {
    let found = discover(registry(), &Discovery::new());
    let tables: Vec<&str> = found.iter().map(|m| m.table.as_str()).collect();

    assert!(!tables.contains(&"seaql_migrations"), "got {tables:?}");
    assert!(tables.contains(&"user"));
    assert!(tables.contains(&"post"));
}

#[test]
fn the_prefix_keeps_another_crates_entities_out() {
    let found = discover(registry(), &Discovery::new().under("blog::models"));
    let keys: Vec<&str> = found.iter().map(|m| m.key.as_str()).collect();

    assert_eq!(
        keys,
        ["post", "user"],
        "sorted, and only this application's"
    );
}

#[test]
fn the_migration_table_is_dropped_even_under_its_own_prefix() {
    // The prefix filter alone is not enough: point it at the migration crate
    // and the name filter still has to hold.
    let found = discover(registry(), &Discovery::new().under("sea_orm_migration"));
    assert!(found.is_empty(), "got {found:?}");
}

#[test]
fn keeping_the_migration_tables_has_to_be_asked_for() {
    let found = discover(registry(), &Discovery::new().keeping_migration_tables());
    let tables: Vec<&str> = found.iter().map(|m| m.table.as_str()).collect();
    assert!(tables.contains(&"seaql_migrations"));
}

#[test]
fn every_known_migration_table_is_recognised() {
    assert!(is_migration_table("seaql_migrations"));
    assert!(is_migration_table("_sqlx_migrations"));
    assert!(!is_migration_table("user"));
}

#[test]
fn a_hidden_model_is_not_discovered_by_the_admin() {
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .model("user", |m| m.hidden()),
    );
    assert_eq!(admin.model_keys(), ["post"]);
}

#[test]
fn the_admin_filters_the_migration_table_out_of_a_real_registry() {
    let admin = Admin::new(Arc::new(Fixture::blog()), AdminConfig::new());
    // No prefix at all, so only the name filter is doing the work.
    assert_eq!(admin.model_keys(), ["post", "user"]);
}
