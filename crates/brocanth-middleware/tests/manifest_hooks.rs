//! Each scaffold manifest names exactly the hooks its plugin registers.
//!
//! A project's module file has one field per hook that a project may turn
//! off, and the generated `hooks_off_from` passes those names to the core.
//! A hook that the plugin registers and the manifest forgets has no field;
//! a name the manifest has and the plugin does not stops the project's
//! build. This test finds both here, in this crate.

#![cfg(feature = "scaffold")]

use brocanth::plugin_api::Plugin;
use brocanth::plugin_hooks::{hook_names, HookSource};
use brocanth_middleware::scaffold;
use brocanth_middleware::{
    Csrf, CsrfConfig, RateLimit, RateLimitConfig, SecurityHeaders, SecurityHeadersConfig,
};
use brocanth_scaffold::Module;

fn assert_same<P: HookSource>(module: &dyn Module, plugin: &P) {
    let manifest = module.manifest();
    let mut declared: Vec<&str> = manifest.hooks.iter().map(|(name, _, _)| *name).collect();
    let mut registered = hook_names(plugin).expect("the plugin installs its hooks");
    declared.sort_unstable();
    registered.sort_unstable();
    assert_eq!(
        registered, declared,
        "`{}`: the plugin registers {registered:?}, the manifest declares {declared:?}",
        manifest.key
    );
}

#[test]
fn csrf_declares_every_hook_it_registers() {
    assert_same(&scaffold::Csrf, &Csrf::new(CsrfConfig::default()));
}

#[test]
fn security_headers_declares_every_hook_they_register() {
    assert_same(
        &scaffold::SecurityHeaders,
        &SecurityHeaders::new(SecurityHeadersConfig::default()),
    );
}

#[test]
fn rate_limit_declares_every_hook_it_registers() {
    assert_same(
        &scaffold::RateLimit,
        &RateLimit::new(RateLimitConfig::default()),
    );
}
