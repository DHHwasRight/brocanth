//! An HTTP entrypoint for a brocanth application, on Tokio and hyper.
//!
//! # What this crate does not know
//!
//! It does not know what a `Ctx` is, what a `Page` is, that a router exists,
//! or that middleware exists. Those types belong to the application -- the
//! scaffold writes them into `src/pages/mod.rs` and the core is generic over
//! them -- so a server crate naming them would be deciding something the
//! application is supposed to decide.
//!
//! What it takes is one async function: given a [`Request`], return a
//! [`Response`]. Routing, the middleware chain, the 404 and the 405 all live
//! on the far side of it, in `main`, where a reader can see them. This crate
//! owns sockets, tasks and HTTP framing, and nothing else. hyper's types do
//! not appear in its public API either, so swapping the implementation would
//! not reach an application.
//!
//! ```no_run
//! use brocanth_server::{Response, Server};
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     Server::bind("127.0.0.1:3000")
//!         .await?
//!         .run(|request| async move {
//!             Response::text(200, format!("you asked for {}", request.path))
//!         })
//!         .await?;
//!     Ok(())
//! }
//! ```
//!
//! # M:N, and where it currently stops
//!
//! A connection costs a task rather than an OS thread, so ten thousand idle
//! keep-alives are affordable and Tokio's work-stealing scheduler spreads
//! handlers over the cores by itself.
//!
//! The honest limit: [`brocanth::routing::MiddlewareStack::run`] is still
//! synchronous, so a handler reached *through the chain* cannot yet `.await`.
//! Until the core grows an async chain, handler bodies occupy their worker
//! while they compute -- which is correct for CPU work and wrong for a
//! database call. The server is async all the way to the application's door;
//! the last few metres are the core's next job.
//!
//! # The core, when it is asked for
//!
//! The function above is the whole contract, and an application is free to
//! write the far side of it by hand. The `bridge` feature writes it instead:
//! [`bridge::serve`] takes an `App` and a `Router` and does the conversion,
//! the dispatch and the 404. It is off by default, so with the feature off
//! this crate is exactly what it was -- a socket and one function -- and the
//! module is not compiled at all.
//!
//! The limit above is what the bridge has to answer, and it answers it with
//! `tokio::task::spawn_blocking`: the core runs on a blocking thread so the
//! async workers stay free. The cost is one thread per concurrent request,
//! stated and measured in [`bridge`].
//!
//! A route that *can* wait does not pay it. The core's route table holds
//! handlers of two kinds, and the bridge reads the kind before it touches the
//! handler: a route that waits is awaited inline and costs a task, a route
//! that cannot is sent to a blocking thread as before. Fifty concurrent 100 ms
//! handlers are fifty OS threads in the second case and two in the first.
//! [`bridge`] has both numbers, what the inline arm does about a panicking
//! handler, and what that does not cover.

#[cfg(feature = "bridge")]
pub mod bridge;

use std::convert::Infallible;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use brocanth::plugin_body::{BodyStream, StreamError, StreamGate, Streaming};
use brocanth::routing::Verb;
use bytes::Bytes;
use http_body_util::BodyExt;
use hyper::body::{Frame, Incoming, SizeHint};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tokio::sync::Notify;

/// The most of a request body this server will hold in memory.
///
/// Stated rather than unbounded: without a cap, one client announcing a large
/// `Content-Length` is all it takes to exhaust the process.
const MAX_BODY: usize = 2 * 1024 * 1024;

/// Why a server could not start, or could not keep running.
#[derive(Debug)]
pub enum ServeError {
    /// The address was taken, malformed, or not ours to bind.
    Bind { addr: String, reason: String },
    /// Accepting a connection failed in a way that is not transient.
    Accept(std::io::Error),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind { addr, reason } => write!(f, "cannot bind {addr}: {reason}"),
            Self::Accept(e) => write!(f, "cannot accept a connection: {e}"),
        }
    }
}

impl std::error::Error for ServeError {}

/// One request, already parsed.
pub struct Request {
    /// The verb, as the router spells it.
    pub verb: Verb,
    /// The path alone. The query string is split off into [`Request::query`],
    /// because a router matching `/users/7?tab=logs` would match nothing.
    pub path: String,
    /// Everything after the first `?`, without it. Empty when there was none.
    pub query: String,
    /// The body, up to [`MAX_BODY`]. Empty for the verbs that carry none.
    pub body: Bytes,
    /// Who connected. `None` behind a proxy that did not say.
    pub peer: Option<SocketAddr>,
    headers: Vec<(String, String)>,
}

impl Request {
    /// A header by name, case-insensitively, or `None`.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Every header, in the order they arrived.
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }
}

/// A body on its way out: every byte, or a promise of them.
///
/// The streaming half is [`brocanth::plugin_body::Streaming`] rather than a
/// type of this crate's own, because the core already owns the contract --
/// what a chunk is, and what a shutdown means to a body that has not finished
/// -- and two spellings of it would drift. hyper is still nowhere in this
/// crate's public API.
pub enum Body {
    /// Every byte. Framed with a `content-length`.
    Bytes(Bytes),
    /// Bytes that do not exist yet. Framed `chunked`, because the length is
    /// unknown by definition.
    Stream(Streaming),
}

impl Body {
    /// How many bytes, when that is knowable. [`None`] for a stream.
    pub fn len(&self) -> Option<usize> {
        match self {
            Self::Bytes(bytes) => Some(bytes.len()),
            Self::Stream(_) => None,
        }
    }

    /// Whether this body is known to carry no bytes. `false` for a stream.
    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }

    /// The bytes, when there are bytes. [`None`] for a stream.
    pub fn bytes(&self) -> Option<&Bytes> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            Self::Stream(_) => None,
        }
    }
}

impl<T: Into<Bytes>> From<T> for Body {
    fn from(bytes: T) -> Self {
        Self::Bytes(bytes.into())
    }
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bytes(bytes) => write!(f, "Bytes({} bytes)", bytes.len()),
            Self::Stream(_) => f.write_str("Stream(..)"),
        }
    }
}

/// What to send back.
pub struct Response {
    pub status: u16,
    pub body: Body,
    headers: Vec<(String, String)>,
}

impl Response {
    /// A header by name, case-insensitively, or `None`.
    ///
    /// The same reader [`Request::header`] has. A response is read as well
    /// as written: a hook checks what an earlier one set, and a test checks
    /// what the handler sent.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// Every header, in the order they were added.
    pub fn headers(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    /// A response with a body and an explicit content type.
    pub fn new(status: u16, content_type: &str, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            body: Body::Bytes(body.into()),
            headers: vec![("content-type".to_string(), content_type.to_string())],
        }
    }

    /// A response whose body is written as it is produced.
    ///
    /// The first bytes reach the client before the last are made, which is
    /// what SSE, a streamed shell and a large download all need. There is no
    /// `content-length` and no `ETag`: neither is knowable here.
    ///
    /// A stream that may live a long time should implement
    /// [`BodyStream::finish`], or a deploy waits for it. See
    /// [`Server::drain_grace`].
    pub fn stream(status: u16, content_type: &str, stream: impl BodyStream) -> Self {
        Self {
            status,
            body: Body::Stream(Streaming::new(stream)),
            headers: vec![("content-type".to_string(), content_type.to_string())],
        }
    }

    /// `text/event-stream`, with the two headers every SSE client needs:
    /// nothing may cache it and nothing may buffer it.
    pub fn events(stream: impl BodyStream) -> Self {
        Self::stream(200, "text/event-stream", stream)
            .with_header("cache-control", "no-cache")
            .with_header("x-accel-buffering", "no")
    }

    /// `text/plain; charset=utf-8`.
    pub fn text(status: u16, body: impl Into<Bytes>) -> Self {
        Self::new(status, "text/plain; charset=utf-8", body)
    }

    /// `text/html; charset=utf-8`.
    pub fn html(status: u16, body: impl Into<Bytes>) -> Self {
        Self::new(status, "text/html; charset=utf-8", body)
    }

    /// `application/json`. The body is sent as given; this crate has no JSON
    /// encoder and does not want one.
    pub fn json(status: u16, body: impl Into<Bytes>) -> Self {
        Self::new(status, "application/json", body)
    }

    /// Adds a header. Repeat for a header that may appear more than once.
    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Drops every header with this name, ignoring case.
    ///
    /// The counterpart of [`with_header`](Self::with_header), which appends
    /// because `set-cookie` has to repeat. A single-valued header does not:
    /// two `content-type` lines are not an override, they are two claims
    /// and the client picks. A caller replacing one drops it first.
    #[must_use]
    pub fn without_header(mut self, name: &str) -> Self {
        self.headers
            .retain(|(held, _)| !held.eq_ignore_ascii_case(name));
        self
    }
}

/// Stops a running server.
///
/// Cloneable and `Send`, so it can go to a signal handler or a test. Stopping
/// closes the listener and lets connections already in flight finish.
#[derive(Clone)]
pub struct Stopper(Arc<Notify>);

impl Stopper {
    /// Asks the accept loop to stop. `run` returns once in-flight connections
    /// have drained.
    pub fn stop(&self) {
        // `notify_waiters` would be lost if `run` has not reached its select
        // yet; the permit `notify_one` stores survives that race.
        self.0.notify_one();
    }
}

/// A bound listener, waiting to be given a handler.
pub struct Server {
    listener: TcpListener,
    stop: Arc<Notify>,
    streams: StreamGate,
    drain: Option<Duration>,
}

impl Server {
    /// Binds, without accepting anything yet.
    ///
    /// Binding here rather than in `run` is deliberate: "the port is taken" is
    /// something an application wants to report *before* it announces that it
    /// started.
    pub async fn bind(addr: &str) -> Result<Self, ServeError> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| ServeError::Bind {
                addr: addr.to_string(),
                reason: e.to_string(),
            })?;
        Ok(Self {
            listener,
            stop: Arc::new(Notify::new()),
            streams: StreamGate::new(),
            drain: None,
        })
    }

    /// The address actually bound. Port 0 becomes a real port here, which is
    /// how the tests get one without racing for a fixed number.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// A handle that stops this server.
    pub fn stopper(&self) -> Stopper {
        Stopper(Arc::clone(&self.stop))
    }

    /// Every response body this server still has open.
    ///
    /// [`Server::run`] closes it the moment the listener stops, which is what
    /// tells an SSE stream to send its last frame and end. A caller that
    /// wants to close it earlier -- or to read how many are open -- takes a
    /// clone of this.
    pub fn streams(&self) -> &StreamGate {
        &self.streams
    }

    /// Shares the gate with whoever else owns one.
    ///
    /// [`bridge::Bridge::serve`](crate::bridge::Bridge::serve) hands over the
    /// application's own
    /// [`App::streams`](brocanth::plugin_app::App::streams), so that a stop
    /// arriving at the listener and a stop arriving at
    /// [`App::shutdown`](brocanth::plugin_app::App::shutdown) close one gate
    /// rather than two halves of one.
    #[must_use]
    pub fn with_streams(mut self, streams: StreamGate) -> Self {
        self.streams = streams;
        self
    }

    /// How long to wait, after the gate is closed, for the connections still
    /// in flight.
    ///
    /// [`None`] -- the default, and what this server did before it could
    /// stream -- waits for as long as they take. That is right for a request
    /// that is merely slow and wrong for a body that never ends: a stream
    /// that does not implement [`BodyStream::finish`] holds the deploy open
    /// for ever. With a budget, whatever is still open when it expires is
    /// dropped, and those clients see a truncated response.
    #[must_use]
    pub fn drain_grace(mut self, grace: Duration) -> Self {
        self.drain = Some(grace);
        self
    }

    /// Accepts until [`Stopper::stop`], serving each connection as a task.
    ///
    /// `handle` is cloned into every connection, so it is `Fn` rather than
    /// `FnMut`: state shared between requests has to say so in its own type.
    /// Each call returns a future, which is what lets a handler await without
    /// occupying a thread.
    pub async fn run<F, Fut>(self, handle: F) -> Result<(), ServeError>
    where
        F: Fn(Request) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send,
    {
        let handle = Arc::new(handle);
        let builder = auto::Builder::new(TokioExecutor::new());
        // hyper's own drain: it tells each connection to finish -- a GOAWAY on
        // h2, no further keep-alive on h1 -- and resolves when the last one is
        // done. Counting connections by hand would get the protocol half wrong.
        let graceful = GracefulShutdown::new();

        loop {
            let accepted = tokio::select! {
                // Biased so a pending stop wins over a connection that arrived
                // in the same tick; a busy listener could otherwise outrun it.
                biased;
                () = self.stop.notified() => break,
                accepted = self.listener.accept() => accepted,
            };

            let (stream, peer) = match accepted {
                Ok(pair) => pair,
                // One failed handshake says nothing about the listener; a
                // listener that is genuinely broken will fail again.
                Err(e) if transient(&e) => continue,
                Err(e) => return Err(ServeError::Accept(e)),
            };

            let handle = Arc::clone(&handle);
            let builder = builder.clone();
            let watcher = graceful.watcher();
            let streams = self.streams.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req: hyper::Request<Incoming>| {
                    let handle = Arc::clone(&handle);
                    let streams = streams.clone();
                    async move {
                        Ok::<_, Infallible>(exchange(req, peer, handle.as_ref(), &streams).await)
                    }
                });
                let conn = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
                let _ = watcher.watch(conn).await;
            });
        }

        // ADR 2 arriving at the wire. The listener is closed; hyper's drain
        // tells each connection to finish, and this tells each open response
        // body to. Without it a subscriber on an SSE endpoint keeps its
        // connection in flight, hyper's drain waits for the connection, and
        // the deploy waits for the subscriber.
        self.streams.close();

        match self.drain {
            None => graceful.shutdown().await,
            Some(grace) => {
                if tokio::time::timeout(grace, graceful.shutdown())
                    .await
                    .is_err()
                {
                    // Dropping `graceful` drops the watchers, which drops the
                    // connections, which drops the bodies. A client mid-frame
                    // sees a truncated response; the alternative is a deploy
                    // that never finishes.
                    return Ok(());
                }
            }
        }
        Ok(())
    }
}

/// Whether an accept error is worth ignoring.
///
/// These are all "that one client went away", which says nothing about the
/// listener.
fn transient(e: &std::io::Error) -> bool {
    use std::io::ErrorKind::*;
    matches!(
        e.kind(),
        ConnectionAborted | ConnectionReset | Interrupted | WouldBlock | TimedOut
    )
}

/// A body on the way to hyper: one chunk, or a stream of them.
///
/// It exists so that `Full<Bytes>` is not this crate's only answer. hyper
/// asks a body for one `poll_frame`, which is exactly the shape
/// [`BodyStream`] has, so the streaming arm is a forward and nothing else.
pub struct OutBody(Out);

enum Out {
    /// One chunk, or none. `None` after it has been taken.
    Once(Option<Bytes>),
    Stream(Streaming),
}

impl OutBody {
    fn once(bytes: Bytes) -> Self {
        Self(Out::Once(Some(bytes)))
    }

    fn empty() -> Self {
        Self(Out::Once(None))
    }

    fn stream(stream: Streaming) -> Self {
        Self(Out::Stream(stream))
    }
}

impl hyper::body::Body for OutBody {
    type Data = Bytes;
    type Error = StreamError;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        match &mut self.get_mut().0 {
            Out::Once(slot) => Poll::Ready(slot.take().map(|bytes| Ok(Frame::data(bytes)))),
            Out::Stream(stream) => match stream.poll_chunk(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(None) => Poll::Ready(None),
                Poll::Ready(Some(Ok(chunk))) => {
                    Poll::Ready(Some(Ok(Frame::data(Bytes::from(chunk)))))
                }
                // The status and the headers are already on the wire, so
                // there is no clean 500 to be had: hyper stops writing and
                // the client sees a response that ends early. PLAN.md §3
                // buffers by default for exactly this reason.
                Poll::Ready(Some(Err(e))) => Poll::Ready(Some(Err(e))),
            },
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(&self.0, Out::Once(None))
    }

    /// Exact for a buffered body, so hyper writes a `content-length`.
    /// Unknown for a stream, so hyper writes `transfer-encoding: chunked` and
    /// the first chunk can leave before the last exists.
    fn size_hint(&self) -> SizeHint {
        match &self.0 {
            Out::Once(Some(bytes)) => SizeHint::with_exact(bytes.len() as u64),
            Out::Once(None) => SizeHint::with_exact(0),
            Out::Stream(_) => SizeHint::default(),
        }
    }
}

/// One request, from hyper's types into ours and back.
async fn exchange<F, Fut>(
    req: hyper::Request<Incoming>,
    peer: SocketAddr,
    handle: &F,
    streams: &StreamGate,
) -> hyper::Response<OutBody>
where
    F: Fn(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    let request = match read(req, peer).await {
        Ok(request) => request,
        // A verb the router has no name for cannot be dispatched, and 404
        // would be a claim about the path, which may be perfectly good.
        Err(Rejected::UnknownVerb) => {
            return render(Response::text(501, "Not Implemented"), false, streams)
        }
        // The handler is never called. Handing it the first two megabytes, or
        // an empty body, would let it act on a request nobody sent -- and an
        // empty body is indistinguishable from a legitimate empty POST.
        Err(Rejected::TooLarge) => {
            return render(Response::text(413, "Payload Too Large"), false, streams)
        }
    };

    // HEAD is GET without the body, and a handler should not have to know
    // that. A page answers HEAD correctly by answering GET at all.
    let head_only = request.verb == Verb::Head;
    render(handle(request).await, head_only, streams)
}

/// Ours into hyper's.
///
/// `streams` is where an unfinished body is registered, so that a shutdown
/// can reach it. A body that arrives already registered -- every body that
/// came through the bridge, because
/// [`App::finish`](brocanth::plugin_app::App::finish) registers it -- is left
/// alone rather than counted twice.
fn render(response: Response, head_only: bool, streams: &StreamGate) -> hyper::Response<OutBody> {
    let length = response.body.len();

    let mut out = hyper::Response::builder().status(response.status);
    for (name, value) in &response.headers {
        out = out.header(name, value);
    }

    let body = match (head_only, response.body) {
        // A HEAD still advertises the length the body would have had: one
        // advertising zero is worse than one advertising nothing. A stream
        // has no length to advertise, so it says nothing and the stream is
        // dropped unpolled -- which is what HEAD means.
        (true, _) => {
            if let Some(length) = length {
                out = out.header("content-length", length.to_string());
            }
            OutBody::empty()
        }
        (false, Body::Bytes(bytes)) => OutBody::once(bytes),
        (false, Body::Stream(mut stream)) => {
            if !stream.is_watched() {
                stream.watch(streams);
            }
            OutBody::stream(stream)
        }
    };

    out.body(body).unwrap_or_else(|_| {
        // Only reachable from a header a handler built out of bytes hyper
        // refuses. Losing the headers beats losing the response.
        hyper::Response::builder()
            .status(500)
            .body(OutBody::once(Bytes::from_static(b"Internal Server Error")))
            .expect("a response with no custom headers always builds")
    })
}

/// Why a request never reached the application.
enum Rejected {
    /// A verb the router has no name for.
    UnknownVerb,
    /// A body past [`MAX_BODY`].
    TooLarge,
}

/// hyper's types into ours.
async fn read(req: hyper::Request<Incoming>, peer: SocketAddr) -> Result<Request, Rejected> {
    let verb = Verb::from_http(req.method()).ok_or(Rejected::UnknownVerb)?;

    let (parts, incoming) = req.into_parts();
    let path = parts.uri.path().to_string();
    let query = parts.uri.query().unwrap_or("").to_string();

    let headers = parts
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or("").to_string(),
            )
        })
        .collect();

    // Capped while reading rather than after: the point is not to hold it in
    // the first place.
    let body = http_body_util::Limited::new(incoming, MAX_BODY)
        .collect()
        .await
        .map_err(|_| Rejected::TooLarge)?
        .to_bytes();

    Ok(Request {
        verb,
        path,
        query,
        body,
        peer: Some(peer),
        headers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_response_carries_its_content_type() {
        let text = Response::text(200, "hi");
        assert_eq!(text.status, 200);
        assert_eq!(text.headers[0].1, "text/plain; charset=utf-8");
        assert_eq!(text.body.len(), Some(2));
        assert_eq!(Response::json(201, "{}").headers[0].1, "application/json");
        assert_eq!(
            Response::html(200, "<p>").headers[0].1,
            "text/html; charset=utf-8"
        );
    }

    #[test]
    fn a_replaced_header_is_not_a_second_one() {
        // `Response::html` seeds a content type and a page may answer
        // JSON. Appending left both on the wire, and which one applied was
        // the client's choice.
        let out = Response::html(200, "{}")
            .without_header("Content-Type")
            .with_header("content-type", "application/json");
        let types: Vec<&str> = out
            .headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(types, ["application/json"]);

        // And a header that is meant to repeat still does.
        let out = Response::html(200, "")
            .with_header("set-cookie", "a=1")
            .with_header("set-cookie", "b=2");
        assert_eq!(
            out.headers
                .iter()
                .filter(|(name, _)| name == "set-cookie")
                .count(),
            2
        );
    }

    #[test]
    fn a_header_lookup_ignores_case() {
        let request = Request {
            verb: Verb::Get,
            path: "/".into(),
            query: String::new(),
            body: Bytes::new(),
            peer: None,
            headers: vec![("content-type".into(), "text/plain".into())],
        };
        assert_eq!(request.header("Content-Type"), Some("text/plain"));
        assert_eq!(request.header("CONTENT-TYPE"), Some("text/plain"));
        assert_eq!(request.header("missing"), None);
    }

    #[tokio::test]
    async fn a_head_response_keeps_the_length_and_drops_the_body() {
        let out = render(Response::text(200, "0123456789"), true, &StreamGate::new());
        assert_eq!(
            out.headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok()),
            Some("10"),
            "a HEAD advertising zero is worse than one advertising nothing"
        );
        let body = out.into_body().collect().await.expect("collect").to_bytes();
        assert!(body.is_empty(), "a HEAD sent {} bytes", body.len());
    }

    #[tokio::test]
    async fn a_get_response_keeps_its_body() {
        let out = render(Response::text(200, "0123456789"), false, &StreamGate::new());
        let body = out.into_body().collect().await.expect("collect").to_bytes();
        assert_eq!(&body[..], b"0123456789");
    }

    #[tokio::test]
    async fn port_zero_resolves_to_a_real_port() {
        let server = Server::bind("127.0.0.1:0").await.expect("bind");
        let addr = server.local_addr().expect("local addr");
        assert_ne!(addr.port(), 0, "the bound port should be concrete");
    }

    #[test]
    fn an_aborted_connection_is_transient_and_a_broken_listener_is_not() {
        use std::io::{Error, ErrorKind};
        assert!(transient(&Error::new(ErrorKind::ConnectionAborted, "")));
        assert!(transient(&Error::new(ErrorKind::Interrupted, "")));
        assert!(!transient(&Error::new(ErrorKind::AddrNotAvailable, "")));
    }
}
