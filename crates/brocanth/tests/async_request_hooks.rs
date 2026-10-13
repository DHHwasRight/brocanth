//! The fifth hook family, and the one question it raised.
//!
//! `docs/async-handlers.md` §5 proved that an async request hook costs nothing
//! when nobody registers one, and then recorded what it had not settled:
//!
//! > `OnRequest` and `OnAsyncRequest` are two families, two traits and two
//! > `Vec`s, and **the ordering between the two lists is a new seam a plugin
//! > author will trip over.**
//!
//! There are no longer two lists. A sync request hook and an async request
//! hook go into one chain and through one `sort`, so `after("session")` is
//! obeyed whichever kind `session` is and whichever kind declared it. The
//! first four tests below are the specification of that, and the second of
//! them — a **sync** hook ordered after an **async** one — is the case that no
//! two-list design can satisfy at all.
//!
//! What it costs is stated in the fifth test: an application that installs an
//! async request hook cannot use the synchronous entry points. That is a
//! compile error, pinned by a `compile_fail` doc test on
//! `Pipeline::before_dispatch`, and not a chain walked halfway.
//!
//! The executor at the bottom is twenty lines of `std`, for the reason
//! `tests/async_handlers.rs` gives: a test that reached for tokio would be
//! testing the wrong thing, and `no_hidden_dependencies.rs` would fail the
//! build anyway.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::{AppBuilder, BuildError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    AsyncRequestHook, Begin, Both, Flow, HandlerError, HookRegistrar, HookSource, OnAsyncRequest,
    OnRequest, Request, RequestHook, RequestState, Response,
};
use brocanth::plugin_order::Order;
use brocanth::plugin_routes::BoxFuture;

// ---------------------------------------------------------------------------
// A recorder the test owns
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

/// What a hook leaves on the request state for the next hook to find.
struct Account(String);

// ---------------------------------------------------------------------------
// `Session`: a plugin whose request hook waits
// ---------------------------------------------------------------------------

struct SessionConfig {
    rec: Rec,
    order: Order,
    /// Answer from `before_dispatch` instead of continuing.
    stop: bool,
    /// Fail from `before_dispatch`.
    fail: bool,
}

impl SessionConfig {
    fn new(rec: &Rec) -> Self {
        Self {
            rec: rec.clone(),
            order: Order::new(),
            stop: false,
            fail: false,
        }
    }
    fn ordered(mut self, order: Order) -> Self {
        self.order = order;
        self
    }
}

struct Session {
    rec: Rec,
    order: Order,
    stop: bool,
    fail: bool,
}

impl Plugin for Session {
    type Config = SessionConfig;
    type Provides = NoContext;
    const NAME: &'static str = "session";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self {
            rec: config.rec,
            order: config.order,
            stop: config.stop,
            fail: config.fail,
        }
    }
}

struct SessionHook {
    rec: Rec,
    stop: bool,
    fail: bool,
}

impl AsyncRequestHook for SessionHook {
    fn before_dispatch<'a>(
        &'a self,
        _request: &'a mut Request,
        state: &'a mut RequestState,
    ) -> BoxFuture<'a, Result<Flow, PluginError>> {
        Box::pin(async move {
            // A real suspension point, so this is not a sync hook in a
            // future's clothing.
            yield_once().await;
            self.rec.push("session:before");
            if self.fail {
                return Err(PluginError::new("no session"));
            }
            if self.stop {
                let mut short = Response::new(b"short".to_vec());
                *short.status_mut() = http::StatusCode::ACCEPTED;
                return Ok(Flow::respond(short));
            }
            // Written after the `.await`, which is the whole point of the
            // family: the lookup happened, and its answer is on the context
            // for the hooks and the handler that follow.
            state.insert(Account("ada".to_string()));
            Ok(Flow::Continue)
        })
    }

    fn after_dispatch(&self, _request: &Request, response: &Response, _state: &mut RequestState) {
        self.rec
            .push(format!("session:after({})", response.status().as_u16()));
    }
}

impl HookSource for Session {
    type Families = OnAsyncRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request_async(
            "load",
            self.order.clone(),
            SessionHook {
                rec: self.rec.clone(),
                stop: self.stop,
                fail: self.fail,
            },
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// `Stamp`: an ordinary synchronous request hook, to order against
// ---------------------------------------------------------------------------

struct StampConfig {
    rec: Rec,
    order: Order,
}

impl StampConfig {
    fn new(rec: &Rec) -> Self {
        Self {
            rec: rec.clone(),
            order: Order::new(),
        }
    }
    fn ordered(mut self, order: Order) -> Self {
        self.order = order;
        self
    }
}

struct Stamp {
    rec: Rec,
    order: Order,
}

impl Plugin for Stamp {
    type Config = StampConfig;
    type Provides = NoContext;
    const NAME: &'static str = "stamp";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self {
            rec: config.rec,
            order: config.order,
        }
    }
}

struct StampHook {
    rec: Rec,
}

impl RequestHook for StampHook {
    fn before_dispatch(
        &self,
        _request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        // Reports what the async hook left, so "ran after" is a fact about
        // the data and not only about the recorder.
        let seen = match state.get::<Account>() {
            Some(account) => account.0.as_str(),
            None => "-",
        };
        self.rec.push(format!("stamp:before({seen})"));
        Ok(Flow::Continue)
    }

    fn after_dispatch(&self, _request: &Request, response: &Response, _state: &mut RequestState) {
        self.rec
            .push(format!("stamp:after({})", response.status().as_u16()));
    }
}

impl HookSource for Stamp {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request(
            "id",
            self.order.clone(),
            StampHook {
                rec: self.rec.clone(),
            },
        );
        Ok(())
    }
}

/// A plugin that registers an async hook and does not say so in its record.
struct Forgetful(Rec);

impl Plugin for Forgetful {
    type Config = Rec;
    type Provides = NoContext;
    const NAME: &'static str = "forgetful";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self(config)
    }
}

impl HookSource for Forgetful {
    // The lie: it installs an async request hook below.
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request_async(
            "load",
            Order::new(),
            SessionHook {
                rec: self.0.clone(),
                stop: false,
                fail: false,
            },
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn request() -> Request {
    Request::new(Vec::new())
}

/// One request, the way an async server layer runs it: `begin_async`, the
/// handler, `finish`.
fn serve<E, H: brocanth::plugin_hooks::HookFamilies>(
    app: &brocanth::plugin_app::App<E, H>,
    rec: &Rec,
) -> Response {
    let mut request = request();
    let mut state = RequestState::new();
    block_on(async {
        let outcome = match app.begin_async(&mut request, &mut state).await {
            Begin::Dispatch => {
                rec.push("handler");
                Ok(Response::new(b"body".to_vec()))
            }
            Begin::Answered(outcome) => outcome,
        };
        app.finish(&mut request, &mut state, outcome)
    })
}

// ---------------------------------------------------------------------------
// 1. One chain, one declared order
// ---------------------------------------------------------------------------

#[test]
fn an_async_hook_ordered_after_a_sync_hook_runs_after_it() {
    let rec = Rec::default();
    // `session` is registered first and declares that it runs after `stamp`,
    // so registration order and declared order disagree and the declaration
    // has to win.
    let app = AppBuilder::new()
        .plugin_hooking(Session::new(
            SessionConfig::new(&rec).ordered(Order::new().after("stamp")),
        ))
        .plugin_hooking(Stamp::new(StampConfig::new(&rec)))
        .build()
        .unwrap();

    let response = serve(&app, &rec);

    assert_eq!(
        rec.take(),
        vec![
            // `stamp` ran first, so it saw no account.
            "stamp:before(-)",
            "session:before",
            "handler",
            "session:after(200)",
            "stamp:after(200)",
        ]
    );
    assert_eq!(response.status(), http::StatusCode::OK);
}

#[test]
fn a_sync_hook_ordered_after_an_async_hook_runs_after_it_and_sees_what_it_wrote() {
    let rec = Rec::default();
    // The direction a two-list design cannot express at all: the sync list
    // would always be drained before the async one, so `stamp` could never
    // follow `session` and could never see the account it loaded.
    let app = AppBuilder::new()
        .plugin_hooking(Stamp::new(
            StampConfig::new(&rec).ordered(Order::new().after("session")),
        ))
        .plugin_hooking(Session::new(SessionConfig::new(&rec)))
        .build()
        .unwrap();

    serve(&app, &rec);

    assert_eq!(
        rec.take(),
        vec![
            "session:before",
            // Written after an `.await` in the hook before it.
            "stamp:before(ada)",
            "handler",
            "stamp:after(200)",
            "session:after(200)",
        ]
    );
}

#[test]
fn registration_order_is_still_only_the_tie_break_across_the_two_kinds() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Session::new(SessionConfig::new(&rec)))
        .plugin_hooking(Stamp::new(StampConfig::new(&rec)))
        .build()
        .unwrap();

    serve(&app, &rec);

    assert_eq!(
        rec.take(),
        vec![
            "session:before",
            "stamp:before(ada)",
            "handler",
            "stamp:after(200)",
            "session:after(200)",
        ]
    );
}

#[test]
fn a_cycle_across_the_two_kinds_is_the_same_build_error_naming_both() {
    let rec = Rec::default();
    let error = AppBuilder::new()
        .plugin_hooking(Session::new(
            SessionConfig::new(&rec).ordered(Order::new().after("stamp")),
        ))
        .plugin_hooking(Stamp::new(
            StampConfig::new(&rec).ordered(Order::new().after("session")),
        ))
        .build()
        .map(|_| ())
        .unwrap_err();

    let BuildError::Order(_) = error else {
        panic!("expected an order error, got {error}");
    };
    let text = error.to_string();
    assert!(text.contains("session.load"), "{text}");
    assert!(text.contains("stamp.id"), "{text}");
}

// ---------------------------------------------------------------------------
// 2. What it costs: the record has to be honest
// ---------------------------------------------------------------------------

#[test]
fn an_async_hook_whose_plugin_forgot_the_marker_fails_the_build_and_names_it() {
    let error = AppBuilder::new()
        .plugin_hooking(Forgetful::new(Rec::default()))
        .build()
        .map(|_| ())
        .unwrap_err();

    let BuildError::AsyncHooksNotDeclared {
        ref instance,
        ref hook,
    } = error
    else {
        panic!("expected the undeclared-family error, got {error}");
    };
    assert_eq!(instance, "forgetful");
    assert_eq!(hook, "forgetful.load");
    assert!(error.to_string().contains("OnAsyncRequest"), "{error}");
}

#[test]
fn the_pipeline_counts_the_two_kinds_apart() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Session::new(SessionConfig::new(&rec)))
        .plugin_hooking(Stamp::new(StampConfig::new(&rec)))
        .build()
        .unwrap();

    assert_eq!(app.pipeline().request_hooks(), 1);
    assert_eq!(app.pipeline().async_request_hooks(), 1);
    assert!(app.pipeline().has_async_request_hooks());
    assert!(!app.pipeline().is_empty());
}

// ---------------------------------------------------------------------------
// 3. `Flow` means on the async path what it means on the sync one
// ---------------------------------------------------------------------------

#[test]
fn an_async_hook_that_answers_stops_the_whole_chain_and_not_after_dispatch() {
    let rec = Rec::default();
    let mut config = SessionConfig::new(&rec);
    config.stop = true;
    let app = AppBuilder::new()
        .plugin_hooking(Session::new(config))
        .plugin_hooking(Stamp::new(
            StampConfig::new(&rec).ordered(Order::new().after("session")),
        ))
        .build()
        .unwrap();

    let response = serve(&app, &rec);

    // `stamp` is a *sync* hook later in the chain, and it does not run: the
    // short-circuit stops the chain, not one list of it.
    assert_eq!(
        rec.take(),
        vec!["session:before", "stamp:after(202)", "session:after(202)"]
    );
    assert_eq!(response.status(), http::StatusCode::ACCEPTED);
    assert_eq!(response.body(), b"short");
}

#[test]
fn an_async_hook_that_fails_skips_the_handler_and_becomes_an_unclaimed_500() {
    let rec = Rec::default();
    let mut config = SessionConfig::new(&rec);
    config.fail = true;
    let app = AppBuilder::new()
        .plugin_hooking(Session::new(config))
        .build()
        .unwrap();

    let response = serve(&app, &rec);

    assert_eq!(rec.take(), vec!["session:before", "session:after(500)"]);
    assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------
// 4. The async entry point is usable without any async hook
// ---------------------------------------------------------------------------

#[test]
fn an_application_with_only_sync_hooks_may_still_use_the_async_entry_point() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Stamp::new(StampConfig::new(&rec)))
        .build()
        .unwrap();

    // Both entry points, same application, same order: a server layer never
    // has to ask which one an application wants.
    serve(&app, &rec);
    assert_eq!(
        rec.take(),
        vec!["stamp:before(-)", "handler", "stamp:after(200)"]
    );

    let rec2 = Rec::default();
    let app2 = AppBuilder::new()
        .plugin_hooking(Stamp::new(StampConfig::new(&rec2)))
        .build()
        .unwrap();
    app2.run(&mut request(), &mut RequestState::new(), |_, _| {
        rec2.push("handler");
        Ok(Response::new(b"body".to_vec()))
    });
    assert_eq!(
        rec2.take(),
        vec!["stamp:before(-)", "handler", "stamp:after(200)"]
    );
}

/// Names the families the signatures above do not mention, so a later change
/// that drops one from the public API fails here rather than silently.
const _: Option<Both<OnRequest, OnAsyncRequest>> = None;
const _: Option<HandlerError> = None;

// ---------------------------------------------------------------------------
// The executor: twenty lines of `std`, and no runtime anywhere near the core
// ---------------------------------------------------------------------------

fn yield_once() -> impl Future<Output = ()> {
    struct YieldOnce(bool);
    impl Future for YieldOnce {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                return Poll::Ready(());
            }
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
    YieldOnce(false)
}

struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}
