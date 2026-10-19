//! The response-hook split, and what a shutdown does to an open body.
//!
//! One sentence of the contract per test:
//!
//! * a header hook runs for every response, a stream included;
//! * a body hook runs only for a body that is complete;
//! * a hook that must act on a stream's bytes wraps the stream, and a hook
//!   that cannot act on them -- `ETag` -- sets nothing rather than something
//!   wrong;
//! * the reverse declared order is the order it always was, across both
//!   methods;
//! * a streaming body is registered with the application's gate, and
//!   `App::shutdown` closes that gate and counts what is left.

use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::{Body, BodyStream, Chunk, StreamGate, Streaming};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, HookRegistrar, HookSource, OnResponse, Request, RequestState, Response,
    ResponseHead, ResponseHook,
};
use brocanth::plugin_order::Order;
use brocanth::plugin_shutdown::{ShutdownCx, ShutdownReason};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Rec(Arc<Mutex<Vec<String>>>);

impl Rec {
    fn push(&self, line: impl Into<String>) {
        self.0.lock().expect("not poisoned").push(line.into());
    }
    fn take(&self) -> Vec<String> {
        self.0.lock().expect("not poisoned").clone()
    }
}

/// Emits its chunks and ends.
struct Canned {
    chunks: Vec<&'static str>,
    next: usize,
}

impl Canned {
    fn new(chunks: &[&'static str]) -> Self {
        Self {
            chunks: chunks.to_vec(),
            next: 0,
        }
    }
}

impl BodyStream for Canned {
    fn poll_chunk(&mut self, _cx: &mut Context<'_>) -> Chunk {
        match self.chunks.get(self.next) {
            Some(chunk) => {
                self.next += 1;
                Poll::Ready(Some(Ok(chunk.as_bytes().to_vec())))
            }
            None => Poll::Ready(None),
        }
    }
}

/// Reads a whole body. Only a test with no transport does this; the point of
/// the change is that the core never has to.
fn drain(body: &mut Body) -> String {
    match body {
        Body::Buffered(bytes) => String::from_utf8(bytes.clone()).expect("utf-8"),
        Body::Streaming(stream) => {
            let mut cx = Context::from_waker(Waker::noop());
            let mut out = Vec::new();
            loop {
                match stream.poll_chunk(&mut cx) {
                    Poll::Ready(Some(Ok(chunk))) => out.extend_from_slice(&chunk),
                    Poll::Ready(Some(Err(e))) => panic!("{e}"),
                    Poll::Ready(None) => return String::from_utf8(out).expect("utf-8"),
                    Poll::Pending => panic!("this stream never parks"),
                }
            }
        }
    }
}

fn streaming(chunks: &[&'static str]) -> Response {
    Response::new(Body::stream(Canned::new(chunks)))
}

fn buffered(body: &str) -> Response {
    Response::new(Body::from(body))
}

// ---------------------------------------------------------------------------
// A plugin whose hook records which half it was asked for
// ---------------------------------------------------------------------------

struct Watcher {
    rec: Rec,
    tag: &'static str,
}

impl Plugin for Watcher {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "watcher";
    const INSTALLS_HOOKS: bool = true;

    fn new(_: ()) -> Self {
        Self {
            rec: Rec::default(),
            tag: "w",
        }
    }
}

impl HookSource for Watcher {
    type Families = OnResponse;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.response(
            "watch",
            Order::new(),
            WatchHook {
                rec: self.rec.clone(),
                tag: self.tag,
            },
        );
        Ok(())
    }
}

struct WatchHook {
    rec: Rec,
    tag: &'static str,
}

impl ResponseHook for WatchHook {
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        self.rec.push(format!(
            "{}:headers({})",
            self.tag,
            match body {
                BodyShape::Buffered { len } => format!("buffered {len}"),
                BodyShape::Streaming(_) => "streaming".to_string(),
            }
        ));
        // A partial-navigation header: pure head, so a stream gets it
        // unchanged. This is the half of the old contract that was never in
        // danger, and the test says so.
        head.headers
            .insert("dp-partial", http::HeaderValue::from_static("body"));
        Ok(())
    }

    fn on_body(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        body: &mut Vec<u8>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        self.rec.push(format!("{}:body({})", self.tag, body.len()));
        body.extend_from_slice(self.tag.as_bytes());
        Ok(())
    }
}

/// One `Watcher`, and the log it writes.
fn with_watcher(response: Response) -> (Response, Vec<String>) {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking(Watcher {
            rec: rec.clone(),
            tag: "w",
        })
        .build()
        .expect("the application builds");
    let mut request = Request::new(Vec::new());
    let mut state = RequestState::new();
    let out = app.finish(&mut request, &mut state, Ok(response));
    (out, rec.take())
}

// ---------------------------------------------------------------------------
// The split
// ---------------------------------------------------------------------------

#[test]
fn a_buffered_body_gets_both_halves_in_order() {
    let (response, log) = with_watcher(buffered("page"));
    assert_eq!(log, ["w:headers(buffered 4)", "w:body(4)"]);
    assert_eq!(
        response.headers().get("dp-partial").map(|v| v.as_bytes()),
        Some(&b"body"[..])
    );
    assert_eq!(response.body().buffered(), Some(&b"pagew"[..]));
}

#[test]
fn a_body_hook_does_not_run_for_a_stream_and_a_header_hook_does() {
    let (mut response, log) = with_watcher(streaming(&["a", "b"]));
    assert_eq!(
        log,
        ["w:headers(streaming)"],
        "a body hook cannot be handed a body that does not exist yet"
    );
    assert_eq!(
        response.headers().get("dp-partial").map(|v| v.as_bytes()),
        Some(&b"body"[..]),
        "the partial-navigation headers are head-only, so a stream gets them"
    );
    assert_eq!(
        drain(response.body_mut()),
        "ab",
        "and the stream is the one the handler built, untouched"
    );
}

// ---------------------------------------------------------------------------
// ETag: unavailable, and said
// ---------------------------------------------------------------------------

struct Tagger;

impl Plugin for Tagger {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "etag";
    const INSTALLS_HOOKS: bool = true;
    fn new(_: ()) -> Self {
        Self
    }
}

impl HookSource for Tagger {
    type Families = OnResponse;
    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.response("etag", Order::new(), ETag);
        Ok(())
    }
}

struct ETag;

impl ResponseHook for ETag {
    fn on_headers(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        match body {
            // The bytes arrive in `on_body`.
            BodyShape::Buffered { .. } => Ok(()),
            // Unavailable, and said rather than skipped: there is nothing to
            // hash, and a tag over anything else would be one that two
            // different bodies could share.
            BodyShape::Streaming(_) => Ok(()),
        }
    }

    fn on_body(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        body: &mut Vec<u8>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let tag = format!("\"{}\"", body.len());
        head.headers.insert(
            http::header::ETAG,
            http::HeaderValue::from_str(&tag).expect("digits and quotes"),
        );
        Ok(())
    }
}

#[test]
fn an_etag_hook_tags_a_buffered_body_and_says_nothing_about_a_stream() {
    let app = AppBuilder::new()
        .plugin_hooking(Tagger)
        .build()
        .expect("builds");
    let mut request = Request::new(Vec::new());
    let mut state = RequestState::new();

    let complete = app.finish(&mut request, &mut state, Ok(buffered("12345")));
    assert_eq!(
        complete.headers().get(http::header::ETAG).unwrap(),
        "\"5\"",
        "a complete body can be hashed"
    );

    let streamed = app.finish(&mut request, &mut state, Ok(streaming(&["12345"])));
    assert!(
        streamed.headers().get(http::header::ETAG).is_none(),
        "a stream has no ETag, and a wrong one is worse than none"
    );
}

// ---------------------------------------------------------------------------
// Compression: the hook that *can* do something about a stream
// ---------------------------------------------------------------------------

struct Shouter;

impl Plugin for Shouter {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "shout";
    const INSTALLS_HOOKS: bool = true;
    fn new(_: ()) -> Self {
        Self
    }
}

impl HookSource for Shouter {
    type Families = OnResponse;
    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.response("shout", Order::new(), Shout);
        Ok(())
    }
}

/// Stands in for compression: it sets a header *and* changes the bytes, which
/// is the combination the old single method could not keep for a stream.
struct Shout;

struct Upper(Box<dyn BodyStream>);

impl BodyStream for Upper {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk {
        match self.0.poll_chunk(cx) {
            Poll::Ready(Some(Ok(chunk))) => Poll::Ready(Some(Ok(chunk.to_ascii_uppercase()))),
            other => other,
        }
    }
    fn finish(&mut self) {
        self.0.finish();
    }
}

impl ResponseHook for Shout {
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        head.headers
            .insert("content-encoding", http::HeaderValue::from_static("shout"));
        if let BodyShape::Streaming(stream) = body {
            // Announcing an encoding and applying nothing is the silent skip
            // this seam exists to prevent.
            stream.wrap(|inner| Box::new(Upper(inner)));
        }
        Ok(())
    }

    fn on_body(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        body: &mut Vec<u8>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        body.make_ascii_uppercase();
        Ok(())
    }
}

#[test]
fn a_header_hook_can_wrap_a_stream_instead_of_skipping_it() {
    let app = AppBuilder::new()
        .plugin_hooking(Shouter)
        .build()
        .expect("builds");
    let mut request = Request::new(Vec::new());
    let mut state = RequestState::new();

    let mut complete = app.finish(&mut request, &mut state, Ok(buffered("quiet")));
    assert_eq!(drain(complete.body_mut()), "QUIET");

    let mut streamed = app.finish(&mut request, &mut state, Ok(streaming(&["qu", "iet"])));
    assert_eq!(
        streamed.headers().get("content-encoding").unwrap(),
        "shout",
        "the header is announced"
    );
    assert_eq!(
        drain(streamed.body_mut()),
        "QUIET",
        "and applied, chunk by chunk, to a body nobody buffered"
    );
}

// ---------------------------------------------------------------------------
// Order
// ---------------------------------------------------------------------------

#[test]
fn both_halves_keep_the_reverse_declared_order() {
    let rec = Rec::default();
    let app = AppBuilder::new()
        .plugin_hooking_named(
            "inner",
            Watcher {
                rec: rec.clone(),
                tag: "inner",
            },
        )
        .plugin_hooking_as(
            "outer",
            Watcher {
                rec: rec.clone(),
                tag: "outer",
            },
        )
        .build()
        .expect("the application builds");

    let mut request = Request::new(Vec::new());
    let mut state = RequestState::new();
    let response = app.finish(&mut request, &mut state, Ok(buffered("x")));

    assert_eq!(
        rec.take(),
        [
            "outer:headers(buffered 1)",
            "outer:body(1)",
            "inner:headers(buffered 6)",
            "inner:body(6)",
        ],
        "reverse declared order, and one hook's two halves together"
    );
    assert_eq!(response.body().buffered(), Some(&b"xouterinner"[..]));
}

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------

#[test]
fn a_streaming_response_is_registered_with_the_application_gate() {
    let app = AppBuilder::new().build().expect("builds");
    let mut request = Request::new(Vec::new());
    let mut state = RequestState::new();

    assert_eq!(app.streams().open_streams(), 0);

    let mut response = app.finish(&mut request, &mut state, Ok(streaming(&["a"])));
    assert_eq!(
        app.streams().open_streams(),
        1,
        "the application knows about a body it has not finished"
    );

    let _ = drain(response.body_mut());
    assert_eq!(
        app.streams().open_streams(),
        0,
        "and stops knowing when it ends"
    );
}

#[test]
fn a_buffered_response_never_reaches_the_gate() {
    let app = AppBuilder::new().build().expect("builds");
    let mut request = Request::new(Vec::new());
    let mut state = RequestState::new();
    let _ = app.finish(&mut request, &mut state, Ok(buffered("done")));
    assert_eq!(app.streams().open_streams(), 0);
    assert!(!app.streams().is_closing());
}

/// Records what the gate looked like when this plugin was told to stop.
struct Nosy {
    seen: Arc<Mutex<Vec<(String, bool, usize)>>>,
}

impl Nosy {
    fn note(&self, cx: &ShutdownCx<'_>) {
        self.seen.lock().expect("not poisoned").push((
            cx.phase().to_string(),
            cx.streams().is_closing(),
            cx.open_streams(),
        ));
    }
}

impl Plugin for Nosy {
    type Config = ();
    type Provides = NoContext;
    const NAME: &'static str = "nosy";

    fn new(_: ()) -> Self {
        Self {
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn begin_shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        self.note(sd);
        Ok(())
    }

    fn shutdown(&mut self, sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        self.note(sd);
        Ok(())
    }
}

#[test]
fn shutdown_closes_the_gate_before_any_plugin_is_asked() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut app = AppBuilder::new()
        .plugin(Nosy {
            seen: Arc::clone(&seen),
        })
        .build()
        .expect("builds");

    let mut request = Request::new(Vec::new());
    let mut state = RequestState::new();
    let mut response = app.finish(&mut request, &mut state, Ok(streaming(&["a", "b"])));

    let report = app.shutdown(ShutdownReason::Requested, None);

    assert_eq!(
        *seen.lock().expect("not poisoned"),
        [
            ("begin_shutdown".to_string(), true, 1),
            ("shutdown".to_string(), true, 1),
        ],
        "a plugin is told to stop with the gate already closed, so it can see \
         that a stream reading from it has been told to end"
    );
    assert_eq!(
        report.streams_left(),
        1,
        "nothing polled the body, so nothing drained it"
    );
    assert!(
        !report.is_clean(),
        "a shutdown that cut a response off is not a clean one"
    );
    assert!(report.to_string().contains("still open"), "{report}");

    // The transport is what drains it; here the test stands in for one.
    let _ = drain(response.body_mut());
    assert_eq!(app.streams().open_streams(), 0);
}

#[test]
fn a_gate_closed_by_the_transport_is_the_gate_the_application_closes() {
    let app = AppBuilder::new().build().expect("builds");
    let transport: StreamGate = app.streams().clone();

    let mut request = Request::new(Vec::new());
    let mut state = RequestState::new();
    let mut response = app.finish(&mut request, &mut state, Ok(streaming(&["a"])));

    assert_eq!(transport.open_streams(), 1);
    transport.close();
    assert!(
        app.streams().is_closing(),
        "one gate: the listener stopping and `App::shutdown` are two routes \
         to the same instruction"
    );

    let _ = drain(response.body_mut());
    assert_eq!(transport.open_streams(), 0);
}

#[test]
fn a_body_the_core_never_saw_can_still_be_watched_by_hand() {
    // What a transport does for a response that did not come through the
    // core: `Streaming::watch` is public for exactly that.
    let gate = StreamGate::new();
    let mut stream = Streaming::new(Canned::new(&["a"]));
    assert!(!stream.is_watched());
    stream.watch(&gate);
    assert!(stream.is_watched());
    assert_eq!(gate.open_streams(), 1);
}
