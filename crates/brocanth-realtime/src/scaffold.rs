//! `realtime` -- what `brocanth create` writes when somebody asks for a
//! broker.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: `Broker::builder`, the four fields of
//! [`Config`](crate::Config), the three spellings of
//! [`Overflow`](crate::Overflow), and the fact that a broker is built
//! synchronously and needs no runtime. Those move when this crate moves, and
//! a generator living in another repository would learn about it from a bug
//! report.
//!
//! # The transport is a second module
//!
//! [`Realtime`] writes the index, the mailboxes and the frames. [`Sse`]
//! (`realtime-sse`) writes the transport into the project: a `BodyStream`
//! over one client's mailbox, and the page `GET /realtime/events` that
//! authorises the topics. The transport is generated code and not a type in
//! this crate, because this crate does not depend on `brocanth` and must
//! not: nothing here touches a request, a response or a route. [`Sse`] says
//! why in full.
//!
//! [`WebSocket`] (`realtime-websocket`) writes the other transport: a loop
//! over one socket, and the page `GET /realtime/socket` that checks the
//! Origin and authorises the topics. The upgrade and the codec are in
//! `brocanth-server`; `docs/realtime.md` has the design.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{configurable, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// The broker, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct Realtime;

impl Module for Realtime {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "realtime",
            "one write is a map lookup, not a query per subscriber",
        )
        .category(Category::Enhancer)
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in("behaviour", "A broadcast broker")
        .krate(Dep::brocanth("brocanth-realtime"))
        // Only for the order of the `[realtime]` section and the builder
        // line: the broker reads its limits from the file the config plugin
        // loaded, so it belongs under it rather than above it.
        .after("config")
        .absent(
            "brocanth-realtime",
            "no broker; a write tells nobody, and a client finds out by asking again",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/realtime/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod realtime;");
        configurable(s, project, "realtime", "Realtime", settings());
        // Also what makes the broker a thing the project reads rather than
        // a field nothing touches: a generated application that warns on
        // its first build is a bad first minute.
        s.at(Slot::AppContext).block(
            "    let realtime: &crate::plugins::realtime::Broadcast = cx.extract();\n\
            \x20   let max_clients = realtime.broker.config().max_clients;\n\
            \x20   crate::plugins::say!(info, \"realtime: up to {max_clients} clients\"; max_clients);"
                .to_string(),
        );
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
    }
}

/// Server-sent events: the broker's transport to a browser.
///
/// # Where the transport lives, and why
///
/// In the generated project, as two files the application owns:
/// `src/plugins/realtime_sse/mod.rs` (a `BodyStream` over one client's
/// mailbox) and `src/pages/realtime/events.rs` (the route, and the one
/// function that authorises a topic).
///
/// Not in this crate, because a transport is a response body and this crate
/// does not depend on `brocanth`: nothing in it touches a request, a
/// response or a route, and an application that only broadcasts in process
/// must not link the router. Not in `brocanth-server` either, because that
/// would draw an arrow from the HTTP transport to one capability crate.
/// The project already depends on all three, so the arrow is drawn where it
/// costs nothing. And the authorisation of a topic is the application's
/// decision in any case: which subject, which policy. A crate would have to
/// take it as a callback; a generated file lets the developer write it.
pub struct Sse;

impl Module for Sse {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "realtime-sse",
            "one long response per subscriber, frames written as they are made",
        )
        .category(Category::Routes)
        .asked_in("behaviour", "")
        .part_of_but_off("realtime")
        .because("it writes the frames the broker makes, and the broker is realtime")
        // `RowChange` is what a page publishes, and the transport's own test
        // publishes one.
        .krate(Dep::brocanth("brocanth-events"))
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/realtime_sse/mod.rs", FRAMES_RS.to_string());
        s.at(Slot::PluginsMod).line("pub mod realtime_sse;");
        s.file("src/pages/realtime/events.rs", EVENTS_RS.to_string());
        // The table's own test lists every route.
        s.at(Slot::PagesRoutes).line("/realtime/events");
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// `src/plugins/realtime_sse/mod.rs`: the stream.
const FRAMES_RS: &str = r#"//! `realtime-sse`: the broker's frames, written to a browser as server-sent
//! events.
//!
//! The transport is here, in the application, and not in
//! `brocanth-realtime`, because that crate does not depend on `brocanth`:
//! nothing in it touches a request, a response or a route. A transport is a
//! response body, and a body belongs to the crate that owns the routes, which
//! is this one. `src/pages/realtime/events.rs` is the route.
//!
//! # What the stream writes
//!
//! ```text
//! : connected 7              first, so the headers leave at once
//! event:lagged               first, when a reconnect may have missed frames
//! id:12 event:posts/* data:  each frame the broker addressed to this client
//! event:lagged data:3        the mailbox was full and 3 frames were dropped
//! :                          a heartbeat, after HEARTBEAT with nothing to say
//! event:close                the server is stopping; the browser reconnects
//! ```
//!
//! `lagged` means "what you show may be stale: fetch it again". It is the
//! mailbox's `Event::Lagged`, written for the browser.
//!
//! # Back-pressure
//!
//! The broker never waits for this stream. The mailbox is bounded, and
//! `[realtime] overflow` says what a full one does. `drop-oldest` and
//! `drop-newest` drop frames, and the stream writes `lagged`. `disconnect`
//! closes the mailbox, the stream ends, and the browser reconnects with
//! `Last-Event-ID` and is told `lagged`.
//!
//! # Ending
//!
//! * The browser leaves. The server drops this stream, which drops the
//!   `Client`, which unsubscribes it and closes its mailbox. The server
//!   learns that a socket is gone only when a write fails, so the heartbeat
//!   is also what frees a dead subscriber, within one period.
//! * The server stops. The stream gate calls `finish`. The stream writes the
//!   frames already queued, then `event:close`, and ends, so a deploy does
//!   not wait for a subscriber.

use std::task::{Context, Poll};
use std::time::Duration;

use brocanth::plugin_body::{BodyStream, Chunk};
use brocanth_realtime::{Client, Event};
use tokio::time::{Instant, Interval, MissedTickBehavior};

/// How long a stream stays silent before it writes a heartbeat.
///
/// Shorter than the idle timeout of any proxy in front of this server, and
/// the longest a dead subscriber holds its mailbox.
pub const HEARTBEAT: Duration = Duration::from_secs(15);

/// One subscriber's frames, as a response body.
pub struct Frames {
    /// The broker's handle for this connection. Dropped with the stream, and
    /// that drop is the disconnect.
    client: Client,
    /// Written before the first frame.
    preamble: Option<Vec<u8>>,
    /// What `finish` left to write: the queued frames and `event:close`.
    tail: Option<Vec<u8>>,
    ended: bool,
    /// Built on the first idle poll: a timer needs the runtime, and the
    /// server's poll is certain to have one.
    ticker: Option<Interval>,
}

impl Frames {
    /// A stream over `client`'s mailbox.
    ///
    /// `missed` is true when the browser reconnected and frames were
    /// published since the last one it read. The broker keeps no history, so
    /// the stream cannot replay them; it says `lagged` instead.
    pub fn new(client: Client, missed: bool) -> Self {
        let mut preamble = format!(": connected {}\n\n", client.id()).into_bytes();
        if missed {
            preamble.extend_from_slice(&lagged(None));
        }
        Self {
            client,
            preamble: Some(preamble),
            tail: None,
            ended: false,
            ticker: None,
        }
    }
}

/// `event:lagged`, with how many frames were dropped when that is known.
fn lagged(skipped: Option<u64>) -> Vec<u8> {
    match skipped {
        Some(n) => format!("event:lagged\ndata:{n}\n\n").into_bytes(),
        None => b"event:lagged\ndata:\n\n".to_vec(),
    }
}

/// What a mailbox event looks like on the wire.
fn encoded(event: Event) -> Vec<u8> {
    match event {
        // Encoded once by the broker and shared by every subscriber.
        Event::Frame(frame) => frame.bytes().to_vec(),
        Event::Lagged { skipped } => lagged(Some(skipped)),
    }
}

impl BodyStream for Frames {
    fn poll_chunk(&mut self, cx: &mut Context<'_>) -> Chunk {
        if let Some(preamble) = self.preamble.take() {
            return Poll::Ready(Some(Ok(preamble)));
        }
        if let Some(tail) = self.tail.take() {
            self.ended = true;
            return Poll::Ready(Some(Ok(tail)));
        }
        if self.ended {
            return Poll::Ready(None);
        }
        match self.client.mailbox().poll_recv(cx) {
            Poll::Ready(Some(event)) => {
                // A frame is traffic, so the next heartbeat is a full
                // period away.
                if let Some(ticker) = &mut self.ticker {
                    ticker.reset();
                }
                return Poll::Ready(Some(Ok(encoded(event))));
            }
            // This stream holds the client, so only the overflow policy
            // closes its mailbox: `disconnect` fired. End; the browser comes
            // back with `Last-Event-ID` and is told `lagged`.
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Pending => {}
        }
        let ticker = self.ticker.get_or_insert_with(|| {
            let mut ticker = tokio::time::interval_at(Instant::now() + HEARTBEAT, HEARTBEAT);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ticker
        });
        match ticker.poll_tick(cx) {
            Poll::Ready(_) => Poll::Ready(Some(Ok(b":\n\n".to_vec()))),
            Poll::Pending => Poll::Pending,
        }
    }

    /// The server is stopping: write what is queued, then `event:close`.
    fn finish(&mut self) {
        let mut tail = Vec::new();
        for event in self.client.mailbox().drain() {
            tail.extend(encoded(event));
        }
        tail.extend_from_slice(b"event:close\ndata:\n\n");
        self.tail = Some(tail);
    }
}

#[cfg(test)]
mod tests {
    use brocanth_events::RowChange;
    use brocanth_realtime::{Broker, Gate, Subscription, Topic};

    use super::*;

    async fn next(frames: &mut Frames) -> Option<String> {
        let chunk = std::future::poll_fn(|cx| frames.poll_chunk(cx)).await?;
        Some(String::from_utf8(chunk.expect("no error")).expect("utf-8"))
    }

    fn subscribed(broker: &std::sync::Arc<Broker>) -> Client {
        let client = broker.connect().expect("room");
        client
            .subscribe(Subscription::new(
                Topic::collection("posts").expect("a topic"),
                Gate::everyone(),
            ))
            .expect("subscribed");
        client
    }

    #[tokio::test]
    async fn a_published_change_is_written_as_an_event() {
        let broker = Broker::new();
        let mut frames = Frames::new(subscribed(&broker), false);
        assert_eq!(
            next(&mut frames).await.as_deref(),
            Some(": connected 1\n\n")
        );

        broker.publish_change(&RowChange::created("posts", 42i64));
        let frame = next(&mut frames).await.expect("a frame");
        assert!(frame.starts_with("id:1\nevent:posts/*\n"), "{frame}");
        assert!(frame.contains("data:id=42\n"), "{frame}");
    }

    #[tokio::test]
    async fn a_reconnect_that_missed_frames_is_told_so() {
        let broker = Broker::new();
        let mut frames = Frames::new(subscribed(&broker), true);
        let preamble = next(&mut frames).await.expect("the preamble");
        assert!(preamble.ends_with("event:lagged\ndata:\n\n"), "{preamble}");
    }

    #[tokio::test]
    async fn finish_writes_the_queue_then_close_then_ends() {
        let broker = Broker::new();
        let mut frames = Frames::new(subscribed(&broker), false);
        let _ = next(&mut frames).await;
        broker.publish_change(&RowChange::deleted("posts", 7i64));

        frames.finish();
        let tail = next(&mut frames).await.expect("the tail");
        assert!(tail.contains("data:id=7\n"), "{tail}");
        assert!(tail.ends_with("event:close\ndata:\n\n"), "{tail}");
        assert_eq!(next(&mut frames).await, None);
    }

    #[tokio::test]
    async fn dropping_the_stream_frees_the_subscriber() {
        let broker = Broker::new();
        let frames = Frames::new(subscribed(&broker), false);
        assert_eq!(broker.clients(), 1);
        drop(frames);
        assert_eq!(broker.clients(), 0);
    }
}
"#;

/// `src/pages/realtime/events.rs`: the route, and who may follow what.
const EVENTS_RS: &str = r#"//! `GET /realtime/events?topics=posts/*,posts/42` -- the broker's frames, as
//! server-sent events.
//!
//! ```js
//! const events = new EventSource("/realtime/events?topics=posts/*");
//! events.addEventListener("posts/*", (e) => console.log(e.data));
//! events.addEventListener("lagged", () => location.reload());
//! ```
//!
//! The topics are in the query because an `EventSource` sends no body and
//! no headers of its own; the cookie goes with it, so a session reads as on
//! any other page. Every topic passes `authorise` before it is filed, and
//! one refusal refuses the request: a stream that quietly follows fewer
//! topics than it asked for is a bug nobody sees.
//!
//! The transport is `crate::plugins::realtime_sse`.

use brocanth_realtime::{Gate, Subscription, Topic};

use crate::pages::{Ctx, Error, IntoResponse, Response};
use crate::plugins::realtime::Broadcast;
use crate::plugins::realtime_sse::Frames;

/// Collections any visitor may follow: every row, every change.
///
/// Empty, so nothing is public until somebody says so here.
const PUBLIC: &[&str] = &[];

/// Whether this request may follow `topic`, and gated by what.
///
/// The one place a topic is authorised, and the place to edit. A public
/// collection takes `Gate::everyone()`. Rows that a policy decides take the
/// policy: `from_policy` gates `posts/*` with the list rule and `posts/42`
/// with the view rule, and the broker inverts the rule into an index key, so
/// a write is a map lookup and not a check per subscriber.
///
/// ```ignore
/// let subject = /* who is asking: the session's user id */;
/// return Ok(Subscription::from_policy(topic, &PostPolicy, &subject));
/// ```
fn authorise(_ctx: &Ctx, topic: Topic) -> Result<Subscription, Error> {
    if PUBLIC.contains(&topic.collection_name()) {
        return Ok(Subscription::new(topic, Gate::everyone()));
    }
    Err(Error::new(
        403,
        format!("no rule lets this request follow {topic}; add one to `authorise` in src/pages/realtime/events.rs"),
    ))
}

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let mut subscriptions = Vec::new();
    for topic in topics(&ctx.query)? {
        subscriptions.push(authorise(ctx, topic)?);
    }
    let broker = &ctx
        .provided::<Broadcast, _>()
        .ok_or_else(|| Error::internal("the realtime plugin put no broker on the context"))?
        .broker;

    // A browser that reconnects sends the id of the last frame it read. The
    // broker keeps no history to replay, so the honest answer to "did I miss
    // anything" is yes whenever a frame was published since.
    let missed = ctx
        .header("last-event-id")
        .is_some_and(|seen| seen.trim().parse::<u64>().ok() != Some(broker.last_frame_id()));

    // At the cap the broker refuses rather than evicting somebody. The
    // browser retries on its own; `retry-after` says when.
    let client = broker
        .connect()
        .map_err(|e| Error::new(503, e.to_string()).with_header("retry-after", "5"))?;
    for subscription in subscriptions {
        client
            .subscribe(subscription)
            .map_err(|e| Error::new(400, e.to_string()))?;
    }
    Ok(Response::events(Frames::new(client, missed)))
}

/// Every `topics=` in the query, split at commas and parsed.
fn topics(query: &str) -> Result<Vec<Topic>, Error> {
    let mut out = Vec::new();
    for pair in query.split('&') {
        let Some(("topics", value)) = pair.split_once('=') else {
            continue;
        };
        for text in decoded(value)?.split(',').filter(|t| !t.is_empty()) {
            let topic =
                Topic::parse(text).map_err(|e| Error::new(400, format!("topic {text:?}: {e}")))?;
            out.push(topic);
        }
    }
    if out.is_empty() {
        return Err(Error::new(
            400,
            "name what to follow: ?topics=posts/* for a collection, posts/42 for a row",
        ));
    }
    Ok(out)
}

/// A query value with its `%XX` and `+` undone.
fn decoded(value: &str) -> Result<String, Error> {
    let bad = || {
        Error::new(
            400,
            format!("the query value {value:?} is not percent-encoded UTF-8"),
        )
    };
    let mut bytes = Vec::with_capacity(value.len());
    let mut rest = value.bytes();
    while let Some(byte) = rest.next() {
        bytes.push(match byte {
            b'+' => b' ',
            b'%' => {
                let hex = [rest.next().ok_or_else(bad)?, rest.next().ok_or_else(bad)?];
                let hex = std::str::from_utf8(&hex).map_err(|_| bad())?;
                u8::from_str_radix(hex, 16).map_err(|_| bad())?
            }
            other => other,
        });
    }
    String::from_utf8(bytes).map_err(|_| bad())
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    use super::*;

    async fn status(target: &str) -> u16 {
        let app = crate::app::build().expect("the application builds");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        // As a browser's `EventSource` asks.
        let mut request = Request::new(Verb::Get, path).with_header("accept", "text/event-stream");
        request.query = query.to_string();
        crate::app::handle(&app, request).await.status
    }

    #[test]
    fn topics_are_read_from_the_query_encoded_or_not() {
        let read = topics("topics=posts/*,posts%2F42&x=1&topics=users%2F*").expect("three");
        let names: Vec<String> = read.iter().map(ToString::to_string).collect();
        assert_eq!(names, ["posts/*", "posts/42", "users/*"]);
        assert!(
            topics("").is_err(),
            "no topic is a 400, not a silent stream"
        );
        assert!(topics("topics=posts").is_err(), "a topic has a slash");
    }

    #[tokio::test]
    async fn a_request_without_topics_is_a_bad_request() {
        assert_eq!(status("/realtime/events").await, 400);
    }

    #[tokio::test]
    async fn a_topic_no_rule_allows_is_refused() {
        assert_eq!(status("/realtime/events?topics=posts/*").await, 403);
    }
}
"#;

/// The `AGENTS.md` section: publishing, and reading it in a browser.
const AGENTS: &str = r#"
## Realtime over server-sent events

`GET /realtime/events?topics=posts/*,posts/42` streams the broker's frames
to a browser. `src/plugins/realtime_sse/mod.rs` is the stream;
`src/pages/realtime/events.rs` is the route.

- Authorise every topic in `authorise` in `src/pages/realtime/events.rs`.
  Nothing is public until it is listed in `PUBLIC` or gated there by a
  policy (`Subscription::from_policy`). Never filter frames after delivery.
- Publish from a page after the write, with the row you already hold:

  ```rust
  let realtime = ctx.provided::<Broadcast, _>().expect("a dispatched request");
  let row = brocanth_realtime::Row::new().with("author_id", author);
  realtime.broker.publish_with_row(&brocanth_events::RowChange::created("posts", id), &row);
  ```

- Read it in the browser. The event name is the topic. `lagged` means the
  view may be stale: fetch it again.

  ```js
  const events = new EventSource("/realtime/events?topics=posts/*");
  events.addEventListener("posts/*", (e) => console.log(e.data));
  events.addEventListener("lagged", () => location.reload());
  ```
"#;

/// WebSockets: the broker's frames to a browser, and commands back.
///
/// # Where the transport lives, and why
///
/// Where [`Sse`] puts it, for the same reasons: in the generated project,
/// as `src/plugins/realtime_websocket/mod.rs` (the loop over one socket)
/// and `src/pages/realtime/socket.rs` (the route, the Origin list and the
/// one function that authorises a topic).
///
/// What this module needs that [`Sse`] does not is in `brocanth-server`:
/// the upgrade out of the request/response shape (`upgrade`), and the
/// handshake, the Origin check and the frame codec (`websocket`, a feature
/// this module turns on). `docs/realtime.md` has the design.
pub struct WebSocket;

impl Module for WebSocket {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "realtime-websocket",
            "a duplex socket per subscriber, frames both ways",
        )
        .category(Category::Routes)
        .asked_in("behaviour", "")
        .part_of_but_off("realtime")
        .because("it carries the frames the broker makes, and the broker is realtime")
        // The upgrade is in every server; the handshake and the codec are
        // behind this feature, so a project without a socket links neither.
        .krate(Dep::brocanth("brocanth-server").feature("websocket"))
        // `RowChange` is what a page publishes, and the socket's own test
        // publishes one.
        .krate(Dep::brocanth("brocanth-events"))
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.file(
            "src/plugins/realtime_websocket/mod.rs",
            SOCKET_RS.to_string(),
        );
        s.at(Slot::PluginsMod).line("pub mod realtime_websocket;");
        s.file("src/pages/realtime/socket.rs", SOCKET_PAGE_RS.to_string());
        // The table's own test lists every route.
        s.at(Slot::PagesRoutes).line("/realtime/socket");
        s.at(Slot::AgentsSections).block(SOCKET_AGENTS.to_string());
    }
}

/// `src/plugins/realtime_websocket/mod.rs`: one socket's loop.
///
/// A file of its own and not a string here, so that `tests/websocket.rs`
/// compiles it and runs its tests in this workspace: the loop is checked by
/// `cargo test`, not only by building a generated project.
const SOCKET_RS: &str = include_str!("../templates/realtime_websocket.rs");

/// `src/pages/realtime/socket.rs`: the route, the Origin list, and who may
/// follow what.
///
/// A file of its own, like the loop, so rustfmt can format it. It is not
/// compiled here: it names the generated project's `crate::pages`.
const SOCKET_PAGE_RS: &str = include_str!("../templates/realtime_socket_page.rs");

/// The `AGENTS.md` section: the socket, and reading it in a browser.
const SOCKET_AGENTS: &str = r#"
## Realtime over a WebSocket

`GET /realtime/socket?topics=posts/*` upgrades to a WebSocket that carries
the broker's frames. `src/plugins/realtime_websocket/mod.rs` is the loop;
`src/pages/realtime/socket.rs` is the route.

- Authorise every topic in `authorise` in `src/pages/realtime/socket.rs`.
  It runs for the topics in the URL and for each `subscribe <topic>` the
  browser sends later, so read what it needs (the user id) into `Viewer`
  when the socket opens. Never filter frames after delivery.
- A page on another site is refused by its `Origin`. List a second site of
  your own in `ALLOWED_ORIGINS`; never turn the check off.
- Each message is one record in the server-sent-events format: `id:`,
  `event:` (the topic) and `data:` lines. `lagged` means the view may be
  stale: fetch it again. A close with code 1001 means the server is
  stopping: reconnect with `?last=<id>`.

  ```js
  const socket = new WebSocket(`ws://${location.host}/realtime/socket?topics=posts/*`);
  socket.onmessage = (e) => console.log(e.data);
  socket.onopen = () => socket.send("subscribe posts/42");
  ```
"#;

/// The `[realtime]` section, with the numbers the crate defaults to and the
/// arithmetic that decides whether they are right here.
fn settings() -> Vec<Setting> {
    vec![
        Setting::integer(
            "max_clients",
            1000,
            "How many connections the broker holds.",
        )
        .note(
            "Refused at the limit, not traded for an existing connection: evicting a\n\
             subscriber to admit a new one lets anyone holding a socket knock everybody\n\
             else off. The memory is max_clients * queue * the size of a frame.",
        ),
        Setting::integer(
            "max_subscriptions_per_client",
            64,
            "How many topics one client may hold.",
        )
        .note(
            "A page asking for more than this is almost certainly asking row by row for\n\
             something the collection wildcard covers.",
        ),
        Setting::integer(
            "queue",
            32,
            "How many frames a mailbox holds before the overflow policy fires.",
        ),
        Setting::text(
            "overflow",
            "drop-oldest",
            "What a full mailbox does: drop-oldest, drop-newest or disconnect.",
        )
        .note(
            "The reader is told how many frames it missed either way, so a stale view\n\
             is a thing the client can see.",
        ),
    ]
}

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "realtime");
    format!(
        r##"//! The realtime broker, as this application registers it.
//!
//! A subscription is an index entry, not a query. `brocanth-policy` is a
//! value rather than a string, so "the owner" inverts into `(author_id, 7)`,
//! the broker files the subscriber under that key, and a write becomes a map
//! lookup. PocketBase runs one `SELECT` per subscriber instead, which is
//! about 180 ms on Postgres at six hundred of them.
//!
//! `brocanth-realtime` is in `Cargo.toml` because this file is here, and
//! through no other route. Delete the registration in `src/plugins/mod.rs`
//! and delete this file, and the application broadcasts nothing and links
//! nothing to do it with.
//!
//! # Publishing
//!
//! A handler that just wrote a row has the row, so it hands it over and the
//! broker reads nothing:
//!
//! ```ignore
//! // In a page handler: `Ctx::provided` reads the plugin context.
//! let realtime = ctx.provided::<Broadcast, _>().expect("a dispatched request");
//! let row = Row::new().with("author_id", author);
//! realtime
//!     .broker
//!     .publish_with_row(&RowChange::created("posts", id), &row);
//! ```
//!
//! `src/app.rs` extracts it once at startup and prints the client limit,
//! which is there to be deleted: it exists so a brand-new project reads the
//! value it was given rather than warning that nothing does.
//!
//! # Reading
//!
//! The broker stops one layer short of the socket. Four things read a
//! mailbox:
//!
//! 1. **A browser, over server-sent events.** The `realtime-sse` module
//!    writes `GET /realtime/events?topics=posts/*` and the stream behind it
//!    (`src/plugins/realtime_sse/mod.rs`): one `Client` per connection, a
//!    heartbeat, `lagged` when frames were dropped, and an end on shutdown.
//!    `brocanth add realtime-sse` adds it. `realtime-websocket` is the
//!    duplex form: `GET /realtime/socket`, frames out and `subscribe`
//!    commands in.
//! 2. **In process.** `Broker::connect`, one subscription, and a thread on
//!    `Mailbox::recv_blocking`. This is how a cache invalidator or an audit
//!    log is written, and it is the reason to have a broker before there is a
//!    transport at all.
//! 3. **Long polling.** Keep the `Client` in the session; each request calls
//!    `Mailbox::recv_timeout` for a few seconds, then `Mailbox::drain`, and
//!    returns the frames as one finished body. Every hook runs exactly as
//!    specified. It costs a request per interval and a little latency.
//! 4. **Nothing.** The index costs what it holds, and an application that
//!    publishes without subscribers pays for a map lookup that finds no one.

use std::sync::Arc;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth_realtime::{{Broker, Config, Overflow}};

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// How many connections the broker will hold. At the limit `connect`
    /// refuses rather than evicting, so a new socket cannot displace an
    /// established one.
    pub max_clients: usize,
    /// How many topics one client may hold, so a client cannot spend the
    /// server's memory by asking.
    pub max_subscriptions_per_client: usize,
    /// How many frames a mailbox holds before the overflow policy fires.
    pub queue: usize,
    /// `drop-oldest`, `drop-newest` or `disconnect`. A string here and an
    /// enum below: the config file is text, and the translation is checked
    /// at startup where a typo can still be answered.
    pub overflow: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            max_clients: 1000,
            max_subscriptions_per_client: 64,
            queue: 32,
            overflow: "drop-oldest".to_string(),
        }}
    }}
}}

/// What this plugin puts on the context.
///
/// A wrapper and not `Arc<Broker>` itself, because `ContextItem` is
/// brocanth's trait and `Broker` is `brocanth-realtime`'s type. That crate
/// deliberately does not depend on the core -- nothing in it touches a
/// request, a response or a route -- so the impl that joins the two belongs
/// in the one crate that can see both, which is this one.
pub struct Broadcast {{
    /// The broker. Shared, so a clone is a pointer and every request sees
    /// the same index.
    pub broker: Arc<Broker>,
}}

impl ContextItem for Broadcast {{}}

pub struct Realtime {{
    settings: Settings,
    /// Built at `configure` and not in `new`, because reading the overflow
    /// policy out of a string can fail and `Plugin::new` cannot.
    broker: Option<Arc<Broker>>,
}}

impl Plugin for Realtime {{
    type Config = Settings;
    type Provides = ProvidesContext<Broadcast>;
    const NAME: &'static str = "realtime";

    fn new(config: Settings) -> Self {{
        Self {{
            settings: config,
            broker: None,
        }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        let overflow = match self.settings.overflow.as_str() {{
            "drop-oldest" => Overflow::DropOldest,
            "drop-newest" => Overflow::DropNewest,
            "disconnect" => Overflow::Disconnect,
            other => {{
                return Err(PluginError::new(format!(
                    "[realtime] overflow is {{other:?}}, which is not a policy\n\
                     help: set `overflow` under [realtime] to drop-oldest, drop-newest or disconnect"
                )))
            }}
        }};
        if self.settings.queue == 0 {{
            return Err(PluginError::new(
                "[realtime] queue is 0, and it must hold at least one frame\n\
                 help: set `queue` under [realtime] to 1 or more, such as 32",
            ));
        }}
        if self.settings.max_clients == 0 {{
            return Err(PluginError::new(
                "[realtime] max_clients is 0, so every connection is refused\n\
                 help: set `max_clients` under [realtime] to 1 or more, such as 1000",
            ));
        }}
        // Synchronous, allocating a few maps, needing no runtime. Unlike a
        // pool, there is nothing here to open, so there is no `before_start`.
        self.broker = Some(
            Broker::builder()
                .config(Config {{
                    max_clients: self.settings.max_clients,
                    max_subscriptions_per_client: self.settings.max_subscriptions_per_client,
                    queue: self.settings.queue,
                    overflow,
                }})
                .build(),
        );
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `configure` runs before the server is handed the application, so
        // the broker is here by the time any request builds a context.
        if let Some(broker) = &self.broker {{
            cx.insert(Broadcast {{
                broker: broker.clone(),
            }});
        }}
    }}
}}
"##
    )
}
