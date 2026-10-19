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

// ---------------------------------------------------------------------------
// The routes
// ---------------------------------------------------------------------------

use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_hooks::{Request, RequestState};
use brocanth_admin::policy::{policy_fn, Principal};

/// What an application's auth middleware would have put in the request
/// state.
struct CurrentUser {
    staff: bool,
}

fn app_with_admin(principal_set: bool) -> brocanth::plugin_app::App<impl Sized, impl Sized> {
    let mut setup = AdminSetup::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(policy_fn(|who: &Principal<'_>, _| {
                who.downcast::<CurrentUser>().is_some_and(|u| u.staff)
            })),
    );
    if principal_set {
        setup = setup.principal(|_req: &Request, state: &RequestState| {
            state
                .get::<CurrentUser>()
                .map_or_else(Principal::anonymous, |u| Principal::new(u))
        });
    }
    AppBuilder::new()
        .plugin(AdminPlugin::new(setup))
        .build()
        .expect("an application")
}

#[test]
fn one_line_mounts_the_whole_admin() {
    let mut app = app_with_admin(true);
    let taken = app.mount_routes::<AdminPlugin>("/backoffice").unwrap();

    // Four, not twelve. The admin's path grammar is in `Route::parse`, and
    // a panel owns every path under its key to a depth nobody here knows,
    // so splitting that grammar across the bundle and the parser would be
    // two places to get it wrong.
    assert_eq!(taken, 4);
    assert_eq!(
        app.route_plan().named("admin.index").unwrap().path,
        "/backoffice"
    );
    assert_eq!(
        app.route_plan().named("admin.screen").unwrap().path,
        "/backoffice/{*rest}"
    );
}

#[test]
fn the_default_prefix_is_admin() {
    let mut app = app_with_admin(true);
    app.mount_routes_default::<AdminPlugin>().unwrap();
    assert_eq!(
        app.route_plan().named("admin.index").unwrap().path,
        "/admin"
    );
}

#[test]
fn an_admin_that_was_never_told_who_is_asking_is_anonymous() {
    // And the default policy denies anonymous, so the whole thing is shut
    // until an application wires both halves. A backoffice that opened
    // because somebody forgot a line is the failure worth designing out.
    let setup = AdminSetup::new(Arc::new(Fixture::blog()), AdminConfig::new());
    let request = Request::new(Vec::new());
    let state = RequestState::new();
    let who = (setup.principal)(&request, &state);
    assert!(who.is_anonymous(), "nobody, until the application says");
}

#[test]
fn the_principal_comes_out_of_the_request_state() {
    let setup = AdminSetup::new(Arc::new(Fixture::blog()), AdminConfig::new()).principal(
        |_req: &Request, state: &RequestState| {
            state
                .get::<CurrentUser>()
                .map_or_else(Principal::anonymous, |u| Principal::new(u))
        },
    );

    let mut state = RequestState::new();
    state.insert(CurrentUser { staff: true });
    let request = Request::new(Vec::new());

    let who = (setup.principal)(&request, &state);
    assert!(
        who.downcast::<CurrentUser>().is_some_and(|u| u.staff),
        "the value the application put there, unchanged"
    );
}
