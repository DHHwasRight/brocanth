//! `AppBuilder::hooks_off`: a hook that the application turns off.
//!
//! The contract is docs/module-config.md §2.3. An off hook never runs, the
//! pipeline lists it, a name that the plugin never registers stops the build,
//! and the hook is absent for ordering: `after_if_present` on it does
//! nothing, and a hard `after` on it stops the build with a `help:` line.

use std::sync::{Arc, Mutex};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::{AppBuilder, BuildError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    Flow, HandlerError, HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState,
    Response,
};
use brocanth::plugin_order::Order;

#[derive(Clone, Default)]
struct Rec(Arc<Mutex<Vec<String>>>);

impl Rec {
    fn push(&self, line: impl Into<String>) {
        self.0.lock().unwrap().push(line.into());
    }
    fn take(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

struct Step {
    rec: Rec,
    id: &'static str,
}

impl RequestHook for Step {
    fn before_dispatch(
        &self,
        _request: &mut Request,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        self.rec.push(self.id);
        Ok(Flow::Continue)
    }
}

/// Instance `rate_limit`, with the hooks `count`, `headers` and
/// `consent-log`.
struct RateLimit(Rec);

impl Plugin for RateLimit {
    type Config = Rec;
    type Provides = NoContext;
    const NAME: &'static str = "rate_limit";
    const INSTALLS_HOOKS: bool = true;

    fn new(rec: Rec) -> Self {
        Self(rec)
    }
}

impl HookSource for RateLimit {
    type Families = OnRequest;

    fn install_hooks(&self, r: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        for (name, id) in [
            ("count", "rate_limit.count"),
            ("headers", "rate_limit.headers"),
            ("consent-log", "rate_limit.consent-log"),
        ] {
            r.request(
                name,
                Order::new(),
                Step {
                    rec: self.0.clone(),
                    id,
                },
            );
        }
        Ok(())
    }
}

/// Instance `audit`, with one hook, `trail`, in the order its config gives.
struct Audit(Rec, Order);

impl Plugin for Audit {
    type Config = (Rec, Order);
    type Provides = NoContext;
    const NAME: &'static str = "audit";
    const INSTALLS_HOOKS: bool = true;

    fn new((rec, order): (Rec, Order)) -> Self {
        Self(rec, order)
    }
}

impl HookSource for Audit {
    type Families = OnRequest;

    fn install_hooks(&self, r: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        r.request(
            "trail",
            self.1.clone(),
            Step {
                rec: self.0.clone(),
                id: "audit.trail",
            },
        );
        Ok(())
    }
}

fn ok_handler(_rq: &mut Request, _st: &mut RequestState) -> Result<Response, HandlerError> {
    Ok(Response::new(Body::from("ok")))
}

fn run<E, H: brocanth::plugin_hooks::HookFamilies>(app: &brocanth::plugin_app::App<E, H>) {
    app.run(
        &mut Request::new(Vec::new()),
        &mut RequestState::new(),
        ok_handler,
    );
}

#[test]
fn an_off_hook_never_runs_and_the_others_still_do() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(RateLimit::new(rec.clone()))
        .hooks_off("rate_limit", ["headers"])
        .build()
        .unwrap();

    run(&app);

    assert_eq!(
        rec.take(),
        vec!["rate_limit.count", "rate_limit.consent-log"]
    );
    assert_eq!(app.pipeline().request_hooks(), 2);
}

#[test]
fn skipped_lists_each_off_hook_in_registration_order() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(RateLimit::new(rec.clone()))
        // Two calls add up, and the order of the list does not matter.
        .hooks_off("rate_limit", ["consent-log"])
        .hooks_off("rate_limit", ["count"])
        .build()
        .unwrap();

    assert_eq!(
        app.pipeline().skipped(),
        &[("rate_limit", "count"), ("rate_limit", "consent-log")]
    );
    run(&app);
    assert_eq!(rec.take(), vec!["rate_limit.headers"]);
}

#[test]
fn an_empty_list_changes_nothing() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(RateLimit::new(rec.clone()))
        .hooks_off("rate_limit", Vec::new())
        .build()
        .unwrap();

    assert!(app.pipeline().skipped().is_empty());
    assert_eq!(app.pipeline().request_hooks(), 3);
}

#[test]
fn a_name_the_plugin_never_registers_stops_the_build() {
    let err = AppBuilder::new()
        .plugin_hooking(RateLimit::new(Rec::default()))
        .hooks_off("rate_limit", ["haeders"])
        .build()
        .unwrap_err();

    assert!(
        matches!(err, BuildError::HooksOffUnknownHook { .. }),
        "{err:?}"
    );
    let text = err.to_string();
    assert!(
        text.starts_with("rate_limit has no hook \"haeders\"\n"),
        "{text}"
    );
    assert!(text.contains("help: did you mean \"headers\"?"), "{text}");
    assert!(text.contains("count, headers, consent-log"), "{text}");
}

#[test]
fn an_instance_nobody_registered_stops_the_build() {
    let err = AppBuilder::new()
        .plugin_hooking(RateLimit::new(Rec::default()))
        .hooks_off("rate_limt", ["headers"])
        .build()
        .unwrap_err();

    assert!(
        matches!(err, BuildError::HooksOffUnknownInstance { .. }),
        "{err:?}"
    );
    let text = err.to_string();
    assert!(text.contains("help: did you mean `rate_limit`?"), "{text}");
}

#[test]
fn after_if_present_on_an_off_hook_does_nothing() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Audit::new((
            rec.clone(),
            Order::new().after_if_present("rate_limit.headers"),
        )))
        .plugin_hooking(RateLimit::new(rec.clone()))
        .hooks_off("rate_limit", ["headers"])
        .build()
        .unwrap();

    run(&app);

    // Registration order: nothing ties `audit.trail` to the others now.
    assert_eq!(
        rec.take(),
        vec!["audit.trail", "rate_limit.count", "rate_limit.consent-log"]
    );
}

#[test]
fn a_hard_after_on_an_off_hook_names_the_module_file_and_the_field() {
    let err = AppBuilder::new()
        .plugin_hooking(RateLimit::new(Rec::default()))
        .hooks_off_from(
            "rate_limit",
            "src/modules/middleware_rate_limit.rs",
            ["headers"],
        )
        .plugin_hooking(Audit::new((
            Rec::default(),
            Order::new().after("rate_limit.headers"),
        )))
        .build()
        .unwrap_err();

    assert!(matches!(err, BuildError::OrderOnOffHook { .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        "`audit.trail` runs after `rate_limit.headers`, which \
         src/modules/middleware_rate_limit.rs turns off\n\
         help: set `headers: true` in src/modules/middleware_rate_limit.rs, or change the \
         order of `audit.trail`"
    );
}

#[test]
fn a_hook_name_with_a_dash_names_the_field_with_an_underscore() {
    let err = AppBuilder::new()
        .plugin_hooking(RateLimit::new(Rec::default()))
        .hooks_off_from("rate_limit", "src/modules/gtag.rs", ["consent-log"])
        .plugin_hooking(Audit::new((
            Rec::default(),
            Order::new().before("rate_limit.consent-log"),
        )))
        .build()
        .unwrap_err();

    let text = err.to_string();
    assert!(
        text.contains("runs before `rate_limit.consent-log`"),
        "{text}"
    );
    assert!(text.contains("set `consent_log: true`"), "{text}");
}

#[test]
fn a_hard_after_on_an_off_hook_without_a_file_names_the_call() {
    let err = AppBuilder::new()
        .plugin_hooking(RateLimit::new(Rec::default()))
        .hooks_off("rate_limit", ["headers"])
        .plugin_hooking(Audit::new((
            Rec::default(),
            Order::new().after("rate_limit.headers"),
        )))
        .build()
        .unwrap_err();

    let text = err.to_string();
    assert!(
        text.contains("which `hooks_off(\"rate_limit\", ..)` turns off\n"),
        "{text}"
    );
    assert!(
        text.contains("help: remove \"headers\" from `hooks_off(\"rate_limit\", ..)`"),
        "{text}"
    );
}

#[test]
fn a_hard_after_on_an_instance_whose_hooks_are_all_off_is_refused() {
    // One hook in the chain: `sort` alone checks nothing at that size, so
    // this proves the check runs before it.
    let err = AppBuilder::new()
        .plugin_hooking(RateLimit::new(Rec::default()))
        .hooks_off("rate_limit", ["count", "headers", "consent-log"])
        .plugin_hooking(Audit::new((
            Rec::default(),
            Order::new().after("rate_limit"),
        )))
        .build()
        .unwrap_err();

    assert!(matches!(err, BuildError::OrderOnOffHook { .. }), "{err:?}");
}

#[test]
fn a_hard_after_on_an_instance_with_a_hook_left_on_still_resolves() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Audit::new((rec.clone(), Order::new().after("rate_limit"))))
        .plugin_hooking(RateLimit::new(rec.clone()))
        .hooks_off("rate_limit", ["headers"])
        .build()
        .unwrap();

    run(&app);
    assert_eq!(
        rec.take(),
        vec!["rate_limit.count", "rate_limit.consent-log", "audit.trail"]
    );
}
