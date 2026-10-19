//! The evidence that a streaming body streams.
//!
//! A type that compiles is not evidence. What this file proves, over a real
//! socket, is the one property the whole change exists for:
//!
//! > **The first bytes reach the client before the last are produced.**
//!
//! The instrument is a stream that will not produce its last chunk until the
//! test says so, and a flag the stream sets when it does. The test reads the
//! first chunk off the socket, then asserts the flag is still false. If the
//! server buffered, the read would block until the last chunk existed and the
//! flag would be true by the time anything arrived.
//!
//! The second half of the file is the other direction: a shutdown reaching a
//! body that would otherwise never end. ADR 2's `Begin` closes the gate, the
//! stream sends a last frame and ends, and the server stops. Without it the
//! deploy waits for the longest-lived subscriber, which is the failure this
//! is designed against.
//!
//! Requests are written by hand with a `TcpStream`, as `serves.rs` does it
//! and for the same reason.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use brocanth::plugin_body::{BodyStream, Chunk, StreamError};
use brocanth_server::{Request, Response, Server, Stopper};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

// ---------------------------------------------------------------------------
// The instrument
// ---------------------------------------------------------------------------

/// What a test can see and do to the stream it started.
#[derive(Clone, Default)]
struct Control(Arc<Shared>);

#[derive(Default)]
struct Shared {
    /// The test has allowed the next chunk.
    released: AtomicBool,
    /// The stream has produced the chunk after the first.
    produced_last: AtomicBool,
    /// `finish` was called: the application is stopping.
    told_to_finish: AtomicBool,
    /// How many chunks the stream has handed over.
    chunks: AtomicUsize,
    waker: Mutex<Option<Waker>>,
}

impl Control {
    /// Lets the stream produce its last chunk.
    fn release(&self) {
        self.0.released.store(true, Ordering::SeqCst);
        if let Some(waker) = self.0.waker.lock().expect("not poisoned").take() {
            waker.wake();
        }
    }

    fn produced_last(&self) -> bool {
        self.0.produced_last.load(Ordering::SeqCst)
    }

    fn told_to_finish(&self) -> bool {
        self.0.told_to_finish.load(Ordering::SeqCst)
    }

    fn chunks(&self) -> usize {
        self.0.chunks.load(Ordering::SeqCst)
    }
}

/// Sends `first`, then waits for the test, then sends `last` and ends.
///
/// Told to finish, it sends `goodbye` instead of waiting any longer -- which
/// is what an SSE endpoint does with `event: close`.
struct Gated {
    control: Control,
    stage: Stage,
}

#[derive(PartialEq, Eq)]
enum Stage {
    First,
    Waiting,
    Done,
}

impl BodyStream for Gated {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk {
        match self.stage {
            Stage::First => {
                self.stage = Stage::Waiting;
                self.control.0.chunks.fetch_add(1, Ordering::SeqCst);
                Poll::Ready(Some(Ok(b"first\n".to_vec())))
            }
            Stage::Waiting => {
                if self.control.0.told_to_finish.load(Ordering::SeqCst) {
                    self.stage = Stage::Done;
                    self.control.0.chunks.fetch_add(1, Ordering::SeqCst);
                    return Poll::Ready(Some(Ok(b"goodbye\n".to_vec())));
                }
                if !self.control.0.released.load(Ordering::SeqCst) {
                    *self.control.0.waker.lock().expect("not poisoned") = Some(cx.waker().clone());
                    return Poll::Pending;
                }
                self.stage = Stage::Done;
                self.control.0.produced_last.store(true, Ordering::SeqCst);
                self.control.0.chunks.fetch_add(1, Ordering::SeqCst);
                Poll::Ready(Some(Ok(b"last\n".to_vec())))
            }
            Stage::Done => Poll::Ready(None),
        }
    }

    fn finish(&mut self) {
        self.control.0.told_to_finish.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Running one
// ---------------------------------------------------------------------------

struct Running {
    addr: String,
    stopper: Stopper,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start(control: Control, grace: Option<Duration>) -> Self {
        let mut server = Server::bind("127.0.0.1:0")
            .await
            .expect("an ephemeral port");
        if let Some(grace) = grace {
            server = server.drain_grace(grace);
        }
        let addr = server.local_addr().expect("local addr").to_string();
        let stopper = server.stopper();
        let task = tokio::spawn(async move {
            server
                .run(move |_: Request| {
                    let control = control.clone();
                    async move {
                        Response::stream(
                            200,
                            "text/plain; charset=utf-8",
                            Gated {
                                control,
                                stage: Stage::First,
                            },
                        )
                    }
                })
                .await
                .expect("the server ran");
        });
        Self {
            addr,
            stopper,
            task,
        }
    }
}

/// Opens a connection and sends the request line, without reading anything.
async fn request(addr: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(b"GET /feed HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    stream.flush().await.expect("flush");
    stream
}

/// Reads until `needle` is in what has arrived, or gives up.
///
/// The timeout is a failure, not a wait: the bytes this looks for are either
/// already on their way or they are being held back by a server that
/// buffered, and the test wants to say which.
async fn read_until(stream: &mut TcpStream, seen: &mut Vec<u8>, needle: &str) {
    let deadline = Duration::from_secs(5);
    let found = tokio::time::timeout(deadline, async {
        loop {
            if String::from_utf8_lossy(seen).contains(needle) {
                return;
            }
            let mut buf = [0u8; 256];
            match stream.read(&mut buf).await {
                Ok(0) => return,
                Ok(n) => seen.extend_from_slice(&buf[..n]),
                Err(e) => panic!("read: {e}"),
            }
        }
    })
    .await;
    assert!(
        found.is_ok(),
        "nothing carrying {needle:?} arrived within {deadline:?}; what did arrive was:\n{}",
        String::from_utf8_lossy(seen)
    );
    assert!(
        String::from_utf8_lossy(seen).contains(needle),
        "the connection closed before {needle:?} arrived:\n{}",
        String::from_utf8_lossy(seen)
    );
}

// ---------------------------------------------------------------------------
// The claim
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_first_chunk_reaches_the_client_before_the_last_is_produced() {
    let control = Control::default();
    let server = Running::start(control.clone(), None).await;

    let mut stream = request(&server.addr).await;
    let mut seen = Vec::new();

    // The first chunk, with the last one still unwritten and unwritable.
    read_until(&mut stream, &mut seen, "first").await;

    assert!(
        !control.produced_last(),
        "the client has the first chunk and the handler has not produced the \
         last one -- unless the server buffered, in which case this is the end \
         of a complete body"
    );
    assert_eq!(
        control.chunks(),
        1,
        "exactly one chunk has been handed over"
    );

    let head = String::from_utf8_lossy(&seen).to_lowercase();
    assert!(head.starts_with("http/1.1 200"), "{head}");
    assert!(
        head.contains("transfer-encoding: chunked"),
        "a body of unknown length has to be framed chunked:\n{head}"
    );
    assert!(
        !head.contains("content-length"),
        "a stream cannot know its length, so it must not claim one:\n{head}"
    );

    // Now let the rest out.
    control.release();
    read_until(&mut stream, &mut seen, "last").await;
    assert!(control.produced_last());

    let whole = String::from_utf8_lossy(&seen).into_owned();
    assert!(
        whole.find("first").unwrap() < whole.find("last").unwrap(),
        "chunks arrived out of order:\n{whole}"
    );

    drop(stream);
    server.stopper.stop();
    let _ = tokio::time::timeout(Duration::from_secs(5), server.task).await;
}

#[tokio::test]
async fn a_second_request_is_served_while_the_first_is_still_streaming() {
    // The same claim from the other side: a body that has not finished must
    // not hold the server. If the first response were buffered, the handler
    // for it would still be running and this would time out.
    let control = Control::default();
    let server = Running::start(control.clone(), None).await;

    let mut first = request(&server.addr).await;
    let mut seen = Vec::new();
    read_until(&mut first, &mut seen, "first").await;

    let mut second = request(&server.addr).await;
    let mut other = Vec::new();
    read_until(&mut second, &mut other, "first").await;

    assert!(!control.produced_last(), "neither stream has finished");

    control.release();
    drop(first);
    drop(second);
    server.stopper.stop();
    let _ = tokio::time::timeout(Duration::from_secs(5), server.task).await;
}

// ---------------------------------------------------------------------------
// ADR 2, one layer down
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stopping_tells_an_open_stream_to_finish_and_the_server_stops() {
    let control = Control::default();
    let server = Running::start(control.clone(), None).await;

    let mut stream = request(&server.addr).await;
    let mut seen = Vec::new();
    read_until(&mut stream, &mut seen, "first").await;

    assert!(!control.told_to_finish());

    // Nothing will ever release this stream, so without the gate the server
    // would wait for it for ever.
    server.stopper.stop();

    read_until(&mut stream, &mut seen, "goodbye").await;
    assert!(
        control.told_to_finish(),
        "`finish` is how a stream learns the application is stopping"
    );
    assert!(
        !control.produced_last(),
        "the stream ended on its own terms rather than being released"
    );

    let stopped = tokio::time::timeout(Duration::from_secs(5), server.task).await;
    assert!(
        stopped.is_ok(),
        "the server did not stop: an open stream held the drain"
    );
    stopped.expect("timed out").expect("the server task");
}

#[tokio::test]
async fn a_stream_that_ignores_finish_is_dropped_when_the_budget_runs_out() {
    /// Never ends, and does not implement `finish`. The stream this is
    /// designed against.
    struct Deaf;

    impl BodyStream for Deaf {
        fn poll_chunk(&mut self, _cx: &mut Context<'_>) -> Chunk {
            // Parked for ever: no waker is stored anywhere, so nothing but
            // the budget can end this.
            Poll::Pending
        }
    }

    let server = Server::bind("127.0.0.1:0")
        .await
        .expect("an ephemeral port")
        .drain_grace(Duration::from_millis(200));
    let addr = server.local_addr().expect("local addr").to_string();
    let stopper = server.stopper();
    let task = tokio::spawn(async move {
        server
            .run(|_: Request| async { Response::stream(200, "text/plain", Deaf) })
            .await
            .expect("the server ran");
    });

    let mut stream = request(&addr).await;
    let mut seen = Vec::new();
    read_until(&mut stream, &mut seen, "200").await;

    stopper.stop();
    let stopped = tokio::time::timeout(Duration::from_secs(5), task).await;
    assert!(
        stopped.is_ok(),
        "the budget did not expire: a deaf stream held the deploy"
    );
    stopped.expect("timed out").expect("the server task");
}

// ---------------------------------------------------------------------------
// The error path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_stream_that_fails_halfway_stops_writing_rather_than_answering_500() {
    /// One chunk, a pause long enough for it to be flushed, then a failure.
    /// The status is long gone by then.
    struct Breaks(u8);

    impl BodyStream for Breaks {
        fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk {
            match self.0 {
                0 => {
                    self.0 = 1;
                    Poll::Ready(Some(Ok(b"half\n".to_vec())))
                }
                1 => {
                    // The pause is the point: without it hyper can learn of
                    // the failure before it has written anything, and the
                    // client sees nothing rather than half.
                    self.0 = 2;
                    let waker = cx.waker().clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        waker.wake();
                    });
                    Poll::Pending
                }
                _ => Poll::Ready(Some(Err(StreamError::new("the row reader gave up")))),
            }
        }
    }

    let server = Server::bind("127.0.0.1:0")
        .await
        .expect("an ephemeral port");
    let addr = server.local_addr().expect("local addr").to_string();
    let stopper = server.stopper();
    let task = tokio::spawn(async move {
        server
            .run(|_: Request| async { Response::stream(200, "text/plain", Breaks(0)) })
            .await
            .expect("the server ran");
    });

    let mut stream = request(&addr).await;
    let mut seen = Vec::new();
    read_until(&mut stream, &mut seen, "half").await;

    let whole = String::from_utf8_lossy(&seen).into_owned();
    assert!(
        whole.starts_with("HTTP/1.1 200"),
        "the status went out with the headers, before anything could fail:\n{whole}"
    );
    assert!(
        !whole.contains("500"),
        "there is no clean 500 once the headers are on the wire:\n{whole}"
    );

    drop(stream);
    stopper.stop();
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
}
