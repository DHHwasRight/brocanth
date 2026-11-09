//! Each scaffold manifest names exactly the hooks its plugin registers.
//!
//! A project's module file has one field per hook that a project may turn
//! off, and the generated `hooks_off_from` passes those names to the core.
//! A hook that the plugin registers and the manifest forgets has no field;
//! a name the manifest has and the plugin does not stops the project's
//! build. Each plugin here is configured so that every hook installs.

#![cfg(feature = "scaffold")]

use brocanth::plugin_api::Plugin;
use brocanth::plugin_hooks::{hook_names, HookSource};
use brocanth_analytics::{scaffold, Analytics, AnalyticsConfig, Beacon, BeaconConfig};
use brocanth_scaffold::Module;

fn declared(module: &dyn Module) -> Vec<&'static str> {
    let mut names: Vec<&str> = module
        .manifest()
        .hooks
        .iter()
        .map(|(name, _, _)| *name)
        .collect();
    names.sort_unstable();
    names
}

fn registered<P: HookSource>(plugin: &P) -> Vec<String> {
    let mut names = hook_names(plugin).expect("the plugin installs its hooks");
    names.sort_unstable();
    names
}

#[test]
fn analytics_declares_every_hook_it_registers() {
    let plugin = Analytics::new(AnalyticsConfig {
        enabled: true,
        collector_origins: vec!["https://example.com".to_string()],
        ..AnalyticsConfig::default()
    });
    assert_eq!(registered(&plugin), declared(&scaffold::PageViews));
}

#[test]
fn analytics_lists_its_hooks_for_a_config_that_installs_fewer() {
    // `HOOKS` is what lets a module file turn off `collect` on a server
    // whose config installs only `count`, so it must name the same hooks.
    let mut hooks = Analytics::HOOKS.to_vec();
    hooks.sort_unstable();
    assert_eq!(hooks, declared(&scaffold::PageViews));
}

#[test]
fn beacon_declares_every_hook_it_registers() {
    let plugin = Beacon::new(BeaconConfig {
        collector: Some("https://app.example.com/_a/v".to_string()),
        ..BeaconConfig::default()
    });
    assert_eq!(registered(&plugin), declared(&scaffold::StaticBeacon));
}
