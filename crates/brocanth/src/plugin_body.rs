//! The response body, and what a shutdown does to one that has not finished.
//!
//! PLAN.md §3 says *"buffered render, clean 500 — no half-written
//! responses"*, and that stays the default: [`Body::Buffered`] is what a page
//! produces, and a handler that fails before the last byte still becomes a
//! clean 500 because nothing has been written yet.
//!
//! What this module adds is the second kind. SSE, a Next.js-style streamed
//! shell and a download that must not be held in memory all need a body the
//! handler has **not** finished producing when it returns. So:
//!
//! ```text
//! Body::Buffered(Vec<u8>)   every byte, now. The default, and what a page is.
//! Body::Streaming(Streaming) a promise of bytes, polled by the transport.
//! ```
//!
//! # No runtime, and no `futures`
//!
//! The core has three dependencies and
//! `tests/no_hidden_dependencies.rs` fails the build if a fourth kind
//! arrives, so [`BodyStream`] is a hand-written poll function over
//! `core::task` rather than a `Stream`. `core::future::Future`,
//! `core::task::Poll` and `alloc::boxed::Box` are std; this adds no crate.
//! It is the same move [`BoxFuture`](crate::plugin_routes::BoxFuture) makes
//! for an async handler.
//!
//! The method is `poll_chunk` rather than an `async fn next_chunk`, and the
//! reason is the transport rather than taste: hyper asks a body for one
//! `poll_frame`, so a body that answered with a borrowed future would have to
//! be stored beside the future that borrows it. A poll function has no such
//! shape.
//!
//! # The core never polls a body
//!
//! It has no runtime, so it cannot. The core's job here is the *contract* —
//! what a stream is, and what shutdown means to one — and the transport's job
//! is to poll it. `brocanth-server` is the transport in this workspace.
//!
//! # Shutdown, which is ADR 2 one layer further down
//!
//! A subscriber on an SSE endpoint holds a connection open for hours. Without
//! a way to reach it, `begin_shutdown` closes every gate in the application
//! and the deploy then waits for the longest-lived subscriber.
//!
//! [`StreamGate`] is that way, and it keeps ADR 2's two phases:
//!
//! 1. **[`StreamGate::close`]** is `begin_shutdown`. It marks the gate closed
//!    and wakes every parked stream, so that each open body is polled once
//!    more. That poll calls [`BodyStream::finish`], which is the stream's
//!    chance to send a last frame — `event: close` for SSE — and then end.
//!    Nothing is cut off.
//! 2. **[`StreamGate::drained`]** is `shutdown`. It resolves when the last
//!    open stream has ended. The transport awaits it under whatever budget it
//!    allows, and drops what is left when the budget runs out.
//!
//! [`App::streams`](crate::plugin_app::App::streams) is the application's
//! gate, [`App::shutdown`](crate::plugin_app::App::shutdown) closes it in
//! phase [`Begin`](crate::plugin_shutdown::ShutdownPhase::Begin), and
//! [`ShutdownCx::open_streams`](crate::plugin_shutdown::ShutdownCx::open_streams)
//! is what a plugin reads to see how many are left.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

// ---------------------------------------------------------------------------
// The body
// ---------------------------------------------------------------------------

/// A response body: every byte, or a promise of them.
///
/// `Response` is `http::Response<Body>`, so a handler that has the whole
/// answer writes `Body::from(bytes)` and nothing else changes for it.
pub enum Body {
    /// Every byte, in memory. What a rendered page is, and what a
    /// [`ResponseHook::on_body`](crate::plugin_hooks::ResponseHook::on_body)
    /// may rewrite.
    Buffered(Vec<u8>),
    /// Bytes that have not been produced yet. No hook may rewrite them,
    /// because there is nothing yet to rewrite.
    Streaming(Streaming),
}

impl Body {
    /// A body with no bytes at all.
    pub fn empty() -> Self {
        Self::Buffered(Vec::new())
    }

    /// A body the transport will poll.
    pub fn stream(stream: impl BodyStream) -> Self {
        Self::Streaming(Streaming::new(stream))
    }

    /// Whether this body has to be polled to be read.
    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Streaming(_))
    }

    /// The bytes, when there are bytes. [`None`] for a stream.
    pub fn buffered(&self) -> Option<&[u8]> {
        match self {
            Self::Buffered(bytes) => Some(bytes),
            Self::Streaming(_) => None,
        }
    }

    /// The bytes, to rewrite. [`None`] for a stream.
    pub fn buffered_mut(&mut self) -> Option<&mut Vec<u8>> {
        match self {
            Self::Buffered(bytes) => Some(bytes),
            Self::Streaming(_) => None,
        }
    }

    /// The stream, to wrap. [`None`] for a buffered body.
    pub fn streaming_mut(&mut self) -> Option<&mut Streaming> {
        match self {
            Self::Buffered(_) => None,
            Self::Streaming(stream) => Some(stream),
        }
    }

    /// The bytes, by value. [`None`] for a stream.
    pub fn into_buffered(self) -> Option<Vec<u8>> {
        match self {
            Self::Buffered(bytes) => Some(bytes),
            Self::Streaming(_) => None,
        }
    }

    /// How many bytes this body is, when that is knowable. [`None`] for a
    /// stream, which is exactly why a stream cannot carry a `content-length`.
    pub fn len(&self) -> Option<usize> {
        self.buffered().map(<[u8]>::len)
    }

    /// Whether this body is known to carry no bytes. `false` for a stream,
    /// which may yet carry some.
    pub fn is_empty(&self) -> bool {
        self.len() == Some(0)
    }
}

impl Default for Body {
    fn default() -> Self {
        Self::empty()
    }
}

impl std::fmt::Debug for Body {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Buffered(bytes) => write!(f, "Buffered({} bytes)", bytes.len()),
            Self::Streaming(_) => f.write_str("Streaming(..)"),
        }
    }
}

impl From<Vec<u8>> for Body {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Buffered(bytes)
    }
}

impl From<&[u8]> for Body {
    fn from(bytes: &[u8]) -> Self {
        Self::Buffered(bytes.to_vec())
    }
}

impl<const N: usize> From<&[u8; N]> for Body {
    fn from(bytes: &[u8; N]) -> Self {
        Self::Buffered(bytes.to_vec())
    }
}

impl From<String> for Body {
    fn from(text: String) -> Self {
        Self::Buffered(text.into_bytes())
    }
}

impl From<&str> for Body {
    fn from(text: &str) -> Self {
        Self::Buffered(text.as_bytes().to_vec())
    }
}

impl From<Streaming> for Body {
    fn from(stream: Streaming) -> Self {
        Self::Streaming(stream)
    }
}

// ---------------------------------------------------------------------------
// The stream
// ---------------------------------------------------------------------------

/// Something went wrong halfway through a body.
///
/// There is no clean 500 from here: the status and the headers are already on
/// the wire. The transport's only honest move is to stop writing, and the
/// client sees a truncated response — which is what every HTTP stack does and
/// why PLAN.md §3 buffers by default.
#[derive(Debug, thiserror::Error)]
#[error("the response stream failed: {message}")]
pub struct StreamError {
    message: String,
}

impl StreamError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

/// One chunk of a body that is still being produced.
pub type Chunk = Poll<Option<Result<Vec<u8>, StreamError>>>;

/// A body produced one chunk at a time.
///
/// `Send` because a bridged request crosses a task boundary, `'static`
/// because the transport outlives the handler that built the stream.
///
/// ```
/// use std::task::{Context, Poll};
/// use brocanth::plugin_body::{BodyStream, Chunk};
///
/// /// Counts down, then ends.
/// struct Ticks(u8);
///
/// impl BodyStream for Ticks {
///     fn poll_chunk(&mut self, _cx: &mut Context<'_>) -> Chunk {
///         match self.0 {
///             0 => Poll::Ready(None),
///             n => {
///                 self.0 = n - 1;
///                 Poll::Ready(Some(Ok(format!("tick {n}\n").into_bytes())))
///             }
///         }
///     }
///
///     fn finish(&mut self) {
///         // The server is stopping: end after the chunk that is due.
///         self.0 = self.0.min(1);
///     }
/// }
/// ```
pub trait BodyStream: Send + 'static {
    /// The next chunk, [`None`] for "the body is complete", or
    /// [`Poll::Pending`] with the waker registered.
    ///
    /// An empty chunk is allowed and means nothing; a transport may drop it
    /// rather than write a zero-length frame.
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk;

    /// The application is shutting down: end at the next chunk boundary.
    ///
    /// Called **at most once**, on the first poll after
    /// [`StreamGate::close`] — which is ADR 2's `begin_shutdown`, and which
    /// wakes the body so that there is such a poll. The stream is still
    /// polled afterwards, so this is the place to queue a last frame —
    /// `event: close` for SSE, a final newline for a log tail — and then
    /// return [`Poll::Ready(None)`](Poll::Ready).
    ///
    /// Defaulted to doing nothing, which means "I will end when I end". A
    /// stream that keeps that default holds the drain open until the
    /// transport's budget runs out and it is dropped, which the client sees as
    /// a truncated response. That is the failure this method exists to avoid,
    /// so a long-lived stream should implement it.
    fn finish(&mut self) {}
}

/// A [`BodyStream`] that ends immediately. The placeholder
/// [`Streaming::wrap`] leaves behind while it moves the real one out.
struct Ended;

impl BodyStream for Ended {
    fn poll_chunk(&mut self, _cx: &mut Context<'_>) -> Chunk {
        Poll::Ready(None)
    }
}

/// A boxed [`BodyStream`], and whether a [`StreamGate`] is watching it.
///
/// One pointer and one flag, on purpose. Everything a gate needs -- the
/// ticket, the waker slot, the "told to finish" bit -- lives inside the box,
/// reached only through the vtable, so dropping a [`Body`] is one indirect
/// call rather than an inlined mutex lock. `tests/hooks_cost_nothing.rs`
/// reads the generated code for that: a `Body` whose destructor is large
/// enough to outline stops the empty request pipeline folding to a `ret`,
/// which is a cost paid by every application including the ones that never
/// stream.
pub struct Streaming {
    inner: Box<dyn BodyStream>,
    watched: bool,
}

impl Streaming {
    pub fn new(stream: impl BodyStream) -> Self {
        Self::from_boxed(Box::new(stream))
    }

    pub fn from_boxed(stream: Box<dyn BodyStream>) -> Self {
        Self {
            inner: stream,
            watched: false,
        }
    }

    /// Puts another stream around this one.
    ///
    /// This is the seam a compressing
    /// [`ResponseHook`](crate::plugin_hooks::ResponseHook) uses over a body it
    /// cannot buffer: it gets the stream through
    /// [`BodyShape::Streaming`](crate::plugin_hooks::BodyShape::Streaming) and
    /// replaces it with one that compresses each chunk on the way past. The
    /// alternative -- announcing `content-encoding` and compressing nothing --
    /// is the silent skip this split exists to prevent.
    ///
    /// A wrapper should forward [`BodyStream::finish`] to what it wraps, or
    /// the shutdown signal stops at it.
    pub fn wrap<F>(&mut self, f: F)
    where
        F: FnOnce(Box<dyn BodyStream>) -> Box<dyn BodyStream>,
    {
        let inner = std::mem::replace(&mut self.inner, Box::new(Ended));
        self.inner = f(inner);
    }

    /// Puts this body under `gate`, so a shutdown can reach it.
    ///
    /// Called once, by whatever is about to poll the body:
    /// [`App::finish`](crate::plugin_app::App::finish) does it for every
    /// response the application produces, and a transport does it for a
    /// response that never went through an `App`. It is the **outermost**
    /// wrap, so it goes on after every response hook has had the stream;
    /// calling it twice puts the body under the gate twice, which is what
    /// [`Streaming::is_watched`] exists to let a caller avoid.
    pub fn watch(&mut self, gate: &StreamGate) {
        let ticket = gate.ticket();
        self.wrap(|inner| {
            Box::new(Gated {
                inner,
                ticket: Some(ticket),
                told_to_finish: false,
            })
        });
        self.watched = true;
    }

    /// Whether this body is already under a gate.
    pub fn is_watched(&self) -> bool {
        self.watched
    }

    /// The next chunk.
    pub fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk {
        self.inner.poll_chunk(cx)
    }

    /// Tells the stream to end, outside a gate. What a transport calls when
    /// it is stopping a body it never registered.
    pub fn finish(&mut self) {
        self.inner.finish();
    }
}

/// A stream under a [`StreamGate`]: the registration, and the one check that
/// turns a closed gate into a [`BodyStream::finish`] call.
///
/// Inside the box rather than beside it, so that [`Body`]'s destructor stays
/// one indirect call. See [`Streaming`].
struct Gated {
    inner: Box<dyn BodyStream>,
    ticket: Option<StreamTicket>,
    told_to_finish: bool,
}

impl BodyStream for Gated {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk {
        if !self.told_to_finish && self.ticket.as_ref().is_some_and(StreamTicket::is_closing) {
            self.told_to_finish = true;
            self.inner.finish();
        }

        let polled = self.inner.poll_chunk(cx);
        match &polled {
            // Over, either way. Releasing here rather than on drop means a
            // transport that holds a finished body does not hold the drain.
            Poll::Ready(None) | Poll::Ready(Some(Err(_))) => self.ticket = None,
            Poll::Pending => {
                if let Some(ticket) = &self.ticket {
                    ticket.park(cx.waker());
                }
            }
            Poll::Ready(Some(Ok(_))) => {}
        }
        polled
    }

    fn finish(&mut self) {
        self.told_to_finish = true;
        self.inner.finish();
    }
}

impl std::fmt::Debug for Streaming {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Streaming")
            .field("watched", &self.watched)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

#[derive(Default)]
struct GateInner {
    closing: AtomicBool,
    open: AtomicUsize,
    /// One slot per open stream, holding the waker of a stream that is
    /// parked. `close` wakes all of them, because a stream that is waiting
    /// for an event that will never come has to be told to look at the gate.
    parked: Mutex<Vec<Arc<Mutex<Option<Waker>>>>>,
    /// Wakers of the [`StreamGate::drained`] futures.
    waiting: Mutex<Vec<Waker>>,
}

impl GateInner {
    fn wake_waiting(&self) {
        let wakers = std::mem::take(&mut *self.waiting.lock().expect("stream gate poisoned"));
        for waker in wakers {
            waker.wake();
        }
    }
}

/// Every response body of this application that has not finished, and the one
/// bit that tells them all to.
///
/// Cheap to clone: it is one `Arc`. An application that never streams never
/// hands out a ticket, and the gate is then a counter that stays at zero.
#[derive(Clone, Default)]
pub struct StreamGate(Arc<GateInner>);

impl StreamGate {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many bodies are still being produced.
    pub fn open_streams(&self) -> usize {
        self.0.open.load(Ordering::Acquire)
    }

    /// Whether [`StreamGate::close`] has been called.
    pub fn is_closing(&self) -> bool {
        self.0.closing.load(Ordering::Acquire)
    }

    /// ADR 2 phase `Begin`, for a body: tell every open stream to end.
    ///
    /// Returns at once. Each open body then calls [`BodyStream::finish`] on
    /// its next poll, and a body that was parked is woken so that there *is* a
    /// next poll. Idempotent, because the server stopping and
    /// `App::begin_shutdown` are two routes to the same instruction.
    pub fn close(&self) {
        if self.0.closing.swap(true, Ordering::AcqRel) {
            return;
        }
        let parked = self.0.parked.lock().expect("stream gate poisoned");
        for slot in parked.iter() {
            if let Some(waker) = slot.lock().expect("stream gate poisoned").take() {
                waker.wake();
            }
        }
        drop(parked);
        // A gate with nothing open is drained the moment it closes.
        self.0.wake_waiting();
    }

    /// ADR 2 phase `Drain`, for a body: resolves when the last one has ended.
    ///
    /// An `async fn` and not a blocking wait, because only the transport can
    /// poll a body and the transport has the runtime. The core holds no
    /// future and spawns nothing: this is the same move
    /// [`Pipeline::begin_async`](crate::plugin_hooks::Pipeline::begin_async)
    /// makes.
    ///
    /// It does **not** close the gate. Awaiting it without calling
    /// [`StreamGate::close`] first waits for streams that were never told to
    /// end, which is the deploy that hangs.
    pub fn drained(&self) -> Drained<'_> {
        Drained { gate: self }
    }

    /// Registers one open body. Dropping the ticket ends the registration.
    pub fn ticket(&self) -> StreamTicket {
        let slot = Arc::new(Mutex::new(None));
        self.0.open.fetch_add(1, Ordering::AcqRel);
        self.0
            .parked
            .lock()
            .expect("stream gate poisoned")
            .push(Arc::clone(&slot));
        StreamTicket {
            gate: Arc::clone(&self.0),
            slot,
        }
    }
}

impl std::fmt::Debug for StreamGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamGate")
            .field("open_streams", &self.open_streams())
            .field("closing", &self.is_closing())
            .finish()
    }
}

/// One open body's registration with a [`StreamGate`].
pub struct StreamTicket {
    gate: Arc<GateInner>,
    slot: Arc<Mutex<Option<Waker>>>,
}

impl StreamTicket {
    /// Whether the application has asked this body to end.
    pub fn is_closing(&self) -> bool {
        self.gate.closing.load(Ordering::Acquire)
    }

    /// Leaves the waker where [`StreamGate::close`] can reach it.
    fn park(&self, waker: &Waker) {
        let mut slot = self.slot.lock().expect("stream gate poisoned");
        match slot.as_ref() {
            Some(parked) if parked.will_wake(waker) => {}
            _ => *slot = Some(waker.clone()),
        }
    }
}

impl Drop for StreamTicket {
    fn drop(&mut self) {
        self.gate
            .parked
            .lock()
            .expect("stream gate poisoned")
            .retain(|slot| !Arc::ptr_eq(slot, &self.slot));
        if self.gate.open.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.gate.wake_waiting();
        }
    }
}

impl std::fmt::Debug for StreamTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StreamTicket")
            .field("closing", &self.is_closing())
            .finish()
    }
}

/// The future [`StreamGate::drained`] returns.
pub struct Drained<'a> {
    gate: &'a StreamGate,
}

impl Future for Drained<'_> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let inner = &self.gate.0;
        if inner.open.load(Ordering::Acquire) == 0 {
            return Poll::Ready(());
        }
        {
            let mut waiting = inner.waiting.lock().expect("stream gate poisoned");
            if !waiting.iter().any(|w| w.will_wake(cx.waker())) {
                waiting.push(cx.waker().clone());
            }
        }
        // Re-read after registering: the last ticket may have been dropped
        // between the load above and the lock.
        if inner.open.load(Ordering::Acquire) == 0 {
            return Poll::Ready(());
        }
        Poll::Pending
    }
}

impl std::fmt::Debug for Drained<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Drained").field("gate", self.gate).finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Emits `chunks` strings, then ends. `finish` cuts it short after the
    /// one that is due.
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

        /// Sends the chunk that is due, then ends.
        fn finish(&mut self) {
            self.chunks.truncate(self.next + 1);
        }
    }

    fn drain(body: &mut Streaming) -> Vec<String> {
        let mut cx = Context::from_waker(Waker::noop());
        let mut out = Vec::new();
        loop {
            match body.poll_chunk(&mut cx) {
                Poll::Ready(Some(Ok(chunk))) => out.push(String::from_utf8(chunk).expect("utf-8")),
                Poll::Ready(Some(Err(e))) => panic!("{e}"),
                Poll::Ready(None) => return out,
                Poll::Pending => panic!("this stream never parks"),
            }
        }
    }

    #[test]
    fn a_buffered_body_knows_its_length_and_a_stream_does_not() {
        assert_eq!(Body::from("hello").len(), Some(5));
        assert_eq!(Body::stream(Canned::new(&["a"])).len(), None);
        assert!(Body::empty().is_empty());
        assert!(!Body::stream(Canned::new(&[])).is_empty());
    }

    #[test]
    fn a_stream_runs_to_the_end_when_nothing_closes_the_gate() {
        let gate = StreamGate::new();
        let mut body = Streaming::new(Canned::new(&["one", "two", "three"]));
        body.watch(&gate);
        assert_eq!(gate.open_streams(), 1);
        assert_eq!(drain(&mut body), ["one", "two", "three"]);
        assert_eq!(gate.open_streams(), 0, "the ticket is released at the end");
    }

    #[test]
    fn a_closed_gate_tells_an_open_stream_to_finish() {
        let gate = StreamGate::new();
        let mut body = Streaming::new(Canned::new(&["one", "two", "three"]));
        body.watch(&gate);

        let mut cx = Context::from_waker(Waker::noop());
        assert!(matches!(body.poll_chunk(&mut cx), Poll::Ready(Some(Ok(_)))));

        gate.close();
        assert!(gate.is_closing());

        // One more chunk -- the one that was due -- and then the end.
        assert_eq!(drain(&mut body), ["two"]);
        assert_eq!(gate.open_streams(), 0);
    }

    #[test]
    fn closing_twice_tells_a_stream_once() {
        /// Counts the `finish` calls rather than acting on them.
        struct Counted(Arc<AtomicUsize>);
        impl BodyStream for Counted {
            fn poll_chunk(&mut self, _cx: &mut Context<'_>) -> Chunk {
                Poll::Ready(Some(Ok(b"x".to_vec())))
            }
            fn finish(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let gate = StreamGate::new();
        let mut body = Streaming::new(Counted(Arc::clone(&calls)));
        body.watch(&gate);

        gate.close();
        gate.close();
        let mut cx = Context::from_waker(Waker::noop());
        for _ in 0..3 {
            let _ = body.poll_chunk(&mut cx);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1, "`finish` is called once");
    }

    #[test]
    fn wrapping_a_stream_keeps_the_bytes_and_changes_them() {
        /// Uppercases whatever the inner stream produces.
        struct Shout(Box<dyn BodyStream>);
        impl BodyStream for Shout {
            fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk {
                match self.0.poll_chunk(cx) {
                    Poll::Ready(Some(Ok(chunk))) => {
                        Poll::Ready(Some(Ok(chunk.to_ascii_uppercase())))
                    }
                    other => other,
                }
            }
            fn finish(&mut self) {
                self.0.finish();
            }
        }

        let mut body = Streaming::new(Canned::new(&["one", "two"]));
        body.wrap(|inner| Box::new(Shout(inner)));
        assert_eq!(drain(&mut body), ["ONE", "TWO"]);
    }

    #[test]
    fn a_gate_with_nothing_open_is_drained_at_once() {
        let gate = StreamGate::new();
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(
            std::pin::pin!(gate.drained()).poll(&mut cx),
            Poll::Ready(())
        );
    }

    #[test]
    fn a_gate_is_drained_when_the_last_stream_ends() {
        let gate = StreamGate::new();
        let mut body = Streaming::new(Canned::new(&["one"]));
        body.watch(&gate);

        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(std::pin::pin!(gate.drained()).poll(&mut cx), Poll::Pending);

        let _ = drain(&mut body);
        assert_eq!(
            std::pin::pin!(gate.drained()).poll(&mut cx),
            Poll::Ready(()),
            "the ticket went with the last chunk"
        );
    }

    #[test]
    fn closing_wakes_a_parked_stream() {
        /// Parks for ever, unless told to finish.
        struct Forever {
            finished: bool,
        }
        impl BodyStream for Forever {
            fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk {
                if self.finished {
                    return Poll::Ready(None);
                }
                // A real stream registers a waker it will fire itself; this
                // one relies on the gate, which is the case under test.
                let _ = cx;
                Poll::Pending
            }
            fn finish(&mut self) {
                self.finished = true;
            }
        }

        let flag = Arc::new(Woken(AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&flag));
        let mut cx = Context::from_waker(&waker);

        let gate = StreamGate::new();
        let mut body = Streaming::new(Forever { finished: false });
        body.watch(&gate);
        assert!(body.poll_chunk(&mut cx).is_pending());
        assert!(!flag.0.load(Ordering::SeqCst));

        gate.close();
        assert!(
            flag.0.load(Ordering::SeqCst),
            "a parked stream has to be woken, or nothing ever reads the gate"
        );
        assert!(matches!(body.poll_chunk(&mut cx), Poll::Ready(None)));
        assert_eq!(gate.open_streams(), 0);
    }

    /// A waker that records that it was woken. `std::task::Wake` rather than
    /// a dependency, as the rest of this workspace does it.
    struct Woken(AtomicBool);

    impl std::task::Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.wake_by_ref();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn an_error_releases_the_ticket_too() {
        struct Broken;
        impl BodyStream for Broken {
            fn poll_chunk(&mut self, _cx: &mut Context<'_>) -> Chunk {
                Poll::Ready(Some(Err(StreamError::new("the database went away"))))
            }
        }

        let gate = StreamGate::new();
        let mut body = Streaming::new(Broken);
        body.watch(&gate);

        let mut cx = Context::from_waker(Waker::noop());
        let Poll::Ready(Some(Err(e))) = body.poll_chunk(&mut cx) else {
            panic!("expected the error");
        };
        assert_eq!(e.message(), "the database went away");
        assert_eq!(gate.open_streams(), 0);
    }
}
