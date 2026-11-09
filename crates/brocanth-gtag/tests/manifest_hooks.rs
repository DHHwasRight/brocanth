//! The scaffold manifest names exactly the hooks the plugin registers.
//!
//! A project's module file has one field per hook that a project may turn
//! off, and its header lists the others. A hook that the plugin registers
//! and the manifest forgets is in neither place. The plugin is configured
//! with a consent log, so that every hook installs.

#![cfg(feature = "scaffold")]

use brocanth::plugin_api::Plugin;
use brocanth::plugin_hooks::hook_names;
use brocanth_gtag::{scaffold, Gtag, GtagConfig};
use brocanth_scaffold::Module;

#[test]
fn gtag_declares_every_hook_it_registers() {
    let plugin = Gtag::new(GtagConfig {
        consent_log: Some("log/consent.jsonl".to_string()),
        ..GtagConfig::default()
    });
    let mut registered = hook_names(&plugin).expect("the plugin installs its hooks");
    registered.sort_unstable();
    let manifest = scaffold::Gtag.manifest();
    let mut declared: Vec<&str> = manifest.hooks.iter().map(|(name, _, _)| *name).collect();
    declared.sort_unstable();
    assert_eq!(registered, declared);
}
