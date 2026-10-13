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
//! **The decision: every core request runs on [`tokio::task::spawn_blocking`],
//! never on a Tokio worker.** Three candidates, and what each costs:
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

use std::net::SocketAddr;
use std::sync::Arc;

use brocanth::plugin_app::{App, BuildError};
use brocanth::plugin_hooks::{
    HookFamilies, Request as CoreRequest, RequestState, Response as CoreResponse,
};
use brocanth::plugin_routes::{Handler, RouteCx};
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
// The bridge
// ---------------------------------------------------------------------------

/// An application and its route table, ready to answer requests.
///
/// [`serve`] is the whole of what most applications need. This type is for the
/// one that wants its own [`Server`] -- an ephemeral port in a test, a stopper
/// wired to a signal handler -- and still wants the core behind it.
pub struct Bridge<E, H> {
    app: App<E, H>,
    router: Router<Handler<E>>,
}

impl<E: 'static, H: HookFamilies> Bridge<E, H> {
    /// Pairs an application with the table it serves.
    pub fn new(app: App<E, H>, router: Router<Handler<E>>) -> Self {
        Self { app, router }
    }

    /// The application, for a caller that wants to read it before serving.
    pub fn app(&self) -> &App<E, H> {
        &self.app
    }

    /// One request, on a blocking thread. See the module header for why.
    pub async fn handle(self: &Arc<Self>, request: Request) -> Response {
        let Ok(request) = to_core(request) else {
            // The path and the query do not recompose into a URI. 404 would be
            // a claim about a route table that has not been consulted and
            // cannot be, so this says what is true: the request line is one
            // this server cannot represent.
            return Response::text(400, "Bad Request");
        };

        let bridge = Arc::clone(self);
        match tokio::task::spawn_blocking(move || bridge.blocking(request)).await {
            Ok(response) => response,
            // A handler panicked, or the runtime is shutting down. Inline, the
            // panic would have unwound into hyper's connection task and the
            // client would have got a closed socket with no status.
            Err(_) => Response::text(500, "Internal Server Error"),
        }
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
                    handler(&mut cx)
                }
                // Built here rather than returned early, so that the response
                // hooks and `after_dispatch` see them like any other response.
                Dispatch::MethodNotAllowed { allow, .. } => Ok(method_not_allowed(allow)),
                Dispatch::NotFound => Ok(plain(404, "Not Found")),
            }
        });

        from_core(response)
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
pub async fn serve<E: 'static, H: HookFamilies>(
    addr: &str,
    app: App<E, H>,
    router: Router<Handler<E>>,
) -> Result<(), BridgeError> {
    let server = Server::bind(addr).await?;
    Bridge::new(app, router).serve(server).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

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
