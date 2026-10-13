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
use std::sync::Arc;

use brocanth::routing::Verb;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
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

/// What to send back.
pub struct Response {
    pub status: u16,
    pub body: Bytes,
    headers: Vec<(String, String)>,
}

impl Response {
    /// A response with a body and an explicit content type.
    pub fn new(status: u16, content_type: &str, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            body: body.into(),
            headers: vec![("content-type".to_string(), content_type.to_string())],
        }
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
            tokio::spawn(async move {
                let service = service_fn(move |req: hyper::Request<Incoming>| {
                    let handle = Arc::clone(&handle);
                    async move { Ok::<_, Infallible>(exchange(req, peer, handle.as_ref()).await) }
                });
                let conn = builder.serve_connection_with_upgrades(TokioIo::new(stream), service);
                let _ = watcher.watch(conn).await;
            });
        }

        graceful.shutdown().await;
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

/// One request, from hyper's types into ours and back.
async fn exchange<F, Fut>(
    req: hyper::Request<Incoming>,
    peer: SocketAddr,
    handle: &F,
) -> hyper::Response<Full<Bytes>>
where
    F: Fn(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    let request = match read(req, peer).await {
        Ok(request) => request,
        // A verb the router has no name for cannot be dispatched, and 404
        // would be a claim about the path, which may be perfectly good.
        Err(Rejected::UnknownVerb) => return render(Response::text(501, "Not Implemented"), false),
        // The handler is never called. Handing it the first two megabytes, or
        // an empty body, would let it act on a request nobody sent -- and an
        // empty body is indistinguishable from a legitimate empty POST.
        Err(Rejected::TooLarge) => return render(Response::text(413, "Payload Too Large"), false),
    };

    // HEAD is GET without the body, and a handler should not have to know
    // that. A page answers HEAD correctly by answering GET at all.
    let head_only = request.verb == Verb::Head;
    render(handle(request).await, head_only)
}

/// Ours into hyper's.
fn render(response: Response, head_only: bool) -> hyper::Response<Full<Bytes>> {
    let length = response.body.len();
    let body = if head_only {
        Bytes::new()
    } else {
        response.body
    };

    let mut out = hyper::Response::builder().status(response.status);
    for (name, value) in &response.headers {
        out = out.header(name, value);
    }
    if head_only {
        // Still the length the body would have had: a HEAD advertising zero
        // is worse than one advertising nothing.
        out = out.header("content-length", length.to_string());
    }

    out.body(Full::new(body)).unwrap_or_else(|_| {
        // Only reachable from a header a handler built out of bytes hyper
        // refuses. Losing the headers beats losing the response.
        hyper::Response::builder()
            .status(500)
            .body(Full::new(Bytes::from_static(b"Internal Server Error")))
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
        assert_eq!(Response::json(201, "{}").headers[0].1, "application/json");
        assert_eq!(
            Response::html(200, "<p>").headers[0].1,
            "text/html; charset=utf-8"
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
        let out = render(Response::text(200, "0123456789"), true);
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
        let out = render(Response::text(200, "0123456789"), false);
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
