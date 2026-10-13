//! The core, served. Off by default; `--features bridge` turns it on.
//!
//! The rest of this crate takes one async function and knows nothing about
//! brocanth beyond the spelling of a verb. This module is the other half: it
//! takes an [`App`] and a [`Router`] and produces that async function, so an
//! application's `main` is a bind and a call rather than a hand-written
//! dispatch.
//!
//! It is a separate feature because the standalone shape is worth keeping. An
//! application that wants the socket and not the core gets exactly the crate
//! it had before: with the feature off this file is not compiled and none of
//! the API below exists. PLAN.md §8 — *"a feature that is off must mean the
//! code is absent, not that a branch returns early."* The tree is the same
//! either way, and honestly so: `brocanth` and `hyper` both already depend on
//! `http`, so the gate buys absence of code rather than absence of a crate.
//!
//! # The sync core on an async socket
//!
//! [`App::run`], [`MiddlewareStack::run`] and every handler the core knows are
//! synchronous. A handler cannot `.await`, so it holds whichever thread calls
//! it for as long as it computes -- or for as long as a database driver keeps
//! it waiting.
//!
//! **The decision, and it still holds for every route that cannot wait: a core
//! request runs on [`tokio::task::spawn_blocking`], never on a Tokio worker.**
//! Three candidates, and what each costs:
//!
//! 1. *Inline.* One line, no threads, no bounds to satisfy. It also gives the
//!    whole M:N property back: `fifty_waiting_handlers_do_not_cost_fifty_threads`
//!    in `tests/serves.rs` holds fifty futures on two workers, and the same
//!    fifty *synchronous* handlers on two workers serialise into twenty-five
//!    rounds. Measured in `tests/bridges.rs`: the bridged fifty finish in
//!    about the time of one, where inline arithmetic says 2.5 seconds.
//! 2. *`spawn_blocking`.* What this module does. The async workers stay free,
//!    accept keeps accepting, and a handler that blocks blocks nobody else.
//! 3. *A bounded pool with explicit backpressure.* Strictly better under
//!    overload, and not built here, because it needs a policy -- queue depth,
//!    and what to answer when the queue is full -- and that policy belongs to
//!    an application, not to a crate with no configuration surface.
//!
//! **The cost of choice 2, stated plainly.** One concurrent core request is
//! one OS thread. Tokio's blocking pool grows to 512 threads by default and
//! then queues, so five hundred slow handlers are five hundred stacks, and the
//! five hundred and first waits with no signal that it is waiting. A
//! deployment that expects that shape should say so --
//! `Builder::max_blocking_threads` -- and understand that the limit is a queue
//! and not a rejection. The second cost is `Send + 'static` on everything that
//! crosses: the [`App`] and the [`Router`] live behind an [`Arc`], and the
//! request is moved in whole rather than borrowed.
//!
//! The one thing choice 2 buys that is not performance: a panicking handler
//! comes back as a [`JoinError`](tokio::task::JoinError), so it is a 500 for
//! that request rather than a dropped connection.
//!
//! # A route that can wait
//!
//! The core now has two kinds of route. [`AnyHandler`] is `Sync` or `Async`
//! and says which **before** it is called, so the bridge chooses per request:
//!
//! | The route | Where it runs | What it costs |
//! |---|---|---|
//! | sync | `spawn_blocking`, exactly as above | one OS thread, for as long as it computes |
//! | async | inline, on the worker that read the request | one task, and one `Box::pin` |
//!
//! Fifty concurrent 100 ms handlers are fifty OS threads when they are sync
//! and **two threads** when they are async, which is the whole point of the
//! two kinds. Both numbers are measured, side by side over one table, in
//! `fifty_awaiting_core_handlers_cost_no_blocking_thread_at_all`.
//!
//! A table with no async route in it pays nothing for the choice.
//! [`RouteKind::MAY_BE_ASYNC`] is `false` for a `Router<Handler<E>>`, so for an
//! application that never registered an async route the inline arm is a
//! constant `false` and folds away, leaving the function it was before.
//!
//! **Where the two halves of the request run, and why it is not symmetric.**
//! For a sync route the whole pipeline -- request hooks, dispatch, handler,
//! error hooks, response hooks, `after_dispatch` -- runs on the blocking
//! thread, exactly as it did. For an async route only the handler can wait, so
//! [`App::begin`] and [`App::finish`] run inline on the worker. That is the one
//! thing this arm changes about hooks, and it is worth saying plainly: **a
//! `RequestHook::before_dispatch` that blocks will block a worker when the
//! route it precedes is async.** Response hooks are CPU by contract --
//! compression and ETag over a `Vec<u8>` that is already in memory -- so they
//! are safe there; a request hook that reaches a session store is not. Until
//! the core has an async request hook family, such a hook wants a sync route.
//!
//! The alternative was to run `begin` on a blocking thread and hop back to the
//! worker to await. It keeps that guarantee and loses the headline: fifty
//! requests arriving together would each take a pool thread for the length of
//! their hooks, and the blocking pool spawns a thread per queued task, so the
//! fifty threads come back for a shorter time. The measurement this module
//! exists for would then be a smaller number rather than zero.
//!
//! **Choosing before the handler is touched.** The kind is read from the route
//! table, through [`RouteKind::is_async`], before [`App::begin`] runs -- which
//! means it is read from the request as it arrived, and a request hook may
//! still rewrite the method or the path afterwards. The prediction is exact
//! unless a hook rewrites a request from one kind of route onto the other, and
//! both mispredictions stay **correct**: an async route reached on a blocking
//! thread is driven to completion there, by a parked thread rather than a
//! runtime, and a sync route reached inline is called inline. Each costs that
//! one request the thread placement the other kind wanted. The price of
//! reading the kind this early is one extra router lookup per request for a
//! table that has async routes in it, and nothing at all for a table that does
//! not.
//!
//! # The panic story, for the arm that has no `JoinError`
//!
//! `spawn_blocking` is a panic net as well as a thread: the panic unwinds into
//! Tokio, comes back as a [`JoinError`](tokio::task::JoinError) and becomes a
//! 500. An inline `.await` has no such net. A panicking handler would unwind
//! into hyper's connection task and the client would get a closed socket with
//! no status, which is a worse answer than the one the sync arm gives.
//!
//! So the inline arm carries its own net: [`catch_unwind`], a future that polls
//! another future inside [`std::panic::catch_unwind`]. Four things had to be
//! true for that to be sound rather than merely convenient.
//!
//! **1. What `AssertUnwindSafe` is asserting here.** [`UnwindSafe`] is a lint
//! against *observing* state that a caught panic left half-written. Almost no
//! real future is `UnwindSafe`, so the assertion is unavoidable; what makes it
//! true is what the bridge does afterwards. **On a caught panic this module
//! observes nothing the handler touched.** The request and the [`RequestState`]
//! are local to the caught region, they are shown to no hook, and they are
//! dropped. The answer is the same fixed `500 Internal Server Error` the
//! `JoinError` arm writes, built from two string literals. In particular the
//! caught outcome is **not** handed to [`App::finish`]: a half-written
//! `RequestState` never reaches an error hook or a response hook, and the two
//! arms produce the same bytes. That is the whole of the assertion.
//!
//! **2. What it does not cover.** Everything the handler reached *through* the
//! context: a connection pool, a cache, an `Arc<Mutex<_>>` an application put
//! there. Those outlive the request, and a panic in the middle of one leaves it
//! as the application left it. This is not new and not narrowed -- a panic
//! inside `spawn_blocking` leaves exactly the same shared state behind, and the
//! process keeps serving in both cases. `std::sync::Mutex` poisoning is what
//! turns a half-write into an error for the next request rather than silence.
//! The inline arm does not widen that exposure; it inherits it.
//!
//! **3. A panic mid-`.await`.** The panic unwinds the `poll` frame, which runs
//! the destructors of everything live in that poll; the future is then left
//! for the wrapper to drop, which runs the destructors of everything it held
//! across its last suspension point. Both halves of the cleanup happen. What a
//! panicking async handler is, exactly, is **a handler cancelled at its last
//! `.await`, plus an unwind** -- so what it can leave half-done is what
//! cancellation can leave half-done: work with no synchronous cleanup, such as
//! a transaction whose rollback is itself a future. `spawn_blocking` has no
//! equivalent, because a thread cannot be cancelled; that is the one way the
//! two arms genuinely differ. It is also not confined to panics: an inline
//! handler is droppable at every `.await` *because* it is a future, so a client
//! that disconnects mid-request can cancel it where it could not cancel a
//! blocking thread. An async route is cancellable. Say so in its documentation.
//!
//! A future that panicked is never polled again. The wrapper marks itself
//! finished before it returns the payload, and a second poll is a panic with a
//! sentence in it rather than a re-entry into a half-torn generator.
//!
//! **4. `panic = "abort"`.** [`catch_unwind`] catches nothing under it: the
//! process aborts at the panic site. That is reachable -- any binary depending
//! on this crate may set `panic = "abort"` in its own profile, and this
//! workspace sets no `[profile]` at all, so the default `unwind` is what the
//! tests measure. The thing to notice is that the two arms do **not** split
//! under abort: `spawn_blocking` produces no `JoinError` either, because the
//! process is gone before Tokio sees anything. "A panicking handler is a 500"
//! was already conditional on `panic = "unwind"` before this change, and it is
//! conditional on it in exactly the same way after.
//!
//! The panic is still printed. `std` runs the panic hook at the panic site,
//! before unwinding, so the default hook's message and backtrace reach stderr
//! from both arms; `brocanth-jobs`' `panic_hook` captures one into a
//! thread-local and reads it back *on the thread that panicked, immediately
//! after catching*, and that discipline survives here -- the panic and the
//! catch are the same `poll` call on the same thread, whichever thread that
//! poll ran on.
//!
//! # Who answers what
//!
//! Each status has exactly one owner, and the overlap is tested in
//! `tests/bridges.rs`.
//!
//! | Status | Owner | Why there |
//! |---|---|---|
//! | 501 | this crate, before the core | a verb the router cannot name cannot be dispatched |
//! | 413 | this crate, before the core | the body was refused while it was still arriving |
//! | 400 | this module, before the core | path and query that do not recompose into a URI |
//! | 404 | the core | the router owns the route table |
//! | 405 + `Allow` | the core | only the route table knows which verbs the path answers |
//! | 500 | the core | an error no error hook claimed; or a panicking handler, here |
//!
//! The 404 and the 405 are built *inside* the dispatch closure, so they travel
//! through [`App::run`] like any other response: the response hooks rewrite
//! them, and `after_dispatch` sees them. A logger that never saw a 404 would
//! not be a logger.
//!
//! ```no_run
//! # use brocanth::plugin_app::AppBuilder;
//! # use brocanth::plugin_routes::Handler;
//! # use brocanth::routing::Router;
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let app = AppBuilder::new().build()?;
//!     let router: Router<Handler<_>> = Router::builder().build()?;
//!     brocanth_server::bridge::serve("127.0.0.1:3000", app, router).await?;
//!     Ok(())
//! }
//! ```
//!
//! [`MiddlewareStack::run`]: brocanth::routing::MiddlewareStack::run

use std::any::Any;
use std::future::Future;
use std::net::SocketAddr;
use std::panic::{self, AssertUnwindSafe};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context as TaskContext, Poll};

use brocanth::plugin_app::{App, BuildError};
use brocanth::plugin_hooks::{
    Begin, HandlerError, HookFamilies, Request as CoreRequest, RequestState,
    Response as CoreResponse,
};
use brocanth::plugin_routes::{AnyHandler, Handler, RouteCx};
use brocanth::plugin_shutdown::{ShutdownReason, ShutdownReport};
use brocanth::routing::{Allow, Dispatch, Router, Verb};
use bytes::Bytes;
use http::header::{HeaderName, HeaderValue, CONTENT_LENGTH, CONTENT_TYPE, TRANSFER_ENCODING};

use crate::{Request, Response, ServeError, Server};

/// Who connected, as the core sees it.
///
/// [`crate::Request::peer`] has nowhere to go in an [`http::Request`], so it
/// goes in the extensions. A handler reads it with
/// `cx.request().extensions().get::<Peer>()`, and gets [`None`] behind a proxy
/// that did not say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer(pub SocketAddr);

/// Why a bridged server stopped.
#[derive(Debug)]
pub enum BridgeError {
    /// The socket. See [`ServeError`].
    Serve(ServeError),
    /// A plugin refused to start, so nothing was served.
    Start(BuildError),
    /// Serving finished, and a plugin then failed to stop. The requests were
    /// all answered; something did not let go of a resource.
    Shutdown(ShutdownReport),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serve(e) => write!(f, "{e}"),
            Self::Start(e) => write!(f, "the application did not start: {e}"),
            Self::Shutdown(report) => {
                write!(f, "shutdown was not clean:")?;
                for failure in report.failures() {
                    write!(f, " `{}` ({})", failure.instance, failure.phase)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for BridgeError {}

impl From<ServeError> for BridgeError {
    fn from(e: ServeError) -> Self {
        Self::Serve(e)
    }
}

// ---------------------------------------------------------------------------
// The conversion
// ---------------------------------------------------------------------------

/// A request whose path and query do not recompose into a URI.
#[derive(Debug)]
struct Unrepresentable;

/// This crate's request into the core's.
///
/// The two shapes disagree in three places.
///
/// *The target.* This crate splits the path from the query, because a router
/// matching `/users/7?tab=logs` would match nothing. [`http::Request`] wants
/// them back as one URI, so they are rejoined. The rejoin is fallible in
/// principle and unreachable in practice -- both halves came out of a URI
/// hyper had already parsed -- and the one caller answers 400 rather than
/// unwrapping.
///
/// *The headers.* A `Vec<(String, String)>` holds pairs that
/// [`http::HeaderMap`] would refuse: a name with a space in it, a value with a
/// control byte. **A pair `http` rejects is dropped, and the request is
/// served.** Nothing here panics and nothing here fails the request, for two
/// reasons. From the wire such a pair cannot arrive: hyper parsed the headers
/// into a `HeaderMap` before this crate ever saw them, so it is already proof
/// that each one is representable. From an in-process caller it is a bug in
/// that caller, and dropping fails closed -- a dropped `authorization` header
/// is a request that is not authorised, which is the safe end of the mistake.
/// Repeated headers are appended rather than inserted, so two `set-cookie`
/// lines stay two.
///
/// *The peer.* It becomes a [`Peer`] extension, the only place an
/// [`http::Request`] has for it.
fn to_core(request: Request) -> Result<CoreRequest, Unrepresentable> {
    let Request {
        verb,
        path,
        query,
        body,
        peer,
        headers,
    } = request;

    let target = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };

    let mut out = CoreRequest::new(body.to_vec());
    *out.method_mut() = verb.to_http();
    *out.uri_mut() = target.parse::<http::Uri>().map_err(|_| Unrepresentable)?;

    let map = out.headers_mut();
    for (name, value) in headers {
        let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) else {
            continue;
        };
        map.append(name, value);
    }

    if let Some(peer) = peer {
        out.extensions_mut().insert(Peer(peer));
    }
    Ok(out)
}

/// The core's response into this crate's.
///
/// Two headers are dropped on the way: `content-length` and
/// `transfer-encoding`. Framing is this crate's job -- the body is a `Bytes`
/// of known length by the time hyper sees it, and `HEAD` needs the length the
/// body *would* have had. A stale `content-length` from a response hook that
/// rewrote a body would otherwise cut the response in half.
///
/// A header value that is not UTF-8 is dropped, because this crate's response
/// headers are `String`. The alternative is `from_utf8_lossy`, which would
/// send a different value under the same name; dropping says less rather than
/// saying something wrong. `http` permits such a value and no formatter in
/// this workspace emits one.
fn from_core(response: CoreResponse) -> Response {
    let (parts, body) = response.into_parts();
    let mut headers = Vec::with_capacity(parts.headers.len());
    for (name, value) in parts.headers.iter() {
        if name == CONTENT_LENGTH || name == TRANSFER_ENCODING {
            continue;
        }
        let Ok(value) = value.to_str() else {
            continue;
        };
        headers.push((name.as_str().to_string(), value.to_string()));
    }

    Response {
        status: parts.status.as_u16(),
        body: Bytes::from(body),
        headers,
    }
}

/// A core response with a status, a plain-text body and nothing else.
fn plain(status: u16, body: &'static str) -> CoreResponse {
    let mut out = CoreResponse::new(body.as_bytes().to_vec());
    *out.status_mut() = http::StatusCode::from_u16(status).expect("a status this crate wrote");
    out.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    out
}

/// 405, carrying the verbs the path does answer.
///
/// Answering 404 instead would hide the only fact that makes the mistake
/// findable, which is why the router reports `allow` separately at all.
fn method_not_allowed(allow: Allow) -> CoreResponse {
    let mut out = plain(405, "Method Not Allowed");
    out.headers_mut()
        .insert(http::header::ALLOW, allow.header_value());
    out
}

// ---------------------------------------------------------------------------
// What a route table holds
// ---------------------------------------------------------------------------

/// What the bridge needs to know about whatever the route table holds.
///
/// Implemented for the two payloads the core offers, and for nothing else: a
/// [`Handler<E>`], which never waits, and an [`AnyHandler<E>`], which is one
/// kind or the other and says which before it is called.
///
/// It exists so that a table with no async route in it keeps the function it
/// had. [`RouteKind::MAY_BE_ASYNC`] is a constant, not a field: for a
/// `Router<Handler<E>>` the inline arm of [`Bridge::handle`] is dead code
/// before the optimiser sees it, so the choice is not even a branch.
pub trait RouteKind<E>: Send + Sync + 'static {
    /// Whether any route in a table of this kind could ever be async.
    ///
    /// `false` removes the inline arm, the extra router lookup that feeds it
    /// and the panic net that guards it, all at compile time.
    const MAY_BE_ASYNC: bool;

    /// Whether running *this* route needs something to drive it.
    ///
    /// Read before the handler is touched. It costs a discriminant.
    fn is_async(&self) -> bool;

    /// Runs the route on the thread that calls it, and does not return until
    /// it is done.
    ///
    /// A sync route is called. An async route can only be reached here when a
    /// request hook rewrote the request onto one after the bridge had already
    /// chosen a blocking thread, and it is driven to completion on that
    /// thread, by [`block_on`] -- which parks rather than spinning, and is on
    /// a blocking pool thread, where blocking is the point.
    fn call_blocking(&self, cx: &mut RouteCx<'_, E>) -> Result<CoreResponse, HandlerError>;

    /// Runs the route as a future, for a caller that will `.await` it.
    ///
    /// A sync route reached this way is called inline and the future it is
    /// wrapped in is ready on its first poll.
    fn call_inline<'a>(
        &'a self,
        cx: &'a mut RouteCx<'_, E>,
    ) -> impl Future<Output = Result<CoreResponse, HandlerError>> + Send + 'a;
}

impl<E: 'static> RouteKind<E> for Handler<E> {
    const MAY_BE_ASYNC: bool = false;

    fn is_async(&self) -> bool {
        false
    }

    fn call_blocking(&self, cx: &mut RouteCx<'_, E>) -> Result<CoreResponse, HandlerError> {
        self(cx)
    }

    // `async fn` would say the same thing and lose the `Send` bound, which the
    // bridge needs: the future is awaited inside a task hyper may move.
    #[allow(clippy::manual_async_fn)]
    fn call_inline<'a>(
        &'a self,
        cx: &'a mut RouteCx<'_, E>,
    ) -> impl Future<Output = Result<CoreResponse, HandlerError>> + Send + 'a {
        // Unreachable: `MAY_BE_ASYNC` is `false`, so nothing calls this. It
        // still has to typecheck, and it is still the right answer.
        async move { self(cx) }
    }
}

impl<E: 'static> RouteKind<E> for AnyHandler<E> {
    const MAY_BE_ASYNC: bool = true;

    fn is_async(&self) -> bool {
        AnyHandler::is_async(self)
    }

    fn call_blocking(&self, cx: &mut RouteCx<'_, E>) -> Result<CoreResponse, HandlerError> {
        match self.call_sync(cx) {
            Some(outcome) => outcome,
            None => block_on(self.call(cx)),
        }
    }

    fn call_inline<'a>(
        &'a self,
        cx: &'a mut RouteCx<'_, E>,
    ) -> impl Future<Output = Result<CoreResponse, HandlerError>> + Send + 'a {
        self.call(cx)
    }
}

/// Drives a future to completion on the calling thread.
///
/// Twenty lines of `std` rather than `Handle::block_on`, because the thread
/// this runs on is a blocking pool thread -- Tokio's rules about blocking
/// inside a runtime are about workers, and a function that cannot be wrong
/// about which is which is worth more than the lines it saves. The runtime is
/// still entered on that thread, so a future waiting on a Tokio timer or
/// socket is woken by the driver as usual: the waker unparks this thread.
///
/// A spurious unpark polls again, which every future permits.
fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);

    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = TaskContext::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

// ---------------------------------------------------------------------------
// The panic net for the inline arm
// ---------------------------------------------------------------------------

/// Polls `future` inside [`std::panic::catch_unwind`].
///
/// The caller passes [`AssertUnwindSafe`], the way `futures`' own
/// `catch_unwind` makes it do, so the assertion is spelled where it is made
/// rather than hidden in here. The module header says what it asserts and what
/// it does not cover.
fn catch_unwind<F: Future>(future: F) -> CatchUnwind<F> {
    CatchUnwind {
        future,
        finished: false,
    }
}

/// See [`catch_unwind`].
struct CatchUnwind<F> {
    future: F,
    finished: bool,
}

impl<F: Future> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
        // SAFETY: `future` is structurally pinned -- it is reached only
        // through `Pin`, it is never moved out, and it is dropped in place
        // with the rest of this struct. `finished` is not pinned and is only
        // read and written.
        let this = unsafe { self.get_unchecked_mut() };
        assert!(
            !this.finished,
            "a future that already finished was polled again"
        );
        // SAFETY: as above. `this` came from a `Pin`, so `future` has not
        // moved and will not.
        let future = unsafe { Pin::new_unchecked(&mut this.future) };

        match panic::catch_unwind(AssertUnwindSafe(|| future.poll(cx))) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(value)) => {
                this.finished = true;
                Poll::Ready(Ok(value))
            }
            Err(payload) => {
                // The panicked future is left where it is, finished, and is
                // dropped with this wrapper a few instructions later -- which
                // is what runs the destructors of whatever it held across its
                // last `.await`. Taking it out to drop it early would move a
                // value that is pinned, and `Pin::set`ting it to nothing would
                // put a panicking destructor between the drop and the write.
                this.finished = true;
                Poll::Ready(Err(payload))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The bridge
// ---------------------------------------------------------------------------

/// An application and its route table, ready to answer requests.
///
/// [`serve`] is the whole of what most applications need. This type is for the
/// one that wants its own [`Server`] -- an ephemeral port in a test, a stopper
/// wired to a signal handler -- and still wants the core behind it.
pub struct Bridge<E, H, R = Handler<E>> {
    app: App<E, H>,
    router: Router<R>,
}

impl<E: 'static, H: HookFamilies, R: RouteKind<E>> Bridge<E, H, R> {
    /// Pairs an application with the table it serves.
    ///
    /// The table holds [`Handler<E>`] when no route waits, and
    /// [`AnyHandler<E>`] when some do. Nothing else changes between the two.
    pub fn new(app: App<E, H>, router: Router<R>) -> Self {
        Self { app, router }
    }

    /// The application, for a caller that wants to read it before serving.
    pub fn app(&self) -> &App<E, H> {
        &self.app
    }

    /// One request, on a blocking thread, or inline when the route can wait.
    /// See the module header for why, and for what the inline arm costs.
    pub async fn handle(self: &Arc<Self>, request: Request) -> Response {
        let Ok(request) = to_core(request) else {
            // The path and the query do not recompose into a URI. 404 would be
            // a claim about a route table that has not been consulted and
            // cannot be, so this says what is true: the request line is one
            // this server cannot represent.
            return Response::text(400, "Bad Request");
        };

        // The kind is read from the table before the handler is touched, and
        // `MAY_BE_ASYNC` deletes the question entirely for a table that holds
        // no async route.
        if R::MAY_BE_ASYNC && self.routes_to_an_async_handler(&request) {
            return match catch_unwind(AssertUnwindSafe(self.inline(request))).await {
                Ok(response) => response,
                // The same two string literals the `JoinError` arm answers
                // with, and for the same reason. Nothing the handler touched
                // is read on the way here.
                Err(_) => Response::text(500, "Internal Server Error"),
            };
        }

        let bridge = Arc::clone(self);
        match tokio::task::spawn_blocking(move || bridge.blocking(request)).await {
            Ok(response) => response,
            // A handler panicked, or the runtime is shutting down. Inline, the
            // panic would have unwound into hyper's connection task and the
            // client would have got a closed socket with no status.
            Err(_) => Response::text(500, "Internal Server Error"),
        }
    }

    /// Whether this request, as it arrived, matches a route that waits.
    ///
    /// A hint, not a decision: the request hooks have not run yet and one of
    /// them may rewrite the method or the path. Both arms answer correctly
    /// either way; see the module header.
    fn routes_to_an_async_handler(&self, request: &CoreRequest) -> bool {
        Verb::from_http(request.method())
            .and_then(|verb| self.router.dispatch(verb, request.uri().path()).handler())
            .is_some_and(RouteKind::is_async)
    }

    /// One request, synchronously: hooks, router, handler, hooks.
    ///
    /// Everything the core does happens here, on one thread, with no `.await`
    /// anywhere inside it -- which is the shape the core has and the reason
    /// this function is called from [`tokio::task::spawn_blocking`].
    fn blocking(&self, mut request: CoreRequest) -> Response {
        let context = self.app.context();
        let mut state = RequestState::new();

        let response = self.app.run(&mut request, &mut state, |request, state| {
            // `request` is `&mut` for the hooks that rewrite it; dispatch only
            // reads, and `RouteCx` borrows it shared.
            let request: &CoreRequest = request;

            let Some(verb) = Verb::from_http(request.method()) else {
                // Unreachable from the wire: `crate::read` answered 501 before
                // this module was reached. It is reachable from a request hook
                // that rewrote the method, and the answer is the same one.
                return Ok(plain(501, "Not Implemented"));
            };

            match self.router.dispatch(verb, request.uri().path()) {
                Dispatch::Found { handler, matched } => {
                    let params = matched.params();
                    let mut cx = RouteCx::new(request, &params, &context, state);
                    handler.call_blocking(&mut cx)
                }
                // Built here rather than returned early, so that the response
                // hooks and `after_dispatch` see them like any other response.
                Dispatch::MethodNotAllowed { allow, .. } => Ok(method_not_allowed(allow)),
                Dispatch::NotFound => Ok(plain(404, "Not Found")),
            }
        });

        from_core(response)
    }

    /// One request, inline: hooks, router, `.await` the handler, hooks.
    ///
    /// The same pipeline as [`Bridge::blocking`] and the same owner for every
    /// status -- the 404, the 405 and the 501 are built inside the dispatch
    /// region here too, so the response hooks and `after_dispatch` see them.
    /// A request only reaches this function because it matched a route that
    /// waits, so those three are reachable here only when a request hook then
    /// rewrote it off that route; they are written out anyway, because the two
    /// arms must not be able to disagree about who owns a status.
    /// [`App::run`] is spelled out as [`App::begin`] and [`App::finish`] for
    /// one reason: the `.await` goes between them, and the core never holds a
    /// future.
    ///
    /// No panic net of its own. The caller wraps the whole of this in one, so
    /// that a panicking *hook* is a 500 as well -- on the other arm
    /// `spawn_blocking` catches those too, and the two arms answer alike.
    async fn inline(&self, mut request: CoreRequest) -> Response {
        let context = self.app.context();
        let mut state = RequestState::new();

        let outcome = match self.app.begin(&mut request, &mut state) {
            Begin::Answered(outcome) => outcome,
            Begin::Dispatch => {
                let read: &CoreRequest = &request;
                match Verb::from_http(read.method()) {
                    None => Ok(plain(501, "Not Implemented")),
                    Some(verb) => match self.router.dispatch(verb, read.uri().path()) {
                        Dispatch::Found { handler, matched } => {
                            let params = matched.params();
                            let mut cx = RouteCx::new(read, &params, &context, &mut state);
                            handler.call_inline(&mut cx).await
                        }
                        Dispatch::MethodNotAllowed { allow, .. } => Ok(method_not_allowed(allow)),
                        Dispatch::NotFound => Ok(plain(404, "Not Found")),
                    },
                }
            }
        };

        from_core(self.app.finish(&mut request, &mut state, outcome))
    }

    /// Starts the application, serves `server` until it is stopped, then stops
    /// the application.
    ///
    /// [`App::start`] runs here rather than in the caller because the core's
    /// own order puts it after the socket is open and `server` is already
    /// bound: a job runner that spawns workers in `start` should not spawn
    /// them for a port that turned out to be taken.
    ///
    /// # Errors
    ///
    /// [`BridgeError::Start`] if a plugin refuses to start -- nothing is
    /// served, and the plugins that did start are stopped first.
    /// [`BridgeError::Serve`] if the listener fails.
    /// [`BridgeError::Shutdown`] if serving finished cleanly and a plugin then
    /// failed to stop.
    pub async fn serve(mut self, server: Server) -> Result<(), BridgeError> {
        if let Err(e) = self.app.start() {
            // Some instances started before the one that failed; they are
            // running and nobody else will stop them.
            self.app.shutdown(ShutdownReason::Failed, None);
            return Err(BridgeError::Start(e));
        }

        let mut bridge = Arc::new(self);
        let handle = Arc::clone(&bridge);
        let outcome = server
            .run(move |request| {
                let handle = Arc::clone(&handle);
                async move { handle.handle(request).await }
            })
            .await;

        // `run` returns after hyper's graceful drain, so every connection task
        // has finished -- but a finished task can still be a few instructions
        // from dropping its clone of this `Arc`, and `App::shutdown` needs
        // `&mut`. Yielding a few times costs nothing and wins that race; if it
        // somehow does not, the plugins keep running rather than being stopped
        // through a reference somebody else holds.
        let mut report = None;
        for _ in 0..64 {
            match Arc::try_unwrap(bridge) {
                Ok(mut owned) => {
                    report = Some(owned.app.shutdown(ShutdownReason::Requested, None));
                    break;
                }
                Err(shared) => {
                    bridge = shared;
                    tokio::task::yield_now().await;
                }
            }
        }

        outcome?;
        match report {
            Some(report) if !report.is_clean() => Err(BridgeError::Shutdown(report)),
            _ => Ok(()),
        }
    }
}

/// Binds `addr` and serves `app` over `router` until the process ends.
///
/// The whole of a `main`:
///
/// ```no_run
/// # use brocanth::plugin_app::AppBuilder;
/// # use brocanth::plugin_routes::Handler;
/// # use brocanth::routing::Router;
/// # async fn f() -> Result<(), Box<dyn std::error::Error>> {
/// let app = AppBuilder::new().build()?;
/// let router: Router<Handler<_>> = Router::builder().build()?;
/// brocanth_server::bridge::serve("127.0.0.1:3000", app, router).await?;
/// # Ok(())
/// # }
/// ```
///
/// Nothing stops it: there is no [`crate::Stopper`] to be had, because nothing
/// holds the [`Server`]. A binary that wants to stop on a signal builds the
/// [`Server`] itself, keeps [`Server::stopper`], and calls
/// [`Bridge::serve`] -- which is what the tests do.
///
/// # Errors
///
/// As [`Bridge::serve`], plus [`BridgeError::Serve`] when the address cannot
/// be bound.
pub async fn serve<E: 'static, H: HookFamilies, R: RouteKind<E>>(
    addr: &str,
    app: App<E, H>,
    router: Router<R>,
) -> Result<(), BridgeError> {
    let server = Server::bind(addr).await?;
    Bridge::new(app, router).serve(server).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    fn request(headers: Vec<(String, String)>) -> Request {
        Request {
            verb: Verb::Get,
            path: "/x".into(),
            query: String::new(),
            body: Bytes::new(),
            peer: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4000)),
            headers,
        }
    }

    #[test]
    fn the_path_and_the_query_become_one_uri_again() {
        let mut r = request(vec![]);
        r.path = "/users/7".into();
        r.query = "tab=logs&page=2".into();
        let core = to_core(r).expect("a URI");
        assert_eq!(core.uri().path(), "/users/7");
        assert_eq!(core.uri().query(), Some("tab=logs&page=2"));
    }

    #[test]
    fn a_path_with_no_query_gets_no_question_mark() {
        let core = to_core(request(vec![])).expect("a URI");
        assert_eq!(core.uri().to_string(), "/x");
    }

    #[test]
    fn a_header_http_refuses_is_dropped_rather_than_panicking() {
        // A name with a space and a value with a control byte: neither can
        // arrive over a socket, because hyper parsed a `HeaderMap` before this
        // crate saw the request. Both can arrive from an in-process caller,
        // and neither may take the process down.
        let core = to_core(request(vec![
            ("x-fine".into(), "kept".into()),
            ("bad name".into(), "dropped".into()),
            ("x-bad-value".into(), "dro\u{0}pped".into()),
        ]))
        .expect("a URI");

        assert_eq!(core.headers().len(), 1, "{:?}", core.headers());
        assert_eq!(core.headers().get("x-fine").unwrap(), "kept");
        assert!(core.headers().get("bad name").is_none());
        assert!(core.headers().get("x-bad-value").is_none());
    }

    #[test]
    fn a_repeated_header_stays_repeated() {
        let core = to_core(request(vec![
            ("set-cookie".into(), "a=1".into()),
            ("set-cookie".into(), "b=2".into()),
        ]))
        .expect("a URI");
        let values: Vec<_> = core.headers().get_all("set-cookie").iter().collect();
        assert_eq!(values.len(), 2, "a second set-cookie was overwritten");
    }

    #[test]
    fn the_peer_survives_as_an_extension() {
        let core = to_core(request(vec![])).expect("a URI");
        assert_eq!(
            core.extensions().get::<Peer>().map(|p| p.0.port()),
            Some(4000)
        );
    }

    #[test]
    fn a_request_line_that_is_not_a_uri_is_refused_rather_than_guessed() {
        let mut r = request(vec![]);
        r.path = "not a path".into();
        assert!(to_core(r).is_err());
    }

    #[test]
    fn a_response_header_that_is_not_utf8_is_dropped_rather_than_mangled() {
        let mut core = plain(200, "ok");
        core.headers_mut().insert(
            "x-latin1",
            HeaderValue::from_bytes(&[0xC0, 0xDE]).expect("http permits it"),
        );
        let out = from_core(core);
        assert!(
            out.headers.iter().all(|(name, _)| name != "x-latin1"),
            "{:?}",
            out.headers
        );
        assert_eq!(out.headers.len(), 1, "the content type should survive");
    }

    #[test]
    fn the_core_does_not_get_to_frame_the_response() {
        // A response hook that rewrote a body and left the old length behind
        // would otherwise cut the response in half.
        let mut core = plain(200, "ok");
        core.headers_mut()
            .insert(CONTENT_LENGTH, HeaderValue::from_static("9999"));
        core.headers_mut()
            .insert(TRANSFER_ENCODING, HeaderValue::from_static("chunked"));
        let out = from_core(core);
        assert_eq!(
            out.headers,
            vec![(
                "content-type".to_string(),
                "text/plain; charset=utf-8".to_string()
            )]
        );
        assert_eq!(out.body, Bytes::from_static(b"ok"));
    }

    // -----------------------------------------------------------------
    // The kind, and the two ways to run one
    // -----------------------------------------------------------------

    #[test]
    fn a_table_of_plain_handlers_cannot_ask_the_question_at_all() {
        // Not a tautology: it is the constant that deletes the inline arm,
        // the extra lookup and the panic net from an application that never
        // registered a route that waits.
        const { assert!(!<Handler<()> as RouteKind<()>>::MAY_BE_ASYNC) };
        const { assert!(<AnyHandler<()> as RouteKind<()>>::MAY_BE_ASYNC) };
    }

    #[test]
    fn block_on_returns_what_a_ready_future_returns() {
        assert_eq!(block_on(async { 7 }), 7);
    }

    #[test]
    fn block_on_parks_until_another_thread_wakes_it() {
        // The case `call_blocking` reaches when a request hook rewrote a
        // request onto a route that waits: the thread is already blocking, so
        // it parks and the waker unparks it.
        struct Delayed {
            started: bool,
            done: Arc<AtomicBool>,
        }

        impl Future for Delayed {
            type Output = &'static str;

            fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Self::Output> {
                if self.done.load(Ordering::SeqCst) {
                    return Poll::Ready("woken");
                }
                if !self.started {
                    self.started = true;
                    let waker = cx.waker().clone();
                    let done = Arc::clone(&self.done);
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(10));
                        done.store(true, Ordering::SeqCst);
                        waker.wake();
                    });
                }
                Poll::Pending
            }
        }

        let delayed = Delayed {
            started: false,
            done: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(block_on(delayed), "woken");
    }

    // -----------------------------------------------------------------
    // The panic net
    // -----------------------------------------------------------------

    #[test]
    fn a_future_that_does_not_panic_comes_back_whole() {
        assert_eq!(
            block_on(catch_unwind(AssertUnwindSafe(async { 1 + 1 }))).ok(),
            Some(2)
        );
    }

    #[test]
    fn a_panic_before_the_first_await_is_caught() {
        // The panic messages in the output below are the point of the tests.
        let caught = block_on(catch_unwind(AssertUnwindSafe(async {
            panic!("on purpose, before");
        })));
        assert!(caught.is_err());
    }

    #[test]
    fn a_panic_after_a_suspension_is_caught_and_the_future_is_not_polled_again() {
        /// Pending once, then panics: a future that panicked mid-`.await`.
        struct PanicLater {
            polls: usize,
        }

        impl Future for PanicLater {
            type Output = ();

            fn poll(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<()> {
                self.polls += 1;
                if self.polls == 1 {
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                panic!("on purpose, after a suspension");
            }
        }

        let mut net = Box::pin(catch_unwind(AssertUnwindSafe(PanicLater { polls: 0 })));
        let mut cx = TaskContext::from_waker(std::task::Waker::noop());
        assert!(net.as_mut().poll(&mut cx).is_pending());
        assert!(matches!(net.as_mut().poll(&mut cx), Poll::Ready(Err(_))));

        // A third poll would re-enter a half-torn generator. It is a bug in
        // the caller rather than a case to handle, so it says so and stops.
        let again = panic::catch_unwind(AssertUnwindSafe(|| net.as_mut().poll(&mut cx)));
        assert!(
            again.is_err(),
            "a future that panicked was polled again and did not object"
        );
    }

    #[test]
    fn the_status_and_the_body_cross_unchanged() {
        let out = from_core(method_not_allowed(
            [Verb::Get, Verb::Post].into_iter().collect(),
        ));
        assert_eq!(out.status, 405);
        assert!(out
            .headers
            .iter()
            .any(|(name, value)| name == "allow" && value.contains("GET")));
    }
}
