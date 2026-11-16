//! Hook phases (ADR 10): a request or response hook names a `Phase`, and
//! `Order` places it inside that phase only.
//!
//! What these tests hold, through `AppBuilder::build` and a real request:
//!
//! * the phases decide the order between hooks of two phases, whatever the
//!   registration order;
//! * `Order::after` decides the order inside one phase;
//! * an `after` that reaches a hook of another phase stops the build with a
//!   `help:` line;
//! * `HookSource::REQUIRED` is unchanged: a phase does not make a hook
//!   required, and a required hook still cannot be turned off;
//! * `after_dispatch` still reaches every request hook, in every phase,
//!   when a hook stops the chain (ADR 9).

use std::sync::{Arc, Mutex};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::{AppBuilder, BuildError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HandlerError, HookRegistrar, HookSource, OnRequest, OnResponse, Request,
    RequestHook, RequestState, Response, ResponseHead, ResponseHook,
};
use brocanth::plugin_order::{Order, OrderError, Phase};

#[derive(Clone, Default)]
struct Rec(Arc<Mutex<Vec<String>>>);

impl Rec {
    fn push(&self, line: impl Into<String>) {
        self.0.lock().unwrap().push(line.into());
    }
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// One hook, request and response halves, that records each call.
struct Step {
    rec: Rec,
    id: String,
    stop: bool,
}

impl RequestHook for Step {
    fn before_dispatch(
        &self,
        _request: &mut Request,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        self.rec.push(format!("before {}", self.id));
        if self.stop {
            return Ok(Flow::respond(Response::new(Body::from("stopped"))));
        }
        Ok(Flow::Continue)
    }

    fn after_dispatch(&self, _request: &Request, _response: &Response, _state: &mut RequestState) {
        self.rec.push(format!("after {}", self.id));
    }
}

impl ResponseHook for Step {
    fn on_headers(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        self.rec.push(format!("out {}", self.id));
        Ok(())
    }
}

/// What one instance registers: a request and a response hook, both named
/// `hook`, in one phase, with one order.
#[derive(Clone)]
struct Spec {
    rec: Rec,
    phase: Phase,
    order: Order,
    stop: bool,
}

macro_rules! plugin {
    ($ty:ident, $name:literal) => {
        struct $ty(Spec);

        impl Plugin for $ty {
            type Config = Spec;
            type Provides = NoContext;
            const NAME: &'static str = $name;
            const INSTALLS_HOOKS: bool = true;

            fn new(spec: Spec) -> Self {
                Self(spec)
            }
        }

        impl HookSource for $ty {
            type Families = Both<OnRequest, OnResponse>;
            const REQUIRED: &'static [&'static str] = &["req"];

            fn install_hooks(&self, r: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
                let step = || Step {
                    rec: self.0.rec.clone(),
                    id: $name.to_string(),
                    stop: self.0.stop,
                };
                r.request("req", self.0.phase, self.0.order.clone(), step());
                r.response("res", self.0.phase, self.0.order.clone(), step());
                Ok(())
            }
        }
    };
}

plugin!(Proxy, "proxy");
plugin!(Limit, "limit");
plugin!(Csrf, "csrf");
plugin!(Cache, "cache");
plugin!(Gzip, "gzip");
plugin!(Audit, "audit");

fn spec(rec: &Rec, phase: Phase) -> Spec {
    Spec {
        rec: rec.clone(),
        phase,
        order: Order::new(),
        stop: false,
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

fn lines(rec: &Rec, prefix: &str) -> Vec<String> {
    rec.take()
        .into_iter()
        .filter_map(|l| l.strip_prefix(prefix).map(str::to_string))
        .collect()
}

#[test]
fn phases_order_hooks_of_two_phases_whatever_the_registration_order() {
    let rec = Rec::default();
    // Registered inside out on purpose.
    let app = AppBuilder::new()
        .plugin_hooking(Gzip::new(spec(&rec, Phase::Encode)))
        .plugin_hooking(Cache::new(spec(&rec, Phase::Cache)))
        .plugin_hooking(Csrf::new(spec(&rec, Phase::Guard)))
        .plugin_hooking(Proxy::new(spec(&rec, Phase::Identify)))
        .build()
        .expect("an application");
    run(&app);
    let all = rec.take();
    let pick =
        |prefix: &str| -> Vec<&str> { all.iter().filter_map(|l| l.strip_prefix(prefix)).collect() };
    // Inward: Encode, Identify, Guard, Cache.
    assert_eq!(pick("before "), ["gzip", "proxy", "csrf", "cache"]);
    // Outward: Cache, Guard, Identify, then Encode last.
    assert_eq!(pick("out "), ["cache", "csrf", "proxy", "gzip"]);
}

#[test]
fn hooks_of_one_phase_follow_their_after() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Audit::new(Spec {
            order: Order::new().after("csrf"),
            ..spec(&rec, Phase::Guard)
        }))
        .plugin_hooking(Csrf::new(spec(&rec, Phase::Guard)))
        .plugin_hooking(Proxy::new(spec(&rec, Phase::Identify)))
        .build()
        .expect("an application");
    run(&app);
    assert_eq!(lines(&rec, "before "), ["proxy", "csrf", "audit"]);
}

#[test]
fn first_and_last_place_hooks_in_a_phase_without_a_name() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Csrf::new(Spec {
            order: Order::new().last(),
            ..spec(&rec, Phase::Guard)
        }))
        .plugin_hooking(Audit::new(spec(&rec, Phase::Guard)))
        .plugin_hooking(Limit::new(Spec {
            order: Order::new().first(),
            ..spec(&rec, Phase::Guard)
        }))
        .build()
        .expect("an application");
    run(&app);
    assert_eq!(lines(&rec, "before "), ["limit", "audit", "csrf"]);
}

#[test]
fn an_after_across_phases_stops_the_build_with_a_help_line() {
    for order in [
        Order::new().after("proxy"),
        Order::new().after_if_present("proxy.req"),
        Order::new().before("cache"),
    ] {
        let rec = Rec::default();
        let err = AppBuilder::new()
            .plugin_hooking(Proxy::new(spec(&rec, Phase::Identify)))
            .plugin_hooking(Limit::new(Spec {
                order: order.clone(),
                ..spec(&rec, Phase::Guard)
            }))
            .plugin_hooking(Cache::new(spec(&rec, Phase::Cache)))
            .build()
            .err()
            .unwrap_or_else(|| panic!("{order:?} must stop the build"));
        assert!(
            matches!(
                err,
                BuildError::Order(OrderError::CrossPhase {
                    phase: Phase::Guard,
                    ..
                })
            ),
            "{err}"
        );
        let text = err.to_string();
        assert!(
            text.contains("`limit.req` runs in the Guard phase"),
            "{text}"
        );
        assert!(text.contains("\nhelp: remove that `"), "{text}");
    }
}

#[test]
fn required_is_unchanged_by_the_phase() {
    let rec = Rec::default();
    // A required hook in any phase still cannot be turned off.
    for phase in Phase::ALL {
        let err = AppBuilder::new()
            .plugin_hooking(Csrf::new(spec(&rec, phase)))
            .hooks_off("csrf", ["req"])
            .build()
            .err()
            .unwrap_or_else(|| panic!("{phase}: a required hook went off"));
        assert!(
            matches!(err, BuildError::HooksOffRequired { name: "req", .. }),
            "{err}"
        );
    }
    // A hook that is not in `REQUIRED` may go off, whatever its phase.
    let app = AppBuilder::new()
        .plugin_hooking(Csrf::new(spec(&rec, Phase::Guard)))
        .hooks_off("csrf", ["res"])
        .build()
        .expect("an application");
    assert_eq!(app.pipeline().skipped(), &[("csrf", "res")]);
}

#[test]
fn after_dispatch_still_reaches_every_hook_when_a_guard_stops_the_chain() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Cache::new(spec(&rec, Phase::Cache)))
        .plugin_hooking(Limit::new(Spec {
            stop: true,
            ..spec(&rec, Phase::Guard)
        }))
        .plugin_hooking(Proxy::new(spec(&rec, Phase::Identify)))
        .plugin_hooking(Gzip::new(spec(&rec, Phase::Encode)))
        .build()
        .expect("an application");
    run(&app);
    let all = rec.take();
    let pick =
        |prefix: &str| -> Vec<&str> { all.iter().filter_map(|l| l.strip_prefix(prefix)).collect() };
    // `cache` never saw the request: the guard answered first.
    assert_eq!(pick("before "), ["gzip", "proxy", "limit"]);
    // Every request hook gets `after_dispatch`, `cache` too, in reverse.
    assert_eq!(pick("after "), ["cache", "limit", "proxy", "gzip"]);
}
