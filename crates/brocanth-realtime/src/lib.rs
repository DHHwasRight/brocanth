//! Realtime for brocanth: indexed by topic, addressed by audience, and
//! stopping one layer short of the socket.
//!
//! PLAN.md §10 is the design document for this crate, and its argument is one
//! sentence:
//!
//! > **A predicate you can inspect can be inverted into an audience. A string
//! > you can only execute.**
//!
//! PocketBase broadcasts to *all* connected clients in chunks — there is no
//! topic index — and for each one runs `SELECT (1) FROM t WHERE id = ? AND
//! <rule>`. At a thousand connections with six hundred subscribers, one write
//! is six hundred point queries: 1.2–6 ms on in-process SQLite, and **about
//! 180 ms on Postgres**, which does not work at all. Postgres is brocanth's
//! base, so that number is the whole point.
//!
//! Because a brocanth policy is a *value* — `brocanth_policy::Pred`, an enum
//! with public variants — it can be matched rather than executed.
//! `Pred::audience()` turns "the owner" into `(author_id, 7)`, the broker
//! files the subscriber under that key, and a write becomes a map lookup with
//! **zero queries**.
//!
//! # Fifty words of orientation
//!
//! [`Broker`] holds the index. A transport opens a [`Client`], which owns a
//! bounded [`Mailbox`]. The client adds [`Subscription`]s, each a [`Topic`]
//! and a [`Gate`]. The model layer publishes a `RowChange`; the broker reads
//! the changed row once, looks up, renders one [`Frame`] per audience class,
//! and pushes. Nothing here opens a socket.
//!
//! ```
//! use brocanth_events::RowChange;
//! use brocanth_policy::{Column, Pred};
//! use brocanth_realtime::{Broker, Row, Subscription, Topic};
//!
//! # #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
//! # enum Post { AuthorId }
//! # impl Column for Post {
//! #     const ALL: &'static [Self] = &[Post::AuthorId];
//! #     fn name(&self) -> &'static str { "author_id" }
//! # }
//! let broker = Broker::new();
//! let client = broker.connect().unwrap();
//!
//! // "my own posts" -- one equality, so it inverts.
//! let filed = client
//!     .subscribe(Subscription::gated_by(
//!         Topic::collection("posts").unwrap(),
//!         Pred::eq(Post::AuthorId, 7i64),
//!     ))
//!     .unwrap();
//! assert!(!filed.gating.is_scanned(), "a map lookup, not a scan");
//!
//! // The model layer just wrote the row, so it hands it over: zero queries.
//! let row = Row::new().with("author_id", 7i64);
//! let delivery = broker.publish_with_row(&RowChange::created("posts", 42i64), &row);
//!
//! assert_eq!(delivery.keyed, 1);
//! assert_eq!(delivery.scanned, 0);
//! assert!(client.mailbox().try_recv().is_some());
//! ```
//!
//! # The three corrections §10 names
//!
//! 1. **Index by topic, not by client.** [`Broker`] is a map from collection
//!    to a wildcard index plus a map of row indexes. PocketBase's own source
//!    carries a `@todo` about this.
//! 2. **Bound the per-client queue with an explicit overflow policy.**
//!    [`Mailbox`] is bounded and [`Overflow::DropOldest`] is the default;
//!    [`mailbox`] argues all three choices. A client that stops reading can
//!    never slow the broadcaster, and the test
//!    `a_stalled_subscriber_does_not_stall_the_broadcaster` proves it by
//!    ordering rather than by a clock.
//! 3. **Cap the client count.** [`Config::max_clients`], refusing rather than
//!    evicting at the limit. PocketBase does not cap at all.
//!
//! # The fallback, and why it is a feature
//!
//! Negation, `<>`, orderings and an opaque fragment do not invert, and
//! `brocanth_policy::NotInvertible` says which in a sentence. §10's own answer
//! is that those fall back to per-subscriber evaluation, so that is what
//! [`Gate::Evaluated`] does — but **in memory, against a row read once for the
//! whole publish**, not one query per subscriber. Even the bad path here is
//! better than PocketBase's only path.
//!
//! What matters is that you can tell. Three places say so:
//! [`Client::subscribe`] returns [`Gating::Scanned`] with the reason,
//! [`Broker::audit`] lists every topic on the slow path, and
//! [`Delivery::scanned`] counts it on every publish. A policy that is
//! accidentally opaque is something somebody can notice and fix, which is the
//! whole requirement.
//!
//! # The limit: this crate does not ship the transport
//!
//! **It produces encoded frames and stops.** That used to be a statement
//! about the core, and it is now a statement about this crate's dependency
//! list.
//!
//! The core's half is done. `brocanth::plugin_hooks::Response` is
//! `http::Response<brocanth::plugin_body::Body>`, and a `Body` is either
//! every byte or a `BodyStream` the transport polls. `ResponseHook` is two
//! methods rather than one: `on_headers` runs for every response, a stream
//! included, and `on_body` takes `&mut Vec<u8>` and runs only for a body that
//! is complete. So nothing is silently skipped -- the partial-navigation
//! headers reach a stream unchanged, compression wraps the stream rather than
//! announcing an encoding it will not apply, and `ETag` is unavailable over a
//! stream and says so rather than hashing something wrong. ADR 2's shutdown
//! reaches an open body too: `StreamGate::close` is `begin_shutdown` and
//! calls `BodyStream::finish`, which is where an SSE endpoint sends its
//! `event: close`.
//!
//! What is left is an arrow this crate refuses to draw. **No `brocanth`.**
//! Nothing here touches a request, a response or a route, and a transport is
//! a handler. A handler belongs to whichever crate owns the routes, and
//! putting one here would make every application that merely broadcasts link
//! the router.
//!
//! **What a transport does with this crate**, in about thirty lines none of
//! which are here: implement `brocanth::plugin_body::BodyStream` over a
//! [`Mailbox`], returning `Poll::Ready(Some(Ok(frame.bytes().to_vec())))` for
//! each frame and `Poll::Pending` with the waker [`Mailbox::recv`] already
//! knows how to store; answer the route with
//! `brocanth_server::Response::events(that_stream)`; open the [`Client`] with
//! [`Broker::connect`], write [`Client::id`] in the first frame, and accept a
//! subscription request on a second endpoint, authorising *there*, as
//! PocketBase does, because that is where the subject is known. Implement
//! `BodyStream::finish` by draining the mailbox once and ending, or a deploy
//! waits for the longest-lived subscriber.
//!
//! A websocket is still a second piece of work, and not this one: it needs an
//! HTTP upgrade below `Response` and a frame codec, neither of which is a
//! body.
//!
//! Note also §4: SSE was on the "cut from v1, explicitly" list, and §4 itself
//! records that three cut things shipped anyway *"and the list is wrong
//! rather than the work"*. This is the fourth.
//!
//! # The transport that needs no stream at all, still not built here
//!
//! Long polling, which was the answer while the core buffered and is still a
//! reasonable answer for an application that wants no open connections. A
//! handler calls [`Broker::connect`] once, keeps the [`Client`] in the
//! session, and on each request calls
//! [`Mailbox::recv_timeout`] for a few seconds and then [`Mailbox::drain`],
//! returning every frame it got as **one complete `Vec<u8>`**. The body is
//! finished when the handler returns, so `Response` does not change, every
//! response hook runs exactly as specified, compression and `ETag` still mean
//! what they mean, and no shutdown phase has to learn about open streams.
//!
//! It costs a request per poll interval and a little latency. It is reported
//! rather than built, for the same reason SSE is: this crate's job is the
//! broker, and a polling endpoint is a handler in whichever crate owns the
//! routes.
//!
//! # No runtime
//!
//! Nothing here is async and nothing here depends on a runtime. A mailbox is
//! readable from a thread ([`Mailbox::recv_blocking`]), from any executor
//! ([`Mailbox::recv`], a hand-written future), or not blocked on at all
//! ([`Mailbox::drain`]). `brocanth-jobs` takes tokio because a queue poller is
//! genuinely asynchronous; a broker that never waits for anything does not
//! need one, and taking one anyway would be a dependency that expresses a
//! preference.
//!
//! # Multi-process
//!
//! §10: *"The same trait over `LISTEN/NOTIFY` buys multi-process realtime —
//! which PocketBase cannot have at any price, its broker being in-memory
//! beside an embedded database."* That is the `listen-notify` feature, off by
//! default. See [`notify`].
//!
//! # The payload
//!
//! §10 wants a fragment rather than JSON, and the renderer is `rsx`. This
//! crate does not depend on it; [`FrameEmitter`] is the seam and [`ChangeLine`]
//! is the default. See [`frame`].

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "admin")]
pub mod admin;
pub mod broker;
pub mod frame;
pub mod gate;
pub mod mailbox;
#[cfg(feature = "listen-notify")]
pub mod notify;
pub mod reach;
pub mod row;
#[cfg(feature = "scaffold")]
pub mod scaffold;
pub mod topic;

/// The panel's screen, compiled in by rsx at macro expansion: nothing is
/// read from disk at run time.
///
/// At the crate root and not inside [`admin`], because a template's own
/// `use` line names this path. Private, like the admin's own template
/// module, which is also what keeps `missing_docs` off generated items.
#[cfg(feature = "admin")]
mod admin_templates {
    rsx::load_dir!("src/admin_templates");
}

pub use broker::{
    Broker, BrokerBuilder, Client, ClientId, Config, ConnectError, Delivery, Fallback, Stats,
    SubscribeError, Subscribed, SubscriptionId,
};
pub use frame::{AudienceClass, ChangeLine, Emit, Frame, FrameEmitter};
pub use gate::{Gate, Gating, Subscription};
pub use mailbox::{Event, Mailbox, Overflow, Pushed, Recv};
pub use reach::{Group, GroupIndex, Plan, Reach, UnknownGroups, ValueKey, Verdict};
pub use row::{NoRows, Row, RowReader};
pub use topic::{BadTopic, Topic};
