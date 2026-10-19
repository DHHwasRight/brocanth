//! The evidence for the two handler types: an async route runs end to end, a
//! sync route is still a sync route, and the pipeline split lets the caller
//! put its own `.await` between the request hooks and the rest.
//!
//! The executor at the bottom is twenty lines of `std`. The core must be
//! drivable without a runtime -- that is the whole claim -- so a test that
//! reached for tokio would be testing the wrong thing, and
//! `no_hidden_dependencies.rs` would fail the build anyway.
//!
//! `docs/async-handlers.md` is the proposal these tests come from. Async
//! middleware is in it and is deliberately not here: nothing mounts a layer,
//! so there is no caller to test.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::{Base, Context as Cx};
use brocanth::plugin_hooks::{Begin, HandlerError, NoHooks, Request, RequestState, Response};
use brocanth::plugin_routes::{async_handler, AnyHandler, BoxFuture, Handler, RouteCx};
use brocanth::routing::{Dispatch, Params, Router, Verb};

// ---------------------------------------------------------------------------
// 1. The two kinds, in one table
// ---------------------------------------------------------------------------

fn sync_route() -> Handler<Base> {
    Box::new(|cx: &mut RouteCx<'_, Base>| {
        let who = cx.params().get("who").unwrap_or("nobody").to_string();
        Ok(Response::new(Body::from(format!("sync {who}"))))
    })
}

fn async_route() -> brocanth::plugin_routes::AsyncHandler<Base> {
    async_handler(|cx: &mut RouteCx<'_, Base>| {
        let who = cx.params().get("who").unwrap_or("nobody").to_string();
        Box::pin(async move {
            // A real suspension point, so the test is not a sync call in a
            // future's clothing.
            yield_once().await;
            Ok(Response::new(Body::from(format!("async {who}"))))
        }) as BoxFuture<'_, _>
    })
}

fn table() -> Router<AnyHandler<Base>> {
    let mut builder = Router::builder();
    builder
        .route("/sync/{who}", Verb::Get, AnyHandler::Sync(sync_route()))
        .expect("path");
    builder
        .route("/async/{who}", Verb::Get, AnyHandler::Async(async_route()))
        .expect("path");
    builder.build().expect("table")
}

/// One request, the way a bridge would run it: `begin`, the handler, `finish`.
fn serve(path: &str) -> Response {
    let app = AppBuilder::new().build().expect("app");
    let router = table();
    let context = app.context();
    let mut request = Request::new(Vec::new());
    *request.uri_mut() = path.parse().expect("uri");
    let mut state = RequestState::new();

    block_on(async {
        let outcome = match app.begin(&mut request, &mut state) {
            Begin::Dispatch => {
                let request_ref: &Request = &request;
                match router.dispatch(Verb::Get, request_ref.uri().path()) {
                    Dispatch::Found { handler, matched } => {
                        let params = matched.params();
                        let mut cx = RouteCx::new(request_ref, &params, &context, &mut state);
                        handler.call(&mut cx).await
                    }
                    _ => Ok(Response::new(Body::from("404"))),
                }
            }
            Begin::Answered(outcome) => outcome,
        };
        app.finish(&mut request, &mut state, outcome)
    })
}

#[test]
fn a_sync_route_and_an_async_route_live_in_one_table_and_both_answer() {
    assert_eq!(serve("/sync/ada").body().buffered(), Some(&b"sync ada"[..]));
    assert_eq!(
        serve("/async/ada").body().buffered(),
        Some(&b"async ada"[..])
    );
}

#[test]
fn the_kind_is_readable_before_the_handler_runs() {
    // This is what lets a bridge pick `spawn_blocking` for one route and an
    // inline `.await` for the other, without calling either.
    let router = table();
    let sync = match router.dispatch(Verb::Get, "/sync/x") {
        Dispatch::Found { handler, .. } => handler.is_async(),
        other => panic!("{other:?}"),
    };
    let asynchronous = match router.dispatch(Verb::Get, "/async/x") {
        Dispatch::Found { handler, .. } => handler.is_async(),
        other => panic!("{other:?}"),
    };
    assert!(!sync, "a sync route reported itself async");
    assert!(asynchronous, "an async route reported itself sync");
}

#[test]
fn a_caller_with_no_runtime_can_still_run_a_sync_route_and_is_told_about_the_other() {
    let app = AppBuilder::new().build().expect("app");
    let context = app.context();
    let router = table();
    let request = Request::new(Vec::new());
    let params = Params::empty();
    let mut state = RequestState::new();

    let Dispatch::Found { handler, .. } = router.dispatch(Verb::Get, "/sync/x") else {
        panic!("no route")
    };
    let mut cx = RouteCx::new(&request, &params, &context, &mut state);
    assert!(handler.call_sync(&mut cx).is_some());

    let Dispatch::Found { handler, .. } = router.dispatch(Verb::Get, "/async/x") else {
        panic!("no route")
    };
    assert!(
        handler.call_sync(&mut cx).is_none(),
        "an async route must not be runnable without a runtime"
    );
}

#[test]
fn request_state_written_before_an_await_is_there_after_it() {
    struct Started(u64);

    let handler = async_handler(|cx: &mut RouteCx<'_, Base>| {
        cx.state_mut().insert(Started(7));
        Box::pin(async move {
            yield_once().await;
            let seen = cx.state().get::<Started>().map(|s| s.0).unwrap_or(0);
            Ok(Response::new(Body::from(format!("{seen}"))))
        }) as BoxFuture<'_, _>
    });

    let app = AppBuilder::new().build().expect("app");
    let context = app.context();
    let request = Request::new(Vec::new());
    let params = Params::empty();
    let mut state = RequestState::new();
    let mut cx = RouteCx::new(&request, &params, &context, &mut state);
    let response = block_on(handler(&mut cx)).expect("ok");
    assert_eq!(response.body().buffered(), Some(&b"7"[..]));
}

#[test]
fn an_async_route_that_fails_reaches_the_error_path() {
    let handler = async_handler(|_cx: &mut RouteCx<'_, Base>| {
        Box::pin(async move {
            yield_once().await;
            Err(HandlerError::message("boom"))
        }) as BoxFuture<'_, _>
    });

    let app = AppBuilder::new().build().expect("app");
    let context = app.context();
    let mut request = Request::new(Vec::new());
    let params = Params::empty();
    let mut state = RequestState::new();

    let response = block_on(async {
        let outcome = {
            let request_ref: &Request = &request;
            let mut cx = RouteCx::new(request_ref, &params, &context, &mut state);
            handler(&mut cx).await
        };
        app.finish(&mut request, &mut state, outcome)
    });
    assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------
// 2. A plugin contributes both kinds
// ---------------------------------------------------------------------------

mod plugin {
    use super::*;
    use brocanth::plugin_api::{Plugin, PluginError};
    use brocanth::plugin_routes::{RouteBundle, RouteSource};

    pub struct Mixed;

    impl Plugin for Mixed {
        type Config = ();
        type Provides = brocanth::plugin_context::NoContext;
        const NAME: &'static str = "mixed";
        const CONTRIBUTES_ROUTES: bool = true;

        fn new(_config: Self::Config) -> Self {
            Self
        }
    }

    impl RouteSource<Base> for Mixed {
        const PREFIX: &'static str = "/mixed";
        fn routes(&self, bundle: &mut RouteBundle<'_, Base>) -> Result<(), PluginError> {
            // Not one line of this changed shape: a sync route is registered
            // exactly as it was before async existed.
            bundle.get(
                "fast",
                "/fast",
                Box::new(|_: &mut RouteCx<'_, Base>| Ok(Response::new(Body::from("fast")))),
            );
            // No `async_handler` here, on purpose: the argument's expected
            // type is already `AsyncHandler<Base>`, so a plain boxed closure
            // infers. The helper is for the generic positions.
            bundle.get_async(
                "slow",
                "/slow",
                Box::new(|_: &mut RouteCx<'_, Base>| {
                    Box::pin(async move {
                        yield_once().await;
                        Ok(Response::new(Body::from("slow")))
                    }) as BoxFuture<'_, _>
                }),
            );
            Ok(())
        }
    }
}

#[test]
fn one_plugin_contributes_a_sync_route_and_an_async_route_in_one_bundle() {
    let mut app = AppBuilder::new()
        .plugin(plugin::Mixed)
        .build()
        .expect("app");
    app.mount_routes_default::<plugin::Mixed>().expect("mount");

    let plan = app.route_plan();
    assert_eq!(plan.len(), 2);
    assert!(!plan.named("mixed.fast").expect("fast").handler.is_async());
    assert!(plan.named("mixed.slow").expect("slow").handler.is_async());
    assert_eq!(plan.named("mixed.slow").expect("slow").path, "/mixed/slow");
}

// ---------------------------------------------------------------------------
// The twenty lines of std
// ---------------------------------------------------------------------------

/// A future that is `Pending` exactly once, so a test that awaits it is
/// genuinely suspended and resumed rather than driven straight through.
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

/// Names two types the signatures above need, so the imports are not unused.
const _: Option<Cx<Base>> = None;
const _: Option<NoHooks> = None;
