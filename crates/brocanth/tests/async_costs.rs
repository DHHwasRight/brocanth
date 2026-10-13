//! What async actually costs, counted rather than estimated.
//!
//! Same counting allocator as `hooks_cost_nothing.rs`. The handlers here
//! allocate nothing of their own -- an empty `Vec` body does not allocate --
//! so every count below is the machinery and not the work.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::future::Future;
use std::task::{Context, Poll, Waker};

use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{HandlerError, Request, RequestState, Response};
use brocanth::plugin_routes::{async_handler, AnyHandler, BoxFuture, Handler, RouteCx};
use brocanth::routing::Params;

thread_local! {
    static ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let _ = ALLOCATIONS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations_during<T>(f: impl FnOnce() -> T) -> (T, usize) {
    let before = ALLOCATIONS.with(Cell::get);
    let value = f();
    let after = ALLOCATIONS.with(Cell::get);
    (value, after - before)
}

fn sync_handler() -> Handler<Base> {
    Box::new(|_: &mut RouteCx<'_, Base>| Ok(Response::new(Vec::new())))
}

fn async_handler_of() -> brocanth::plugin_routes::AsyncHandler<Base> {
    async_handler(|_: &mut RouteCx<'_, Base>| {
        Box::pin(async move { Ok(Response::new(Vec::new())) }) as BoxFuture<'_, _>
    })
}

#[test]
fn a_sync_route_through_the_enum_allocates_exactly_what_it_did_before() {
    let app = AppBuilder::new().build().expect("app");
    let context = app.context();
    let request = Request::new(Vec::new());
    let params = Params::empty();
    let mut state = RequestState::new();

    let bare = sync_handler();
    let wrapped = AnyHandler::Sync(sync_handler());

    let mut cx = RouteCx::new(&request, &params, &context, &mut state);
    let _ = bare(&mut cx);
    let _ = wrapped.call_sync(&mut cx);

    let (_, direct) = allocations_during(|| bare(&mut cx));
    let (_, through_enum) = allocations_during(|| wrapped.call_sync(&mut cx));

    assert_eq!(
        direct, 0,
        "the baseline allocates; the comparison proves nothing"
    );
    assert_eq!(
        through_enum, 0,
        "a sync route behind `AnyHandler` allocated {through_enum} time(s); the enum must \
         add a branch and nothing else"
    );
}

#[test]
fn an_async_route_costs_one_allocation_for_its_future() {
    let app = AppBuilder::new().build().expect("app");
    let context = app.context();
    let request = Request::new(Vec::new());
    let params = Params::empty();
    let mut state = RequestState::new();
    let handler = async_handler_of();

    let mut cx = RouteCx::new(&request, &params, &context, &mut state);
    // `poll_to_completion`, not `block_on`: `block_on` builds an `Arc` waker
    // and that one allocation belongs to the executor, not to the route.
    let _ = poll_to_completion(handler(&mut cx));

    let (_, count) = allocations_during(|| poll_to_completion(handler(&mut cx)));

    // One `Box::pin`. Recorded as an exact number so that a future change
    // which quietly adds a second one fails here.
    assert_eq!(
        count, 1,
        "an async route allocated {count} time(s); one `Box::pin` is the price, and more \
         than one is a regression"
    );
}

#[test]
fn a_sync_route_awaited_through_the_uniform_call_still_allocates_nothing() {
    // `AnyHandler::call` is an `async fn`, so a sync route run through it is
    // wrapped in a state machine. The state machine lives on the caller's
    // stack, so the wrapping is free; what is not free is that the caller now
    // needs something to poll it.
    let app = AppBuilder::new().build().expect("app");
    let context = app.context();
    let request = Request::new(Vec::new());
    let params = Params::empty();
    let mut state = RequestState::new();
    let wrapped = AnyHandler::Sync(sync_handler());

    let mut cx = RouteCx::new(&request, &params, &context, &mut state);
    let _ = poll_to_completion(wrapped.call(&mut cx));

    let (_, count) = allocations_during(|| poll_to_completion(wrapped.call(&mut cx)));
    assert_eq!(
        count, 0,
        "a sync route awaited through `AnyHandler::call` allocated {count} time(s)"
    );
}

#[test]
fn a_handler_error_from_an_async_route_costs_the_same_box_it_always_did() {
    let app = AppBuilder::new().build().expect("app");
    let context = app.context();
    let mut request = Request::new(Vec::new());
    let params = Params::empty();
    let mut state = RequestState::new();

    let failing = async_handler(|_: &mut RouteCx<'_, Base>| {
        Box::pin(async move { Err(HandlerError::message("no")) }) as BoxFuture<'_, _>
    });

    let run = |request: &mut Request, state: &mut RequestState| {
        let outcome = {
            let request_ref: &Request = request;
            let mut cx = RouteCx::new(request_ref, &params, &context, state);
            poll_to_completion(failing(&mut cx))
        };
        app.finish(request, state, outcome)
    };

    let _ = run(&mut request, &mut state);
    let (response, _count) = allocations_during(|| run(&mut request, &mut state));
    assert_eq!(response.status(), http::StatusCode::INTERNAL_SERVER_ERROR);
}

// ---------------------------------------------------------------------------

/// Polls a future that is known to finish on its first poll, without a thread
/// park -- so that the measurement is of the future and not of the executor.
fn poll_to_completion<F: Future>(future: F) -> F::Output {
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut cx) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("this future was supposed to finish at once"),
    }
}
