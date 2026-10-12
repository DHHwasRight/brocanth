//! The server, over a real socket.
//!
//! The unit tests in `src/lib.rs` cover what needs no network. These bind an
//! ephemeral port, speak HTTP/1.1 at it with a `TcpStream` and read the bytes
//! back, which is the only way to prove the framing is right. Writing the
//! requests by hand rather than pulling in an HTTP client keeps the
//! dev-dependency list at one.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use brocanth::routing::Verb;
use brocanth_server::{Request, Response, Server, Stopper};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A server running as a task, and the address to reach it at.
struct Running {
    addr: String,
    stopper: Stopper,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start<F, Fut>(handle: F) -> Self
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Response> + Send + 'static,
    {
        let server = Server::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral port");
        let addr = server.local_addr().expect("local addr").to_string();
        let stopper = server.stopper();
        let task = tokio::spawn(async move {
            server.run(handle).await.expect("the server ran");
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

/// Sends a raw request and returns the whole response, headers included.
async fn send(addr: &str, raw: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(raw.as_bytes()).await.expect("write");
    stream.flush().await.expect("flush");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.expect("read");
    String::from_utf8_lossy(&response).into_owned()
}

async fn get(addr: &str, path: &str) -> String {
    send(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"),
    )
    .await
}

#[tokio::test]
async fn it_answers_a_get_with_the_body_the_handler_returned() {
    let server = Running::start(|request: Request| async move {
        Response::text(200, format!("you asked for {}", request.path))
    })
    .await;
    let response = get(&server.addr, "/hello").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(
        response.contains("content-type: text/plain; charset=utf-8"),
        "{response}"
    );
    assert!(response.ends_with("you asked for /hello"), "{response}");
    server.stop().await;
}

#[tokio::test]
async fn the_query_string_is_split_off_the_path() {
    // A router matching `/users/7?tab=logs` would match nothing, so the split
    // has to happen before the application ever sees the path.
    let server = Running::start(|request: Request| async move {
        Response::text(
            200,
            format!("path={} query={}", request.path, request.query),
        )
    })
    .await;
    let response = get(&server.addr, "/users/7?tab=logs&page=2").await;
    assert!(
        response.ends_with("path=/users/7 query=tab=logs&page=2"),
        "{response}"
    );
    server.stop().await;
}

#[tokio::test]
async fn a_path_with_no_query_gets_an_empty_one_rather_than_a_question_mark() {
    let server = Running::start(|request: Request| async move {
        Response::text(200, format!("[{}]", request.query))
    })
    .await;
    assert!(get(&server.addr, "/plain").await.ends_with("[]"));
    server.stop().await;
}

#[tokio::test]
async fn head_sends_the_headers_and_the_length_but_no_body() {
    // A page answers HEAD correctly by answering GET at all: the router is
    // asked for GET and the body is dropped on the way out.
    let server = Running::start(|_| async { Response::text(200, "0123456789") }).await;
    let response = send(
        &server.addr,
        "HEAD / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("content-length: 10"), "{response}");
    assert!(
        !response.contains("0123456789"),
        "a HEAD sent a body:\n{response}"
    );
    server.stop().await;
}

#[tokio::test]
async fn a_post_body_reaches_the_handler() {
    let server = Running::start(|request: Request| async move {
        Response::text(
            200,
            format!(
                "{} bytes: {}",
                request.body.len(),
                String::from_utf8_lossy(&request.body)
            ),
        )
    })
    .await;
    let response = send(
        &server.addr,
        "POST /submit HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
    )
    .await;
    assert!(response.ends_with("5 bytes: hello"), "{response}");
    server.stop().await;
}

#[tokio::test]
async fn a_verb_the_router_has_no_name_for_is_501_not_404() {
    // 404 would be a claim about the path, and the path may be perfectly good.
    let server = Running::start(|_| async { Response::text(200, "never reached") }).await;
    let response = send(
        &server.addr,
        "PROPFIND / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 501"), "{response}");
    assert!(!response.contains("never reached"), "{response}");
    server.stop().await;
}

#[tokio::test]
async fn the_status_and_extra_headers_survive() {
    let server = Running::start(|_| async {
        Response::text(404, "nope")
            .with_header("x-brocanth", "1")
            .with_header("allow", "GET, HEAD")
    })
    .await;
    let response = get(&server.addr, "/missing").await;
    assert!(response.starts_with("HTTP/1.1 404"), "{response}");
    assert!(response.contains("x-brocanth: 1"), "{response}");
    assert!(response.contains("allow: GET, HEAD"), "{response}");
    server.stop().await;
}

#[tokio::test]
async fn the_handler_sees_the_verb_the_router_uses() {
    let server =
        Running::start(
            |request: Request| async move { Response::text(200, request.verb.as_str()) },
        )
        .await;
    for (sent, expected) in [
        ("GET", Verb::Get),
        ("DELETE", Verb::Delete),
        ("PATCH", Verb::Patch),
    ] {
        let response = send(
            &server.addr,
            &format!("{sent} / HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(response.ends_with(expected.as_str()), "{sent}: {response}");
    }
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fifty_waiting_handlers_do_not_cost_fifty_threads() {
    // The M:N claim, as an assertion rather than a sentence.
    //
    // Fifty requests arrive at once, each awaiting 100ms. The runtime has two
    // worker threads. A thread-per-request server would need fifty threads to
    // finish in ~100ms, and a pool of two would take ~2.5 seconds. Tokio
    // parks fifty *tasks* on two threads, so the whole set lands in roughly
    // the time of one -- which is the entire reason for choosing it.
    const REQUESTS: usize = 50;
    const WAIT: Duration = Duration::from_millis(100);

    let entered = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&entered);
    let server = Running::start(move |_| {
        let counter = Arc::clone(&counter);
        async move {
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(WAIT).await;
            Response::text(200, "waited")
        }
    })
    .await;

    let start = Instant::now();
    let mut inflight = Vec::with_capacity(REQUESTS);
    for i in 0..REQUESTS {
        let addr = server.addr.clone();
        inflight.push(tokio::spawn(
            async move { get(&addr, &format!("/{i}")).await },
        ));
    }
    for task in inflight {
        let response = task.await.expect("request task");
        assert!(response.ends_with("waited"), "{response}");
    }
    let elapsed = start.elapsed();

    assert_eq!(entered.load(Ordering::SeqCst), REQUESTS);
    assert!(
        elapsed < WAIT * 4,
        "{REQUESTS} concurrent {WAIT:?} handlers took {elapsed:?} on 2 worker \
         threads, so they were not overlapping"
    );
    server.stop().await;
}

#[tokio::test]
async fn stopping_lets_run_return() {
    let server = Running::start(|_| async { Response::text(200, "") }).await;
    // A request first, so the stop happens against a server that has actually
    // served something rather than one still warming up.
    assert!(get(&server.addr, "/").await.starts_with("HTTP/1.1 200"));
    // `stop` consumes and joins; a hang here is the test failing.
    tokio::time::timeout(Duration::from_secs(5), server.stop())
        .await
        .expect("run returned after stop");
}

/// Sends a raw request, tolerating a reset.
///
/// A server that rejects a body mid-upload answers and closes while the client
/// is still writing, so the write or the read may legitimately fail with
/// `ConnectionReset`. That is a correct outcome, not a test failure.
async fn send_tolerantly(addr: &str, raw: &str) -> String {
    let Ok(mut stream) = TcpStream::connect(addr).await else {
        return String::new();
    };
    let _ = stream.write_all(raw.as_bytes()).await;
    let _ = stream.flush().await;
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response).await;
    String::from_utf8_lossy(&response).into_owned()
}

#[tokio::test]
async fn a_body_over_the_cap_never_reaches_the_handler() {
    // The invariant worth asserting is not the status -- the client may be
    // reset before it reads one -- but that the handler was never entered.
    // Handing it the first two megabytes would let it act on a request nobody
    // sent, and handing it an empty body is worse: that is indistinguishable
    // from a legitimate empty POST.
    const NEVER_CALLED: usize = usize::MAX;
    let seen = Arc::new(AtomicUsize::new(NEVER_CALLED));
    let recorder = Arc::clone(&seen);
    let server = Running::start(move |request: Request| {
        let recorder = Arc::clone(&recorder);
        async move {
            recorder.store(request.body.len(), Ordering::SeqCst);
            Response::text(200, "ok")
        }
    })
    .await;

    let oversized = 3 * 1024 * 1024;
    let mut raw = format!(
        "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {oversized}\r\nConnection: close\r\n\r\n"
    );
    raw.push_str(&"x".repeat(oversized));
    let response = send_tolerantly(&server.addr, &raw).await;

    assert_eq!(
        seen.load(Ordering::SeqCst),
        NEVER_CALLED,
        "the handler was entered for a body past the cap"
    );
    // When a response did get through, it must say why rather than pretend.
    if !response.is_empty() {
        assert!(response.starts_with("HTTP/1.1 413"), "{response}");
    }
    server.stop().await;
}

#[tokio::test]
async fn a_body_under_the_cap_arrives_whole() {
    // The other side of the cap, and the one that would break silently if the
    // limit were ever set from the wrong units.
    let server = Running::start(|request: Request| async move {
        Response::text(200, format!("{}", request.body.len()))
    })
    .await;

    let size = 1024 * 1024;
    let mut raw = format!(
        "POST / HTTP/1.1\r\nHost: x\r\nContent-Length: {size}\r\nConnection: close\r\n\r\n"
    );
    raw.push_str(&"y".repeat(size));
    let response = send(&server.addr, &raw).await;
    assert!(response.ends_with(&size.to_string()), "{response}");
    server.stop().await;
}
