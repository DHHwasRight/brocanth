//! The brocanth plugin: built from a value the application constructs, and
//! nothing that happens behind its back.

#![cfg(feature = "plugin")]

use std::sync::Arc;

use brocanth::plugin_api::Plugin;
use brocanth_admin::{AdminConfig, AdminPlugin, AdminSetup, Discovery, Fixture};

#[test]
fn the_plugin_arrives_already_configured() {
    // §8: a plugin names the type it is built from and the application
    // constructs that value. The core parses nothing.
    let plugin = AdminPlugin::new(AdminSetup::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .at("/backoffice")
            .discovering(Discovery::new().under("blog::models")),
    ));

    let admin = plugin.admin();
    assert_eq!(admin.model_keys(), ["post", "user"]);
    assert_eq!(admin.base_path(), "/backoffice");
    assert_eq!(AdminPlugin::NAME, "admin");
}

#[test]
fn discovery_runs_once_when_the_plugin_is_built() {
    // The entity registry is read out of the binary, so it cannot change while
    // the process runs. Reading it per request would buy nothing.
    let plugin = AdminPlugin::new(AdminSetup::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new(),
    ));
    let first = plugin.admin();
    let second = plugin.admin();

    assert!(Arc::ptr_eq(&first, &second), "one admin, shared");
}
