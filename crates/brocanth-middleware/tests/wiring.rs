//! What a project has to write to install the CSRF hook.
//!
//! `plugin_hooking` widens the application's hook record, and an `App` is
//! a struct whose field needs that record spelled out. A generated project
//! spells it in `src/plugins/mod.rs`, so the spelling is checked here
//! rather than discovered by somebody whose first build failed on a type
//! they did not write.

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::{App, AppBuilder};
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{Both, HookSource, NoHooks};
use brocanth_middleware::{Csrf, CsrfConfig};

/// The record one hooking plugin produces, and nothing else registered.
type Hooks = Both<<Csrf as HookSource>::Families, NoHooks>;

#[test]
fn the_plugin_installs_and_its_hook_record_is_nameable() {
    // The annotation is the test: it fails to compile if the record this
    // plugin widens the builder with is not the one written above.
    let app: App<Base, Hooks> = AppBuilder::new()
        .plugin_hooking(Csrf::new(CsrfConfig::default()))
        .build()
        .expect("an application");
    assert!(app.plugin_named::<Csrf>("csrf").is_some());
}

#[test]
fn a_name_nothing_can_write_stops_the_build() {
    // `install_hooks` runs here, so a cookie name that cannot go in a
    // header is a failure to start rather than a request that fails later.
    let built = AppBuilder::new()
        .plugin_hooking(Csrf::new(CsrfConfig::default().cookie("a b")))
        .build();
    assert!(built.is_err(), "a cookie name with a space was accepted");
}
