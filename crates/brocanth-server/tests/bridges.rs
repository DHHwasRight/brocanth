//! The bridge, over a real socket.
//!
//! `serves.rs` proves the socket. This proves the two things the bridge adds:
//! that a request reaches the core and comes back, and that the core's
//! synchronous pipeline does not take the async runtime down with it.
//!
//! The requests are written by hand with a `TcpStream`, for the reason
//! `serves.rs` gives: it keeps the dev-dependency list at one, and it is the
//! only way to see the framing.
#![cfg(feature = "bridge")]

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::{App, AppBuilder};
use brocanth::plugin_body::Body as CoreBody;
use brocanth::plugin_context::{Base, NoContext};
use brocanth::plugin_hooks::{
    BodyShape, Flow, HookFamilies, HookRegistrar, HookSource, OnRequest, OnResponse,
    Request as CoreRequest, RequestHook, RequestState, Response as CoreResponse, ResponseHead,
    ResponseHook,
};
use brocanth::plugin_order::Order;
use brocanth::plugin_routes::{AnyHandler, Handler, RouteCx};
use brocanth::routing::{Router, RouterBuilder, Verb};
use brocanth_server::bridge::{Bridge, RouteKind};
use brocanth_server::{Server, Stopper};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// ---------------------------------------------------------------------------
// An application: one plugin, one hook, a handful of routes
// ---------------------------------------------------------------------------

/// Stamps every response that went through the pipeline.
///
/// This is the whole instrument for "which layer answered". A status carrying
/// `x-pipeline` was produced on the core's side of the bridge and was seen by
/// the response hooks; a status without it was produced by the server before
/// the core was reached. Asserting that by hand would be asserting the code
/// that is under test.
struct Stamp;

impl Plugin for Stamp {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "stamp";
    const INSTALLS_HOOKS: bool = true;

    fn new((): ()) -> Self {
        Self
    }
}

impl HookSource for Stamp {
    type Families = OnResponse;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.response("mark", Order::new(), Mark);
        Ok(())
    }
}

struct Mark;

impl ResponseHook for Mark {
    fn on_headers(
        &self,
        _request: &CoreRequest,
        head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let status = head.status.as_u16();
        head.headers.insert(
            "x-pipeline",
            http::HeaderValue::from_str(&status.to_string()).expect("a status is ASCII digits"),
        );
        Ok(())
    }
}

fn ok(body: impl Into<CoreBody>) -> CoreResponse {
    http::Response::builder()
        .status(200)
        .header("content-type", "text/plain; charset=utf-8")
        .body(body.into())
        .expect("a response this test wrote")
}

type App0 = App<Base, brocanth::plugin_hooks::Both<OnResponse, brocanth::plugin_hooks::NoHooks>>;

/// The application under test, and its table.
///
/// `routes` is handed the builder so each test adds the handler it needs
/// without a shared table growing a case per test.
fn application<F>(routes: F) -> (App0, Router<Handler<Base>>)
where
    F: FnOnce(&mut brocanth::routing::RouterBuilder<Handler<Base>>),
{
    let app = AppBuilder::new()
        .plugin_hooking(Stamp::new(()))
        .build()
        .expect("the application builds");
    let mut builder = Router::builder();
    routes(&mut builder);
    let router = builder.build().expect("the table builds");
    (app, router)
}

/// The same application, over a table that may hold routes that wait.
///
/// The only difference is the payload type: `AnyHandler<Base>` rather than
/// `Handler<Base>`. The bridge reads the kind off the table, so this is the
/// whole of what an application does to opt into the inline arm.
fn awaiting_application<F>(routes: F) -> (App0, Router<AnyHandler<Base>>)
where
    F: FnOnce(&mut RouterBuilder<AnyHandler<Base>>),
{
    let app = AppBuilder::new()
        .plugin_hooking(Stamp::new(()))
        .build()
        .expect("the application builds");
    let mut builder = Router::builder();
    routes(&mut builder);
    let router = builder.build().expect("the table builds");
    (app, router)
}

/// A route that waits, written the way a plugin writes one.
fn waits(
    f: impl for<'a, 'r> Fn(
            &'a mut RouteCx<'r, Base>,
        ) -> brocanth::plugin_routes::BoxFuture<
            'a,
            Result<CoreResponse, brocanth::plugin_hooks::HandlerError>,
        > + Send
        + Sync
        + 'static,
) -> AnyHandler<Base> {
    AnyHandler::Async(Box::new(f))
}

/// What a handler reports about where and when it ran.
///
/// The thread set is the structural half of the measurement: a route that
/// needs a thread per concurrent request shows fifty of them, and a route that
/// does not shows as many as the runtime has workers. Wall clock alone would
/// be a timing assertion; this is a counting one.
#[derive(Default)]
struct Probe {
    entered: AtomicUsize,
    live: AtomicUsize,
    peak: AtomicUsize,
    threads: Mutex<HashSet<ThreadId>>,
}

impl Probe {
    fn enter(&self) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        let now = self.live.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.threads
            .lock()
            .expect("no handler panics while holding this")
            .insert(std::thread::current().id());
    }

    fn leave(&self) {
        self.live.fetch_sub(1, Ordering::SeqCst);
    }

    fn entered(&self) -> usize {
        self.entered.load(Ordering::SeqCst)
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }

    fn threads(&self) -> usize {
        self.threads
            .lock()
            .expect("no handler panics while holding this")
            .len()
    }
}

// ---------------------------------------------------------------------------
// Running one, over a real port
// ---------------------------------------------------------------------------

struct Running {
    addr: String,
    stopper: Stopper,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start<E: 'static, H: HookFamilies, R: RouteKind<E>>(
        app: App<E, H>,
        router: Router<R>,
    ) -> Self {
        let server = Server::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral port");
        let addr = server.local_addr().expect("local addr").to_string();
        let stopper = server.stopper();
        let task = tokio::spawn(async move {
            Bridge::new(app, router)
                .serve(server)
                .await
                .expect("the bridge served");
        });
        Self {
            addr,
            stopper,
            task,
        }
    }

    async fn stop(self) {
        self.stopper.stop();
        self.task.await.expect("the server task did not panic");
    }
}

async fn send(addr: &str, raw: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(raw.as_bytes()).await.expect("write");
    stream.flush().await.expect("flush");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");
    String::from_utf8_lossy(&response).into_owned()
}

async fn request(addr: &str, verb: &str, path: &str) -> String {
    send(
        addr,
        &format!("{verb} {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"),
    )
    .await
}

async fn get(addr: &str, path: &str) -> String {
    request(addr, "GET", path).await
}

// ---------------------------------------------------------------------------
// A request reaches the core and comes back
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_handler_sees_the_path_the_parameters_and_the_query() {
    let (app, router) = application(|b| {
        b.route(
            "/users/{id}",
            Verb::Get,
            Box::new(|cx: &mut RouteCx<'_, Base>| {
                let id = cx.params().get("id").unwrap_or("?").to_string();
                let query = cx.request().uri().query().unwrap_or("").to_string();
                Ok(ok(format!("id={id} query={query}")))
            }),
        )
        .expect("one route");
    });
    let server = Running::start(app, router).await;

    let response = get(&server.addr, "/users/7?tab=logs").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("id=7 query=tab=logs"), "{response}");
    server.stop().await;
}

#[tokio::test]
async fn a_post_body_and_a_header_reach_the_core() {
    let (app, router) = application(|b| {
        b.route(
            "/submit",
            Verb::Post,
            Box::new(|cx: &mut RouteCx<'_, Base>| {
                let ct = cx
                    .request()
                    .headers()
                    .get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("none")
                    .to_string();
                let body = String::from_utf8_lossy(cx.request().body()).into_owned();
                Ok(ok(format!("{ct}: {body}")))
            }),
        )
        .expect("one route");
    });
    let server = Running::start(app, router).await;

    let response = send(
        &server.addr,
        "POST /submit HTTP/1.1\r\nHost: x\r\nContent-Type: text/csv\r\nContent-Length: 5\r\n\
         Connection: close\r\n\r\nhello",
    )
    .await;
    assert!(response.ends_with("text/csv: hello"), "{response}");
    server.stop().await;
}

#[tokio::test]
async fn head_is_answered_by_the_get_handler_with_the_length_and_no_body() {
    // Two layers agree here and neither may be deleted: the core's router
    // falls back from HEAD to GET, and this crate drops the body on the way
    // out while keeping the length the body would have had.
    let (app, router) = application(|b| {
        b.route(
            "/",
            Verb::Get,
            Box::new(|_: &mut RouteCx<'_, Base>| Ok(ok("0123456789"))),
        )
        .expect("one route");
    });
    let server = Running::start(app, router).await;

    let response = request(&server.addr, "HEAD", "/").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("content-length: 10"), "{response}");
    assert!(
        !response.contains("0123456789"),
        "a HEAD sent a body:\n{response}"
    );
    server.stop().await;
}

#[tokio::test]
async fn a_content_length_the_core_wrote_does_not_reach_the_wire() {
    // A response hook that rewrites a body and leaves the old length behind
    // would cut the response in half. Framing belongs to this crate.
    let (app, router) = application(|b| {
        b.route(
            "/",
            Verb::Get,
            Box::new(|_: &mut RouteCx<'_, Base>| {
                Ok(http::Response::builder()
                    .status(200)
                    .header("content-length", "9999")
                    .body(CoreBody::from("short"))
                    .expect("a response this test wrote"))
            }),
        )
        .expect("one route");
    });
    let server = Running::start(app, router).await;

    let response = get(&server.addr, "/").await;
    assert!(!response.contains("9999"), "{response}");
    assert!(response.contains("content-length: 5"), "{response}");
    server.stop().await;
}

// ---------------------------------------------------------------------------
// One layer per status
// ---------------------------------------------------------------------------

/// A table with one GET route, for the status tests.
fn one_route() -> (App0, Router<Handler<Base>>) {
    application(|b| {
        b.route(
            "/hello",
            Verb::Get,
            Box::new(|_: &mut RouteCx<'_, Base>| Ok(ok("hello"))),
        )
        .expect("one route");
    })
}

#[tokio::test]
async fn the_core_owns_the_404_and_the_hooks_see_it() {
    let (app, router) = one_route();
    let server = Running::start(app, router).await;

    let response = get(&server.addr, "/nowhere").await;
    assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    assert!(
        response.contains("x-pipeline: 404"),
        "the 404 skipped the pipeline, so a logger would never see it:\n{response}"
    );
    server.stop().await;
}

#[tokio::test]
async fn the_core_owns_the_405_and_says_which_verbs_the_path_answers() {
    let (app, router) = one_route();
    let server = Running::start(app, router).await;

    let response = request(&server.addr, "DELETE", "/hello").await;
    assert!(response.starts_with("HTTP/1.1 405"), "{response}");
    assert!(
        response.contains("allow: GET, HEAD"),
        "a 405 without an Allow list hides the only findable fact:\n{response}"
    );
    assert!(response.contains("x-pipeline: 405"), "{response}");
    server.stop().await;
}

#[tokio::test]
async fn the_server_owns_the_501_and_the_core_never_sees_it() {
    // A verb the router has no name for cannot be dispatched, so there is
    // nothing for the core to decide. It answers before the bridge is reached.
    let (app, router) = one_route();
    let server = Running::start(app, router).await;

    let response = request(&server.addr, "PROPFIND", "/hello").await;
    assert!(response.starts_with("HTTP/1.1 501"), "{response}");
    assert!(
        !response.contains("x-pipeline"),
        "the 501 entered the core, so two layers now answer it:\n{response}"
    );
    server.stop().await;
}

#[tokio::test]
async fn the_server_owns_the_413_and_the_core_never_sees_it() {
    let (app, router) = application(|b| {
        b.route(
            "/hello",
            Verb::Post,
            Box::new(|_: &mut RouteCx<'_, Base>| panic!("a body past the cap reached the core")),
        )
        .expect("one route");
    });
    let server = Running::start(app, router).await;

    let oversized = 3 * 1024 * 1024;
    let mut raw = format!(
        "POST /hello HTTP/1.1\r\nHost: x\r\nContent-Length: {oversized}\r\n\
         Connection: close\r\n\r\n"
    );
    raw.push_str(&"x".repeat(oversized));

    // The server answers and closes while the client is still writing, so a
    // reset here is the correct outcome rather than a failure.
    let mut response = String::new();
    if let Ok(mut stream) = TcpStream::connect(&server.addr).await {
        let _ = stream.write_all(raw.as_bytes()).await;
        let _ = stream.flush().await;
        let mut bytes = Vec::new();
        let _ = stream.read_to_end(&mut bytes).await;
        response = String::from_utf8_lossy(&bytes).into_owned();
    }

    if !response.is_empty() {
        assert!(response.starts_with("HTTP/1.1 413"), "{response}");
        assert!(!response.contains("x-pipeline"), "{response}");
    }
    server.stop().await;
}

#[tokio::test]
async fn a_panicking_handler_is_a_500_rather_than_a_dropped_connection() {
    // The handler below panics on purpose; the backtrace in the test output is
    // the point of the test. Inline, that panic unwinds into hyper's
    // connection task and the client gets a closed socket with no status.
    // On a blocking thread it comes back as a `JoinError`.
    let (app, router) = application(|b| {
        b.route(
            "/boom",
            Verb::Get,
            Box::new(|_: &mut RouteCx<'_, Base>| panic!("on purpose")),
        )
        .expect("one route");
    });
    let server = Running::start(app, router).await;

    let response = get(&server.addr, "/boom").await;
    assert!(response.starts_with("HTTP/1.1 500"), "{response}");
    server.stop().await;
}

// ---------------------------------------------------------------------------
// The measurement
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fifty_blocking_core_handlers_do_not_block_the_two_async_workers() {
    // `fifty_waiting_handlers_do_not_cost_fifty_threads` in `serves.rs` is the
    // same claim for a handler that can `.await`. The core cannot: a handler
    // reached through `App::run` holds its thread. So this measures the claim
    // the bridge actually makes, which is narrower and has a price.
    //
    // Fifty requests arrive at once. Each handler calls `std::thread::sleep`,
    // which no runtime can multiplex. Two async workers. The arithmetic:
    //
    //   inline          -> 25 rounds of 100ms  = ~2.5s, and the accept loop is
    //                      wedged for all of it
    //   spawn_blocking  -> one round of 100ms, on 50 threads of the blocking
    //                      pool, with both workers free throughout
    //
    // The cost is in the second line and is not hidden: fifty concurrent core
    // requests are fifty OS threads. `peak` below is that thread count, named.
    const REQUESTS: usize = 50;
    const WAIT: Duration = Duration::from_millis(100);

    let live = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(AtomicUsize::new(0));

    let (live_h, peak_h, entered_h) = (Arc::clone(&live), Arc::clone(&peak), Arc::clone(&entered));
    let (app, router) = application(move |b| {
        b.route(
            "/wait",
            Verb::Get,
            Box::new(move |_: &mut RouteCx<'_, Base>| {
                entered_h.fetch_add(1, Ordering::SeqCst);
                let now = live_h.fetch_add(1, Ordering::SeqCst) + 1;
                peak_h.fetch_max(now, Ordering::SeqCst);
                // Not `tokio::time::sleep`: the core is synchronous and a
                // handler cannot await. This is what a blocking driver call
                // looks like from the runtime's side.
                std::thread::sleep(WAIT);
                live_h.fetch_sub(1, Ordering::SeqCst);
                Ok(ok("waited"))
            }),
        )
        .expect("one route");
    });
    let server = Running::start(app, router).await;

    // A heartbeat on the same two workers. If the core ran inline, this task
    // would be starved for the whole run; its tick count is the evidence that
    // the async side stayed free.
    let ticks = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ticks);
    let heartbeat = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(5)).await;
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });

    let start = Instant::now();
    let mut inflight = Vec::with_capacity(REQUESTS);
    for _ in 0..REQUESTS {
        let addr = server.addr.clone();
        inflight.push(tokio::spawn(async move { get(&addr, "/wait").await }));
    }
    for task in inflight {
        let response = task.await.expect("request task");
        assert!(response.ends_with("waited"), "{response}");
    }
    let elapsed = start.elapsed();
    heartbeat.abort();

    let peak = peak.load(Ordering::SeqCst);
    let ticks = ticks.load(Ordering::SeqCst);
    assert_eq!(entered.load(Ordering::SeqCst), REQUESTS);
    assert!(
        elapsed < WAIT * 4,
        "{REQUESTS} concurrent {WAIT:?} synchronous handlers took {elapsed:?} on 2 worker \
         threads; inline they would take ~{:?}, so they did not overlap",
        WAIT * (REQUESTS as u32 / 2)
    );
    assert!(
        peak >= REQUESTS / 2,
        "only {peak} of {REQUESTS} handlers were ever in flight at once, so the blocking \
         pool serialised them"
    );
    assert!(
        ticks >= 5,
        "the heartbeat ticked {ticks} times in {elapsed:?}, so the async workers were blocked"
    );

    println!(
        "bridged: {REQUESTS} synchronous {WAIT:?} handlers in {elapsed:?}, peak {peak} \
         concurrent (= {peak} blocking threads), heartbeat {ticks} ticks"
    );
    server.stop().await;
}

// ---------------------------------------------------------------------------
// A route that waits
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_async_route_sees_the_path_the_parameters_and_the_query_across_an_await() {
    // The borrow is the point: the handler reads a parameter, waits, and then
    // reads the request again, with nothing cloned to make that work.
    let (app, router) = awaiting_application(|b| {
        b.route(
            "/users/{id}",
            Verb::Get,
            waits(|cx: &mut RouteCx<'_, Base>| {
                Box::pin(async move {
                    let id = cx.params().get("id").unwrap_or("?").to_string();
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let query = cx.request().uri().query().unwrap_or("").to_string();
                    cx.state_mut().insert(id.clone());
                    Ok(ok(format!("id={id} query={query}")))
                })
            }),
        )
        .expect("one route");
    });
    let server = Running::start(app, router).await;

    let response = get(&server.addr, "/users/7?tab=logs").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("id=7 query=tab=logs"), "{response}");
    assert!(
        response.contains("x-pipeline: 200"),
        "the inline arm skipped the response hooks, so a logger would never \
         see an async route:\n{response}"
    );
    server.stop().await;
}

#[tokio::test]
async fn a_sync_route_in_a_table_that_has_async_routes_is_unchanged() {
    // Both kinds in one table, which is the shape `AnyHandler` exists for.
    let (app, router) = awaiting_application(|b| {
        b.route(
            "/sync",
            Verb::Get,
            AnyHandler::Sync(Box::new(|_: &mut RouteCx<'_, Base>| Ok(ok("plain")))),
        )
        .expect("one route");
        b.route(
            "/await",
            Verb::Get,
            waits(|_: &mut RouteCx<'_, Base>| Box::pin(async move { Ok(ok("waited")) })),
        )
        .expect("another route");
    });
    let server = Running::start(app, router).await;

    let response = get(&server.addr, "/sync").await;
    assert!(response.contains("x-pipeline: 200"), "{response}");
    assert!(response.ends_with("plain"), "{response}");
    assert!(get(&server.addr, "/await").await.ends_with("waited"));

    // The 404 and the 405 are still the core's, from a table of either kind.
    let missing = get(&server.addr, "/nowhere").await;
    assert!(missing.starts_with("HTTP/1.1 404"), "{missing}");
    assert!(missing.contains("x-pipeline: 404"), "{missing}");
    let wrong_verb = request(&server.addr, "DELETE", "/await").await;
    assert!(wrong_verb.starts_with("HTTP/1.1 405"), "{wrong_verb}");
    assert!(wrong_verb.contains("x-pipeline: 405"), "{wrong_verb}");
    server.stop().await;
}

#[tokio::test]
async fn a_panicking_async_handler_is_a_500_rather_than_a_dropped_connection() {
    // The handlers below panic on purpose; the backtraces in the test output
    // are the point of the test. There is no `JoinError` on this arm, so the
    // 500 comes from the bridge's own `catch_unwind` -- once for a panic
    // before the first `.await`, and once for a panic after one, which is the
    // case where the future is left suspended and has to be dropped.
    let (app, router) = awaiting_application(|b| {
        b.route(
            "/boom",
            Verb::Get,
            waits(|_: &mut RouteCx<'_, Base>| Box::pin(async move { panic!("on purpose") })),
        )
        .expect("one route");
        b.route(
            "/boom-later",
            Verb::Get,
            waits(|_: &mut RouteCx<'_, Base>| {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    panic!("on purpose, after an await")
                })
            }),
        )
        .expect("another route");
        b.route(
            "/fine",
            Verb::Get,
            waits(|_: &mut RouteCx<'_, Base>| Box::pin(async move { Ok(ok("still here")) })),
        )
        .expect("a third route");
    });
    let server = Running::start(app, router).await;

    for path in ["/boom", "/boom-later"] {
        let response = get(&server.addr, path).await;
        assert!(response.starts_with("HTTP/1.1 500"), "{path}: {response}");
        assert!(
            !response.contains("x-pipeline"),
            "a caught panic was shown to the response hooks, so the two arms \
             no longer answer alike -- {path}: {response}"
        );
    }

    // The net caught it rather than taking the connection task with it.
    assert!(
        get(&server.addr, "/fine").await.ends_with("still here"),
        "the server stopped serving after a handler panicked"
    );
    server.stop().await;
}

/// Rewrites `/front` onto `/back`, to move a request across the kinds.
struct Reroute;

impl Plugin for Reroute {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "reroute";
    const INSTALLS_HOOKS: bool = true;

    fn new((): ()) -> Self {
        Self
    }
}

impl HookSource for Reroute {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request("reroute", Order::new(), Rewrite);
        Ok(())
    }
}

struct Rewrite;

impl RequestHook for Rewrite {
    fn before_dispatch(
        &self,
        request: &mut CoreRequest,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if let Some(rest) = request.uri().path().strip_prefix("/front") {
            *request.uri_mut() = format!("/back{rest}")
                .parse()
                .expect("a path this test wrote");
        }
        Ok(Flow::Continue)
    }
}

#[tokio::test]
async fn a_hook_that_reroutes_across_the_kinds_still_gets_the_right_answer() {
    // The bridge reads the kind before the request hooks run, so a hook that
    // rewrites the path can make it wrong in both directions. Neither
    // misprediction may change the answer: the thread it costs is the whole
    // of what it costs.
    let app = AppBuilder::new()
        .plugin_hooking(Reroute::new(()))
        .build()
        .expect("the application builds");
    let mut builder: RouterBuilder<AnyHandler<Base>> = Router::builder();
    builder
        // `/front/sync` is predicted sync and lands on a route that waits.
        .route(
            "/front/sync",
            Verb::Get,
            AnyHandler::Sync(Box::new(|_: &mut RouteCx<'_, Base>| {
                panic!("the hook should have rewritten this away")
            })),
        )
        .expect("one route");
    builder
        .route(
            "/back/sync",
            Verb::Get,
            waits(|_: &mut RouteCx<'_, Base>| {
                Box::pin(async move {
                    // On a blocking thread, driven by `block_on`. A Tokio
                    // timer still fires: the runtime is entered there.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    Ok(ok("awaited on the blocking thread"))
                })
            }),
        )
        .expect("another route");
    builder
        // `/front/await` is predicted async and lands on one that does not.
        .route(
            "/front/await",
            Verb::Get,
            waits(|_: &mut RouteCx<'_, Base>| {
                Box::pin(async move { panic!("the hook should have rewritten this away") })
            }),
        )
        .expect("a third route");
    builder
        .route(
            "/back/await",
            Verb::Get,
            AnyHandler::Sync(Box::new(|_: &mut RouteCx<'_, Base>| {
                Ok(ok("called inline"))
            })),
        )
        .expect("a fourth route");
    let router = builder.build().expect("the table builds");
    let server = Running::start(app, router).await;

    let rewritten_to_async = get(&server.addr, "/front/sync").await;
    assert!(
        rewritten_to_async.ends_with("awaited on the blocking thread"),
        "{rewritten_to_async}"
    );
    let rewritten_to_sync = get(&server.addr, "/front/await").await;
    assert!(
        rewritten_to_sync.ends_with("called inline"),
        "{rewritten_to_sync}"
    );
    server.stop().await;
}

// ---------------------------------------------------------------------------
// The measurement, both arms over one table
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fifty_awaiting_core_handlers_cost_no_blocking_thread_at_all() {
    // The twin of `fifty_blocking_core_handlers_do_not_block_the_two_async_workers`
    // above, and of `fifty_waiting_handlers_do_not_cost_fifty_threads` in
    // `serves.rs`. Those two bracket the claim this one proves: the first says
    // the bridge can hold fifty synchronous handlers at once *by spending
    // fifty OS threads*, the second says the socket alone needs none of them.
    // This says the bridge needs none of them either, once the route can wait.
    //
    // Both routes are in one table, so nothing but the handler's kind differs:
    //
    //   /wait   sync, `std::thread::sleep`  -> one OS thread per request
    //   /await  async, `tokio::time::sleep` -> one task per request
    //
    // The assertion is a thread count, not a stopwatch. Fifty handlers in
    // flight at once across two threads cannot be fifty threads whatever the
    // machine was doing at the time.
    const REQUESTS: usize = 50;
    const WAIT: Duration = Duration::from_millis(100);
    const WORKERS: usize = 2;

    let sync = Arc::new(Probe::default());
    let waiting = Arc::new(Probe::default());

    let (sync_h, waiting_h) = (Arc::clone(&sync), Arc::clone(&waiting));
    let (app, router) = awaiting_application(move |b| {
        b.route(
            "/wait",
            Verb::Get,
            AnyHandler::Sync(Box::new(move |_: &mut RouteCx<'_, Base>| {
                sync_h.enter();
                std::thread::sleep(WAIT);
                sync_h.leave();
                Ok(ok("waited"))
            })),
        )
        .expect("one route");
        b.route(
            "/await",
            Verb::Get,
            waits(move |_: &mut RouteCx<'_, Base>| {
                let probe = Arc::clone(&waiting_h);
                Box::pin(async move {
                    probe.enter();
                    tokio::time::sleep(WAIT).await;
                    probe.leave();
                    Ok(ok("awaited"))
                })
            }),
        )
        .expect("another route");
    });
    let server = Running::start(app, router).await;

    async fn fifty(addr: &str, path: &'static str) -> Duration {
        let start = Instant::now();
        let mut inflight = Vec::with_capacity(REQUESTS);
        for _ in 0..REQUESTS {
            let addr = addr.to_string();
            inflight.push(tokio::spawn(async move { get(&addr, path).await }));
        }
        for task in inflight {
            let response = task.await.expect("request task");
            assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        }
        start.elapsed()
    }

    let awaited = fifty(&server.addr, "/await").await;
    let blocked = fifty(&server.addr, "/wait").await;

    assert_eq!(waiting.entered(), REQUESTS);
    assert_eq!(sync.entered(), REQUESTS);

    // The claim, structurally. Fifty handlers overlapped; they used as many
    // threads as the runtime has workers, which is to say none of their own.
    assert!(
        waiting.peak() >= REQUESTS / 2,
        "only {} of {REQUESTS} awaiting handlers were in flight at once, so \
         they did not overlap and the thread count below proves nothing",
        waiting.peak()
    );
    assert!(
        waiting.threads() <= WORKERS,
        "{REQUESTS} awaiting handlers ran on {} threads, with only {WORKERS} \
         workers, so the bridge spent blocking threads on a route that waits",
        waiting.threads()
    );
    assert!(
        awaited < WAIT * 4,
        "{REQUESTS} concurrent {WAIT:?} awaiting handlers took {awaited:?}"
    );

    // The other arm, unchanged and unimproved: a thread each.
    assert!(
        sync.threads() >= REQUESTS / 2,
        "{REQUESTS} synchronous handlers ran on {} threads; they are supposed \
         to cost one each",
        sync.threads()
    );
    assert!(
        blocked < WAIT * 4,
        "{REQUESTS} concurrent {WAIT:?} synchronous handlers took {blocked:?}"
    );

    println!(
        "async: {REQUESTS} x {WAIT:?} in {awaited:?}, peak {} concurrent on \
         {} thread(s)\n sync: {REQUESTS} x {WAIT:?} in {blocked:?}, peak {} \
         concurrent on {} thread(s)",
        waiting.peak(),
        waiting.threads(),
        sync.peak(),
        sync.threads(),
    );
    server.stop().await;
}

// ---------------------------------------------------------------------------
// The lifecycle around the socket
// ---------------------------------------------------------------------------

/// Counts the two lifecycle calls the server owns.
#[derive(Default)]
struct Counts {
    started: AtomicUsize,
    stopped: AtomicUsize,
}

struct Life(Arc<Counts>);

impl Plugin for Life {
    type Config = Arc<Counts>;
    type Provides = NoContext;
    const NAME: &'static str = "life";

    fn new(config: Arc<Counts>) -> Self {
        Self(config)
    }

    fn start(&mut self, _lc: &mut brocanth::plugin_api::Lifecycle<'_>) -> Result<(), PluginError> {
        self.0.started.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn shutdown(
        &mut self,
        _sd: &mut brocanth::plugin_shutdown::ShutdownCx<'_>,
    ) -> Result<(), PluginError> {
        self.0.stopped.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn the_application_starts_before_the_first_request_and_stops_after_the_last() {
    // `Plugin::start` is "the socket is open", which is a fact only the server
    // has, and `App::shutdown` needs `&mut App` back from the `Arc` the
    // serving closure shared. Both are the bridge's job, and the second one is
    // a race with the last connection task, so it is measured rather than
    // assumed.
    let counts = Arc::new(Counts::default());
    let app = AppBuilder::new()
        .plugin(Life::new(Arc::clone(&counts)))
        .build()
        .expect("the application builds");
    let mut builder: brocanth::routing::RouterBuilder<Handler<Base>> = Router::builder();
    builder
        .route(
            "/",
            Verb::Get,
            Box::new(|_: &mut RouteCx<'_, Base>| Ok(ok("up"))),
        )
        .expect("one route");
    let router = builder.build().expect("the table builds");

    let server = Running::start(app, router).await;
    assert!(get(&server.addr, "/").await.ends_with("up"));
    assert_eq!(
        counts.started.load(Ordering::SeqCst),
        1,
        "a request was answered by an application that never started"
    );

    server.stop().await;
    assert_eq!(
        counts.stopped.load(Ordering::SeqCst),
        1,
        "serving ended and the plugins were never stopped"
    );
}
