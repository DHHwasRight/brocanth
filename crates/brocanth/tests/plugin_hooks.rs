//! The extension system, tested at the seam rather than at the implementation.
//!
//! Each test below is one sentence from the contract in `lib.rs`: every hook
//! fires at the stated moment, order is deterministic and respects what was
//! declared, a cycle names both ends, an error stops what it should and not
//! more, and two applications in one process still do not interfere.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_app::{AppBuilder, BuildError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    Both, ErrorHook, Flow, HandlerError, HookRegistrar, HookSource, NoHooks, OnError, OnModel,
    OnRequest, OnResponse, Request, RequestHook, RequestState, Response, ResponseHook,
};
use brocanth::plugin_model::{ModelEvent, ModelHook};
use brocanth::plugin_order::Order;
use brocanth::plugin_routes::{Handler, RouteBundle, RouteCx, RouteError, RouteSource};
use brocanth::plugin_shutdown::{ShutdownCx, ShutdownPhase, ShutdownReason};
use brocanth::routing::Verb;

// ---------------------------------------------------------------------------
// A recorder the test owns. Never a global, so the last test in this file is
// the one that proves it.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// A plugin that hooks the request and the response
// ---------------------------------------------------------------------------

struct TraceConfig {
    rec: Rec,
    tag: &'static str,
    order: Order,
    /// Short-circuit in `before_dispatch`.
    stop: bool,
    /// Fail in `before_dispatch`.
    fail_before: bool,
    /// Fail in `on_response`.
    fail_response: bool,
}

impl TraceConfig {
    fn new(rec: &Rec, tag: &'static str) -> Self {
        Self {
            rec: rec.clone(),
            tag,
            order: Order::new(),
            stop: false,
            fail_before: false,
            fail_response: false,
        }
    }
    fn ordered(mut self, order: Order) -> Self {
        self.order = order;
        self
    }
}

struct Trace {
    rec: Rec,
    tag: &'static str,
    order: Order,
    stop: bool,
    fail_before: bool,
    fail_response: bool,
}

impl Plugin for Trace {
    type Config = TraceConfig;
    type Provides = NoContext;
    const NAME: &'static str = "trace";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self {
            rec: config.rec,
            tag: config.tag,
            order: config.order,
            stop: config.stop,
            fail_before: config.fail_before,
            fail_response: config.fail_response,
        }
    }
}

struct TraceHook {
    rec: Rec,
    tag: &'static str,
    stop: bool,
    fail_before: bool,
    fail_response: bool,
}

impl RequestHook for TraceHook {
    fn before_dispatch(
        &self,
        _request: &mut Request,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        self.rec.push(format!("{}:before", self.tag));
        if self.fail_before {
            return Err(PluginError::new(format!("{} refused", self.tag)));
        }
        if self.stop {
            let mut short = Response::new(b"short".to_vec());
            *short.status_mut() = http::StatusCode::ACCEPTED;
            return Ok(Flow::respond(short));
        }
        Ok(Flow::Continue)
    }

    fn after_dispatch(&self, _request: &Request, response: &Response, _state: &mut RequestState) {
        self.rec
            .push(format!("{}:after({})", self.tag, response.status().as_u16()));
    }
}

impl ResponseHook for TraceHook {
    fn on_response(
        &self,
        _request: &Request,
        response: &mut Response,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        self.rec.push(format!("{}:response", self.tag));
        if self.fail_response {
            return Err(PluginError::new(format!("{} broke the response", self.tag)));
        }
        response.body_mut().extend_from_slice(self.tag.as_bytes());
        Ok(())
    }
}

impl HookSource for Trace {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        let hook = || TraceHook {
            rec: self.rec.clone(),
            tag: self.tag,
            stop: self.stop,
            fail_before: self.fail_before,
            fail_response: self.fail_response,
        };
        registrar.request("trace", self.order.clone(), hook());
        registrar.response("rewrite", self.order.clone(), hook());
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// A plugin that turns its own error into a response
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct Denied(&'static str);

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "denied: {}", self.0)
    }
}
impl std::error::Error for Denied {}

struct Auth {
    rec: Rec,
    status: http::StatusCode,
}

impl Plugin for Auth {
    type Config = (Rec, http::StatusCode);
    type Provides = NoContext;
    const NAME: &'static str = "auth";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self {
            rec: config.0,
            status: config.1,
        }
    }
}

struct AuthErrors {
    rec: Rec,
    status: http::StatusCode,
    name: &'static str,
}

impl ErrorHook for AuthErrors {
    fn on_error(
        &self,
        error: &HandlerError,
        _request: &Request,
        _state: &mut RequestState,
    ) -> Option<Response> {
        self.rec.push(format!("{}:asked", self.name));
        // The core has never heard of `Denied`. This hook has.
        error.downcast_ref::<Denied>()?;
        let mut response = Response::new(self.name.as_bytes().to_vec());
        *response.status_mut() = self.status;
        Some(response)
    }
}

impl HookSource for Auth {
    type Families = OnError;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.error(
            "render",
            Order::new(),
            AuthErrors {
                rec: self.rec.clone(),
                status: self.status,
                name: "auth",
            },
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// A plugin with twelve screens
// ---------------------------------------------------------------------------

struct Admin {
    screens: usize,
}

impl Plugin for Admin {
    type Config = usize;
    type Provides = NoContext;
    const NAME: &'static str = "admin";
    const CONTRIBUTES_ROUTES: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { screens: config }
    }
}

impl<E> RouteSource<E> for Admin {
    const PREFIX: &'static str = "/admin";

    fn routes(&self, bundle: &mut RouteBundle<'_, E>) -> Result<(), PluginError> {
        bundle.get("dashboard", "/", screen("dashboard"));
        for n in 1..self.screens {
            let name = format!("screen{n}");
            bundle.get(&name, &format!("/{name}"), screen("screen"));
        }
        bundle.post("destroy", "/destroy", screen("destroy"));
        Ok(())
    }
}

fn screen<E>(body: &'static str) -> Handler<E> {
    Box::new(move |_cx: &mut RouteCx<'_, E>| Ok(Response::new(body.as_bytes().to_vec())))
}

// ---------------------------------------------------------------------------
// A plugin that drains
// ---------------------------------------------------------------------------

struct Pool {
    rec: Rec,
    name: &'static str,
    fail_drain: bool,
}

impl Plugin for Pool {
    type Config = (Rec, &'static str, bool);
    type Provides = NoContext;
    const NAME: &'static str = "pool";

    fn new(config: Self::Config) -> Self {
        Self {
            rec: config.0,
            name: config.1,
            fail_drain: config.2,
        }
    }

    fn start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.rec.push(format!("{}:start({})", self.name, lc.stage()));
        Ok(())
    }

    fn begin_shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        self.rec.push(format!("{}:{}", self.name, sd.phase()));
        Ok(())
    }

    fn shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        self.rec.push(format!(
            "{}:{}(grace={})",
            self.name,
            sd.phase(),
            sd.remaining().is_some()
        ));
        if self.fail_drain {
            return Err(PluginError::new("the pool would not close"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// A plugin that hooks an ORM's event, in a core that has never heard of an ORM
// ---------------------------------------------------------------------------

/// The ORM's event kind, declared in the ORM's crate, not here.
struct BeforeSave;
struct Writing<'a> {
    table: &'static str,
    audit: &'a mut Vec<String>,
}
impl ModelEvent for BeforeSave {
    type Payload<'a> = Writing<'a>;
}

struct Auditor {
    tag: &'static str,
}

impl Plugin for Auditor {
    type Config = &'static str;
    type Provides = NoContext;
    const NAME: &'static str = "auditor";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { tag: config }
    }
}

struct AuditHook(&'static str);

impl ModelHook<BeforeSave> for AuditHook {
    fn on_event(&self, event: &mut Writing<'_>) -> Result<(), PluginError> {
        event.audit.push(format!("{}:{}", self.0, event.table));
        Ok(())
    }
}

impl HookSource for Auditor {
    type Families = OnModel;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.model::<BeforeSave>("trail", Order::new(), AuditHook(self.tag));
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn request() -> Request {
    Request::new(Vec::new())
}

fn ok_handler(_rq: &mut Request, _st: &mut RequestState) -> Result<Response, HandlerError> {
    Ok(Response::new(b"body:".to_vec()))
}

// ---------------------------------------------------------------------------
// Firing points
// ---------------------------------------------------------------------------

#[test]
fn every_hook_fires_at_the_stated_moment() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Trace::new(TraceConfig::new(&rec, "t")))
        .build()
        .unwrap();

    let response = app.run(&mut request(), &mut RequestState::new(), |rq, st| {
        rec.push("handler");
        ok_handler(rq, st)
    });

    assert_eq!(
        rec.take(),
        vec!["t:before", "handler", "t:response", "t:after(200)"]
    );
    // The response hook really did rewrite the body.
    assert_eq!(response.body(), b"body:t");
}

#[test]
fn a_short_circuit_skips_the_handler_and_nothing_else() {
    let rec = Rec::default();
    let mut config = TraceConfig::new(&rec, "t");
    config.stop = true;
    let app = AppBuilder::new()
        .plugin_hooking(Trace::new(config))
        .build()
        .unwrap();

    let response = app.run(&mut request(), &mut RequestState::new(), |rq, st| {
        rec.push("handler");
        ok_handler(rq, st)
    });

    assert_eq!(rec.take(), vec!["t:before", "t:response", "t:after(202)"]);
    assert_eq!(response.status(), http::StatusCode::ACCEPTED);
    assert_eq!(response.body(), b"shortt");
}

// ---------------------------------------------------------------------------
// Order
// ---------------------------------------------------------------------------

#[test]
fn declared_order_beats_registration_order_and_response_hooks_run_inside_out() {
    let rec = Rec::default();
    // `trace` is registered first and declares that it runs after `zeta`.
    let app = AppBuilder::new()
        .plugin_hooking(Trace::new(
            TraceConfig::new(&rec, "trace").ordered(Order::new().after("zeta")),
        ))
        .plugin_hooking_as("zeta", Trace::new(TraceConfig::new(&rec, "zeta")))
        .build()
        .unwrap();

    app.run(&mut request(), &mut RequestState::new(), ok_handler);

    assert_eq!(
        rec.take(),
        vec![
            // before_dispatch: declared order
            "zeta:before",
            "trace:before",
            // on_response: reverse, so the one declared last is outermost
            "trace:response",
            "zeta:response",
            // after_dispatch: reverse too
            "trace:after(200)",
            "zeta:after(200)",
        ]
    );
}

#[test]
fn the_same_registrations_give_the_same_order_every_time() {
    for _ in 0..8 {
        let rec = Rec::default();
        let app = AppBuilder::new()
            .plugin_hooking(Trace::new(TraceConfig::new(&rec, "a")))
            .plugin_hooking_as("b", Trace::new(TraceConfig::new(&rec, "b")))
            .plugin_hooking_as("c", Trace::new(TraceConfig::new(&rec, "c")))
            .build()
            .unwrap();
        app.run(&mut request(), &mut RequestState::new(), ok_handler);
        assert_eq!(
            rec.take()[..3],
            ["a:before".to_string(), "b:before".into(), "c:before".into()]
        );
    }
}

#[test]
fn a_cycle_in_the_declarations_is_a_build_error_naming_both() {
    let rec = Rec::default();
    let err = AppBuilder::new()
        .plugin_hooking(Trace::new(
            TraceConfig::new(&rec, "a").ordered(Order::new().after("zeta")),
        ))
        .plugin_hooking_as(
            "zeta",
            Trace::new(TraceConfig::new(&rec, "b").ordered(Order::new().after("trace"))),
        )
        .build()
        .unwrap_err();

    assert!(matches!(err, BuildError::Order(_)), "{err:?}");
    let text = err.to_string();
    assert!(text.contains("trace.trace"), "{text}");
    assert!(text.contains("zeta.trace"), "{text}");
}

#[test]
fn ordering_after_a_hook_nobody_registered_names_it_and_suggests_the_near_one() {
    let rec = Rec::default();
    let err = AppBuilder::new()
        .plugin_hooking(Trace::new(
            TraceConfig::new(&rec, "a").ordered(Order::new().after("zeat")),
        ))
        .plugin_hooking_as("zeta", Trace::new(TraceConfig::new(&rec, "b")))
        .build()
        .unwrap_err();

    let text = err.to_string();
    assert!(text.contains("zeat"), "{text}");
    assert!(text.contains("did you mean `zeta`"), "{text}");
}

#[test]
fn an_optional_dependency_on_an_absent_plugin_is_silence() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Trace::new(
            TraceConfig::new(&rec, "a").ordered(Order::new().after_if_present("session")),
        ))
        .build()
        .unwrap();
    app.run(&mut request(), &mut RequestState::new(), ok_handler);
    assert_eq!(rec.take()[0], "a:before");
}

// ---------------------------------------------------------------------------
// Registering a hooking plugin the wrong way
// ---------------------------------------------------------------------------

#[test]
fn a_hooking_plugin_registered_with_plugin_fails_the_build_and_names_the_call_to_use() {
    let rec = Rec::default();
    let err = AppBuilder::new()
        .plugin(Trace::new(TraceConfig::new(&rec, "t")))
        .build()
        .unwrap_err();

    assert!(matches!(err, BuildError::HooksNotInstalled { .. }), "{err:?}");
    let text = err.to_string();
    assert!(text.contains("plugin_hooking"), "{text}");
    assert!(text.contains("`trace`"), "{text}");
}

#[test]
fn a_hooking_plugin_that_forgets_the_declaration_fails_the_build_too() {
    struct Quiet;
    impl Plugin for Quiet {
        type Config = ();
        type Provides = NoContext;
        const NAME: &'static str = "quiet";
        // INSTALLS_HOOKS left at its default of `false`, which is the bug.
        fn new(_: ()) -> Self {
            Self
        }
    }
    impl HookSource for Quiet {
        type Families = OnRequest;
        fn install_hooks(&self, _r: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
            Ok(())
        }
    }

    let err = AppBuilder::new().plugin_hooking(Quiet).build().unwrap_err();
    assert!(matches!(err, BuildError::HooksNotDeclared { .. }), "{err:?}");
    assert!(err.to_string().contains("INSTALLS_HOOKS"), "{err}");
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[test]
fn an_error_hook_renders_an_error_the_core_has_never_heard_of() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Auth::new((rec.clone(), http::StatusCode::FOUND)))
        .build()
        .unwrap();

    let response = app.run(&mut request(), &mut RequestState::new(), |_, _| {
        Err(HandlerError::new(Denied("no session")))
    });

    assert_eq!(response.status(), http::StatusCode::FOUND);
    assert_eq!(response.body(), b"auth");
    assert_eq!(rec.take(), vec!["auth:asked"]);
}

#[test]
fn an_error_nobody_claims_is_a_bare_500() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Auth::new((rec.clone(), http::StatusCode::FOUND)))
        .build()
        .unwrap();

    let response = app.run(&mut request(), &mut RequestState::new(), |_, _| {
        Err(HandlerError::message("something else entirely"))
    });

    // The hook was asked, declined, and the core answered the only way it can.
    assert_eq!(rec.take(), vec!["auth:asked"]);
    assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
    assert!(response.body().is_empty());
}

#[test]
fn a_failing_request_hook_skips_the_handler_and_goes_to_the_error_hooks() {
    let rec = Rec::default();
    let mut config = TraceConfig::new(&rec, "t");
    config.fail_before = true;
    let app = AppBuilder::new()
        .plugin_hooking(Trace::new(config))
        .plugin_hooking(Auth::new((rec.clone(), http::StatusCode::FOUND)))
        .build()
        .unwrap();

    let response = app.run(&mut request(), &mut RequestState::new(), |rq, st| {
        rec.push("handler");
        ok_handler(rq, st)
    });

    // The handler never ran; the error hook was asked and declined (the error
    // is a `PluginError`, not a `Denied`); the response hook and
    // `after_dispatch` still ran, because a logger logs failures too.
    assert_eq!(
        rec.take(),
        vec!["t:before", "auth:asked", "t:response", "t:after(500)"]
    );
    assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
}

#[test]
fn a_failing_response_hook_stops_the_remaining_response_hooks_and_not_after_dispatch() {
    let rec = Rec::default();
    let mut broken = TraceConfig::new(&rec, "broken");
    broken.fail_response = true;
    let app = AppBuilder::new()
        // `inner` is declared first, so it is the innermost and runs last.
        .plugin_hooking(Trace::new(TraceConfig::new(&rec, "inner")))
        .plugin_hooking_as("outer", Trace::new(broken))
        .build()
        .unwrap();

    let response = app.run(&mut request(), &mut RequestState::new(), ok_handler);

    assert_eq!(
        rec.take(),
        vec![
            "inner:before",
            "broken:before",
            // reverse order: the outer one runs first and fails
            "broken:response",
            // "inner:response" never happens
            "broken:after(500)",
            "inner:after(500)",
        ]
    );
    assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[test]
fn an_application_mounts_twelve_screens_in_one_line_at_a_prefix_it_chooses() {
    let mut app = AppBuilder::new().plugin(Admin::new(12)).build().unwrap();

    let taken = app.mount_routes::<Admin>("/backoffice").unwrap();

    assert_eq!(taken, 13); // twelve screens plus the destroy action
    assert_eq!(app.route_plan().len(), 13);
    assert_eq!(
        app.route_plan().named("admin.dashboard").unwrap().path,
        "/backoffice"
    );
    assert_eq!(
        app.route_plan().named("admin.screen1").unwrap().path,
        "/backoffice/screen1"
    );
    assert_eq!(
        app.route_plan().named("admin.destroy").unwrap().verb,
        Verb::Post
    );
}

#[test]
fn the_plugins_own_prefix_is_a_suggestion_the_application_may_take() {
    let mut app = AppBuilder::new().plugin(Admin::new(2)).build().unwrap();
    app.mount_routes_default::<Admin>().unwrap();
    assert_eq!(
        app.route_plan().named("admin.dashboard").unwrap().path,
        "/admin"
    );
}

#[test]
fn a_filter_takes_a_subset_without_the_plugins_cooperation() {
    let mut app = AppBuilder::new().plugin(Admin::new(3)).build().unwrap();
    app.mount_routes_where::<Admin>("/admin", |name| name != "admin.destroy")
        .unwrap();
    assert_eq!(app.route_plan().len(), 3);
    assert!(app.route_plan().named("admin.destroy").is_none());
}

#[test]
fn two_bundles_on_one_path_is_an_error_naming_both() {
    let mut app = AppBuilder::new()
        .plugin(Admin::new(2))
        .plugin_as("shadow", Admin::new(2))
        .build()
        .unwrap();

    app.mount_routes::<Admin>("/admin").unwrap();
    let err = app.mount_routes_of::<Admin>("shadow", "/admin").unwrap_err();

    assert!(matches!(err, RouteError::Conflict { .. }), "{err:?}");
    let text = err.to_string();
    assert!(text.contains("admin"), "{text}");
    assert!(text.contains("shadow"), "{text}");
    // The rejected mount left the plan exactly as it was.
    assert_eq!(app.route_plan().len(), 3);
}

#[test]
fn a_route_source_that_was_never_mounted_is_listed_rather_than_lost() {
    let app = AppBuilder::new().plugin(Admin::new(2)).build().unwrap();
    assert_eq!(app.unmounted_route_sources(), vec!["admin"]);

    let mut app = app;
    app.mount_routes::<Admin>("/admin").unwrap();
    assert!(app.unmounted_route_sources().is_empty());
}

#[test]
fn mounting_a_plugin_that_was_never_registered_names_it() {
    let mut app = AppBuilder::new().build().unwrap();
    let err = app.mount_routes::<Admin>("/admin").unwrap_err();
    assert!(matches!(err, RouteError::NoSuchPlugin { .. }), "{err:?}");
}

#[test]
fn a_contributed_handler_runs_and_sees_the_context() {
    let mut app = AppBuilder::new().plugin(Admin::new(2)).build().unwrap();
    app.mount_routes::<Admin>("/admin").unwrap();

    let context = app.context();
    let request = Request::new(Vec::new());
    let params = brocanth::routing::Params::empty();
    let mut state = RequestState::new();
    let mut cx = RouteCx::new(&request, &params, &context, &mut state);

    let route = app.route_plan().named("admin.dashboard").unwrap();
    let response = (route.handler)(&mut cx).unwrap();
    assert_eq!(response.body(), b"dashboard");
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[test]
fn an_orm_emits_through_a_core_that_has_never_heard_of_an_orm() {
    let app = AppBuilder::new()
        .plugin_hooking(Auditor::new("first"))
        .plugin_hooking_as("second", Auditor::new("second"))
        .build()
        .unwrap();

    let dispatch = app.model_dispatch();
    let mut audit = Vec::new();
    dispatch
        .emit::<BeforeSave>(&mut Writing {
            table: "posts",
            audit: &mut audit,
        })
        .unwrap();

    assert_eq!(audit, vec!["first:posts", "second:posts"]);
    // And the handle is cheap to carry around.
    let other = dispatch.clone();
    assert_eq!(other.hooks().len_for::<BeforeSave>(), 2);
}

// ---------------------------------------------------------------------------
// Start and shutdown
// ---------------------------------------------------------------------------

#[test]
fn shutdown_runs_both_phases_in_reverse_registration_order() {
    let rec = Rec::default();
    let mut app = AppBuilder::new()
        .plugin(Pool::new((rec.clone(), "pool", false)))
        .plugin_as("jobs", Pool::new((rec.clone(), "jobs", false)))
        .build()
        .unwrap();

    app.start().unwrap();
    let report = app.shutdown(ShutdownReason::Signal, Some(Duration::from_secs(30)));

    assert_eq!(
        rec.take(),
        vec![
            // start: registration order
            "pool:start(start)",
            "jobs:start(start)",
            // begin: reverse, so the job runner stops taking work before the
            // pool it depends on is told anything
            "jobs:begin_shutdown",
            "pool:begin_shutdown",
            // drain: reverse again
            "jobs:shutdown(grace=true)",
            "pool:shutdown(grace=true)",
        ]
    );
    assert!(report.is_clean(), "{report}");
    assert_eq!(report.reason(), ShutdownReason::Signal);
    assert_eq!(report.order().len(), 4);
}

#[test]
fn a_failing_drain_is_recorded_and_the_others_still_close() {
    let rec = Rec::default();
    let mut app = AppBuilder::new()
        .plugin(Pool::new((rec.clone(), "pool", false)))
        .plugin_as("broken", Pool::new((rec.clone(), "broken", true)))
        .build()
        .unwrap();

    let report = app.shutdown(ShutdownReason::Requested, None);

    assert!(!report.is_clean());
    assert_eq!(report.failures().len(), 1);
    assert_eq!(report.failures()[0].instance, "broken");
    assert_eq!(report.failures()[0].phase, ShutdownPhase::Drain);
    // `pool` closed anyway, which is the whole point.
    assert!(rec.take().contains(&"pool:shutdown(grace=false)".to_string()));
}

// ---------------------------------------------------------------------------
// The specification
// ---------------------------------------------------------------------------

#[test]
fn two_applications_in_one_process_do_not_share_hooks() {
    let rec_a = Rec::default();
    let rec_b = Rec::default();

    let app_a = AppBuilder::new()
        .plugin_hooking(Trace::new(TraceConfig::new(&rec_a, "a")))
        .build()
        .unwrap();
    let app_b = AppBuilder::new()
        .plugin_hooking(Trace::new(TraceConfig::new(&rec_b, "b1")))
        .plugin_hooking_as("second", Trace::new(TraceConfig::new(&rec_b, "b2")))
        .build()
        .unwrap();

    assert_eq!(app_a.pipeline().request_hooks(), 1);
    assert_eq!(app_b.pipeline().request_hooks(), 2);

    app_a.run(&mut request(), &mut RequestState::new(), ok_handler);
    assert!(rec_b.take().is_empty());

    app_b.run(&mut request(), &mut RequestState::new(), ok_handler);
    assert_eq!(rec_a.take().len(), 3);
    assert_eq!(rec_b.take().len(), 6);
}

#[test]
fn an_application_with_no_hooks_at_all_still_runs() {
    let app: brocanth::plugin_app::App<brocanth::plugin_context::Base, NoHooks> =
        AppBuilder::new().build().unwrap();
    let response = app.run(&mut request(), &mut RequestState::new(), ok_handler);
    assert_eq!(response.body(), b"body:");
    assert!(app.pipeline().is_empty());
}
