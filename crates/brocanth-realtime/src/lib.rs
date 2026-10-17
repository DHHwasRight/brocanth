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
//! **It produces encoded frames and stops.** The reason is a property of the
//! core, not a shortage of time:
//!
//! > The core's `Response = http::Response<Vec<u8>>` is buffered, and
//! > `ResponseHook::on_response(&mut Response)` is specified to see a complete
//! > body. A streaming body would silently skip every response hook — which is
//! > exactly §8's "compression, partial-navigation headers and ETag" list. So
//! > SSE is not one type change away; it is a change to the hook contract.
//!
//! `brocanth::plugin_hooks` declares `pub type Response =
//! http::Response<Vec<u8>>` and `ResponseHook::on_response(&self, &Request,
//! &mut Response, &mut RequestState)`. A hook holds `&mut Vec<u8>`: it may
//! read the whole body, rewrite it, compress it, hash it into an `ETag`. None
//! of that is meaningful over a body that has not finished, and most of it is
//! not meaningful over a body that never will.
//!
//! **What the core would have to change to close the seam**, exactly:
//!
//! 1. A response body that can be a stream as well as a `Vec<u8>` — an enum,
//!    or a `Body` type the core owns. `Layer<C, O>` is already generic over
//!    the output, so the router does not care; `Response` is the concrete type
//!    that does.
//! 2. `ResponseHook` split in two, because one trait cannot keep both
//!    meanings. A **header** hook that sees the status and headers before the
//!    first byte, and runs for every response including a stream; and a
//!    **body** hook that sees `&mut Vec<u8>` and runs only for a buffered one.
//!    Compression and partial-navigation headers are the first kind; `ETag` is
//!    the second and is simply not available for a stream, which is correct
//!    and should be stated rather than silently skipped.
//! 3. ADR 2's two-phase shutdown extended to open streams: `begin_shutdown`
//!    has to be able to tell a connection to finish, with `shutdown` waiting
//!    for it, or a deploy hangs on the longest-lived subscriber.
//!
//! Until those exist, a transport built on top of this crate would be a plugin
//! that bypasses the hook chain — and §8 is explicit that *"anything a plugin
//! cannot do through [the extension points] is a gap in the core, to be fixed
//! by widening the seam rather than by special-casing the plugin."* So the gap
//! is written down here instead of worked around.
//!
//! Note also §4: SSE is on the "cut from v1, explicitly" list, and §4 itself
//! records that three cut things shipped anyway *"and the list is wrong rather
//! than the work"*. The broker is the half of realtime that pays for itself
//! immediately — it is also what cache invalidation and an audit log want —
//! and it is the half that does not need the core to change. The transport is
//! the half that does.
//!
//! # One transport that needs no core change at all, not built here
//!
//! Long polling. A handler calls [`Broker::connect`] once, keeps the
//! [`Client`] in the session, and on each request calls
//! [`Mailbox::recv_timeout`] for a few seconds and then [`Mailbox::drain`],
//! returning every frame it got as **one complete `Vec<u8>`**. The body is
//! finished when the handler returns, so `Response` does not change, every
//! response hook runs exactly as specified, compression and `ETag` still mean
//! what they mean, and no shutdown phase has to learn about open streams.
//!
//! It costs a request per poll interval and a little latency, and it is a real
//! option for an application that wants realtime before the core grows a
//! streaming body. It is reported rather than built: this crate's job is the
//! broker, and a polling endpoint is a handler in whichever crate owns the
//! routes.
//!
//! **What a transport does with this crate**, when the core is ready: open the
//! stream, [`Broker::connect`], write `Client::id` in the first frame, accept
//! a subscription request on a second endpoint (authorising *there*, as
//! PocketBase does, because that is where the subject is known), then loop on
//! [`Mailbox::recv`] writing [`Frame::bytes`] verbatim. That loop is about
//! thirty lines and none of them are in this crate.
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
