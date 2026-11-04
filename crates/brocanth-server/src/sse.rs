//! Server-sent events from async code, with no `poll` written by hand.
//!
//! [`BodyStream`] is a poll function, on purpose: the core has no runtime.
//! A handler that wants to send events as they happen does not want to write
//! one. [`channel`] is the other side of that bargain: a [`Sender`] the
//! handler moves into a task, and an [`EventStream`] it hands to
//! [`Response::events`](crate::Response::events).
//!
//! ```no_run
//! use std::time::Duration;
//! use brocanth_server::sse::{self, Event};
//! use brocanth_server::Response;
//!
//! fn ticks() -> Response {
//!     let (events, stream) = sse::channel(16);
//!     tokio::spawn(async move {
//!         for n in 0.. {
//!             // `Err` means the client went away or the server is stopping.
//!             if events.send(Event::new(n.to_string()).with_event("tick")).await.is_err() {
//!                 return;
//!             }
//!             tokio::time::sleep(Duration::from_secs(1)).await;
//!         }
//!     });
//!     Response::events(stream.heartbeat(Duration::from_secs(15)))
//! }
//! ```
//!
//! # What each side observes
//!
//! * **The client disconnects.** The transport drops the body, which drops
//!   the [`EventStream`]. Every later [`Sender::send`] returns [`Closed`],
//!   and [`Sender::closed`] resolves. A transport learns that a client left
//!   only when it writes, so an idle stream needs a
//!   [`heartbeat`](EventStream::heartbeat) for the disconnect to be noticed
//!   at all.
//! * **The server stops.** The stream gate calls [`BodyStream::finish`]. The
//!   stream stops accepting events, writes the ones already queued, and ends.
//!   The sender sees the same [`Closed`] as for a disconnect, so a producer
//!   has one exit to write, not two.
//! * **The producer is faster than the client.** The channel is bounded.
//!   [`Sender::send`] waits for room, which is back-pressure on the producer;
//!   [`Sender::try_send`] refuses instead, for a producer that must not wait.
//!
//! # The encoding
//!
//! [`Event::encode`] writes the `event:`, `id:` and `data:` lines. A newline
//! in the data starts a new `data:` line, as the protocol requires. A `\r`
//! or `\n` in the event name or the id is dropped, because it would end the
//! field early and let the data write a field of its own.

use std::fmt;
use std::task::{Context, Poll};
use std::time::Duration;

use brocanth::plugin_body::{BodyStream, Chunk};
use tokio::sync::mpsc;
use tokio::time::{Instant, Interval, MissedTickBehavior};

/// One event: an optional name, an optional id, and the data.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    /// The `event:` field. `None` is the default `message` event, which an
    /// `EventSource` reads with `onmessage`.
    pub event: Option<String>,
    /// The `data:` field. Each line becomes its own `data:` line.
    pub data: String,
    /// The `id:` field. A browser sends the last one it saw back as
    /// `Last-Event-ID` when it reconnects.
    pub id: Option<String>,
}

impl Event {
    /// An unnamed event carrying `data`.
    pub fn new(data: impl Into<String>) -> Self {
        Self {
            event: None,
            data: data.into(),
            id: None,
        }
    }

    /// Names the event. A browser listens with `addEventListener(name, ..)`.
    #[must_use]
    pub fn with_event(mut self, name: impl Into<String>) -> Self {
        self.event = Some(name.into());
        self
    }

    /// Sets the id the browser echoes back as `Last-Event-ID`.
    #[must_use]
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// The bytes on the wire, ending with the blank line that ends a record.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = String::with_capacity(self.data.len() + 32);
        if let Some(id) = &self.id {
            field(&mut out, "id", id);
        }
        if let Some(event) = &self.event {
            field(&mut out, "event", event);
        }
        // `\r\n`, `\r` and `\n` all end a line in SSE. Each becomes one
        // `data:` line, so the client reads back the text it was sent.
        let data = self.data.replace("\r\n", "\n").replace('\r', "\n");
        for line in data.split('\n') {
            out.push_str("data:");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
        out.into_bytes()
    }
}

/// One single-line field. A line break in it is dropped, not obeyed.
fn field(out: &mut String, name: &str, value: &str) {
    out.push_str(name);
    out.push(':');
    out.extend(value.chars().filter(|c| !matches!(c, '\r' | '\n')));
    out.push('\n');
}

/// A comment line: ignored by every client, and enough to keep a proxy from
/// closing an idle connection.
fn comment_bytes(text: &str) -> Vec<u8> {
    let mut out = String::with_capacity(text.len() + 4);
    for line in text.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        out.push(':');
        if !line.is_empty() {
            out.push(' ');
            out.push_str(line);
        }
        out.push('\n');
    }
    out.push('\n');
    out.into_bytes()
}

/// The stream is gone: the client disconnected, or the server is stopping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the event stream is closed: the client left or the server is stopping")
    }
}

impl std::error::Error for Closed {}

/// Why [`Sender::try_send`] did not queue the event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrySendError {
    /// The queue is full. The client reads slower than the producer writes.
    Full,
    /// The stream is gone. See [`Closed`].
    Closed,
}

impl fmt::Display for TrySendError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => {
                f.write_str("the event stream is full: the client reads slower than events arrive")
            }
            Self::Closed => Closed.fmt(f),
        }
    }
}

impl std::error::Error for TrySendError {}

/// A bounded channel of events: the [`Sender`] for the producer, the
/// [`EventStream`] for the response.
///
/// `capacity` is how many encoded records wait for the client before
/// [`Sender::send`] waits too. Zero is raised to one.
pub fn channel(capacity: usize) -> (Sender, EventStream) {
    let (tx, rx) = mpsc::channel(capacity.max(1));
    (
        Sender { tx },
        EventStream {
            rx,
            heartbeat: None,
            ticker: None,
            finishing: false,
        },
    )
}

/// The producer's half. Cheap to clone: every clone writes to one stream.
#[derive(Clone, Debug)]
pub struct Sender {
    tx: mpsc::Sender<Vec<u8>>,
}

impl Sender {
    /// Queues an event, waiting for room when the client is behind.
    pub async fn send(&self, event: Event) -> Result<(), Closed> {
        self.send_bytes(event.encode()).await
    }

    /// Queues a comment line. Clients ignore it.
    pub async fn comment(&self, text: &str) -> Result<(), Closed> {
        self.send_bytes(comment_bytes(text)).await
    }

    /// Queues bytes that are already one or more complete SSE records.
    ///
    /// For a producer that encodes its own, such as a broker whose frames
    /// are encoded once and shared by every subscriber. Nothing checks them.
    pub async fn send_bytes(&self, bytes: Vec<u8>) -> Result<(), Closed> {
        self.tx.send(bytes).await.map_err(|_| Closed)
    }

    /// Queues an event, or refuses at once when there is no room.
    pub fn try_send(&self, event: Event) -> Result<(), TrySendError> {
        self.tx.try_send(event.encode()).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => TrySendError::Full,
            mpsc::error::TrySendError::Closed(_) => TrySendError::Closed,
        })
    }

    /// Resolves when the stream is gone: the client left, or the server is
    /// stopping. `select!` it against the producer's own wait to stop early.
    pub async fn closed(&self) {
        self.tx.closed().await;
    }

    /// Whether the stream is gone.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// The response's half: a [`BodyStream`] for
/// [`Response::events`](crate::Response::events).
pub struct EventStream {
    rx: mpsc::Receiver<Vec<u8>>,
    heartbeat: Option<Duration>,
    /// Built on the first poll, not in [`EventStream::heartbeat`]: a timer
    /// needs the runtime, and the transport's poll is certain to have one.
    ticker: Option<Interval>,
    finishing: bool,
}

impl EventStream {
    /// Writes a comment line after `every` without an event.
    ///
    /// Two jobs. A proxy that closes idle connections sees traffic. And the
    /// transport learns a client left only when a write fails, so without a
    /// heartbeat a silent stream holds its [`Sender`] open for ever.
    #[must_use]
    pub fn heartbeat(mut self, every: Duration) -> Self {
        self.heartbeat = Some(every);
        self.ticker = None;
        self
    }
}

impl fmt::Debug for EventStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventStream")
            .field("heartbeat", &self.heartbeat)
            .field("finishing", &self.finishing)
            .finish()
    }
}

impl BodyStream for EventStream {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk {
        match self.rx.poll_recv(cx) {
            Poll::Ready(Some(bytes)) => {
                // An event is traffic too, so the next heartbeat is due a
                // full period from now.
                if let Some(ticker) = &mut self.ticker {
                    ticker.reset();
                }
                return Poll::Ready(Some(Ok(bytes)));
            }
            // Every sender is gone, or `finish` closed the channel and the
            // queue is empty. Either way, the body is complete.
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Pending => {}
        }
        if self.finishing {
            return Poll::Pending;
        }
        let Some(every) = self.heartbeat else {
            return Poll::Pending;
        };
        let ticker = self.ticker.get_or_insert_with(|| {
            let mut ticker = tokio::time::interval_at(Instant::now() + every, every);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker
        });
        match ticker.poll_tick(cx) {
            Poll::Ready(_) => Poll::Ready(Some(Ok(b":\n\n".to_vec()))),
            Poll::Pending => Poll::Pending,
        }
    }

    /// The server is stopping: refuse new events, write the queued ones, end.
    fn finish(&mut self) {
        self.finishing = true;
        self.rx.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("utf-8")
    }

    #[test]
    fn an_event_is_its_fields_then_a_blank_line() {
        let event = Event::new("hello").with_event("greeting").with_id("7");
        assert_eq!(text(event.encode()), "id:7\nevent:greeting\ndata:hello\n\n");
        assert_eq!(text(Event::new("").encode()), "data:\n\n");
    }

    #[test]
    fn every_line_of_the_data_is_a_data_line() {
        let event = Event::new("<p>\r\none</p>\rtwo\n");
        assert_eq!(
            text(event.encode()),
            "data:<p>\ndata:one</p>\ndata:two\ndata:\n\n"
        );
    }

    #[test]
    fn a_line_break_cannot_smuggle_a_field_through_the_name_or_the_id() {
        let event = Event::new("x")
            .with_event("a\ndata: forged")
            .with_id("1\r\nevent: forged");
        assert_eq!(
            text(event.encode()),
            "id:1event: forged\nevent:adata: forged\ndata:x\n\n"
        );
    }

    #[test]
    fn a_comment_is_a_colon_line() {
        assert_eq!(text(comment_bytes("ping")), ": ping\n\n");
        assert_eq!(text(comment_bytes("a\nb")), ": a\n: b\n\n");
        assert_eq!(text(comment_bytes("")), ":\n\n");
    }

    #[tokio::test]
    async fn events_come_out_in_order_and_the_stream_ends_with_the_last_sender() {
        let (events, mut stream) = channel(4);
        events.send(Event::new("one")).await.expect("open");
        events.comment("two").await.expect("open");
        drop(events);
        let mut out = Vec::new();
        while let Some(chunk) = std::future::poll_fn(|cx| stream.poll_chunk(cx)).await {
            out.push(text(chunk.expect("no error")));
        }
        assert_eq!(out, ["data:one\n\n", ": two\n\n"]);
    }

    #[tokio::test]
    async fn dropping_the_stream_is_a_disconnect_the_sender_sees() {
        let (events, stream) = channel(4);
        assert!(!events.is_closed());
        drop(stream);
        events.closed().await;
        assert_eq!(events.send(Event::new("late")).await, Err(Closed));
        assert_eq!(
            events.try_send(Event::new("late")),
            Err(TrySendError::Closed)
        );
    }

    #[tokio::test]
    async fn finish_writes_what_is_queued_then_ends_and_refuses_the_rest() {
        let (events, mut stream) = channel(4);
        events.send(Event::new("queued")).await.expect("open");
        stream.finish();
        events.closed().await;
        assert_eq!(events.send(Event::new("late")).await, Err(Closed));

        let first = std::future::poll_fn(|cx| stream.poll_chunk(cx)).await;
        assert_eq!(
            first.map(|c| text(c.expect("ok"))).as_deref(),
            Some("data:queued\n\n")
        );
        assert!(std::future::poll_fn(|cx| stream.poll_chunk(cx))
            .await
            .is_none());
    }

    #[tokio::test]
    async fn a_full_queue_refuses_try_send_and_makes_send_wait() {
        let (events, mut stream) = channel(1);
        events.try_send(Event::new("one")).expect("room");
        assert_eq!(events.try_send(Event::new("two")), Err(TrySendError::Full));

        let waiting = tokio::spawn({
            let events = events.clone();
            async move { events.send(Event::new("two")).await }
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished(), "send waits while the queue is full");

        let _ = std::future::poll_fn(|cx| stream.poll_chunk(cx)).await;
        waiting.await.expect("joined").expect("room after a read");
    }

    #[tokio::test(start_paused = true)]
    async fn an_idle_stream_writes_a_heartbeat() {
        let (_events, stream) = channel(1);
        let mut stream = stream.heartbeat(Duration::from_secs(15));
        let chunk = std::future::poll_fn(|cx| stream.poll_chunk(cx)).await;
        assert_eq!(
            chunk.map(|c| text(c.expect("ok"))).as_deref(),
            Some(":\n\n")
        );
    }
}
