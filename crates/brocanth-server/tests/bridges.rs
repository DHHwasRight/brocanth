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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::{App, AppBuilder};
use brocanth::plugin_context::{Base, NoContext};
use brocanth::plugin_hooks::{
    HookFamilies, HookRegistrar, HookSource, OnResponse, Request as CoreRequest, RequestState,
    Response as CoreResponse, ResponseHook,
};
use brocanth::plugin_order::Order;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_server::bridge::Bridge;
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
    fn on_response(
        &self,
        _request: &CoreRequest,
        response: &mut CoreResponse,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let status = response.status().as_u16();
        response.headers_mut().insert(
            "x-pipeline",
            http::HeaderValue::from_str(&status.to_string()).expect("a status is ASCII digits"),
        );
        Ok(())
    }
}

fn ok(body: impl Into<Vec<u8>>) -> CoreResponse {
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

// ---------------------------------------------------------------------------
// Running one, over a real port
// ---------------------------------------------------------------------------

struct Running {
    addr: String,
    stopper: Stopper,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start<E: 'static, H: HookFamilies>(
        app: App<E, H>,
        router: Router<Handler<E>>,
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
                    .body(b"short".to_vec())
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
