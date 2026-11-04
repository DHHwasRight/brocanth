# brocanth-realtime

A realtime broker indexed by topic and addressed by audience. A brocanth
policy is a value (`brocanth_policy::Pred`), so the broker inverts "the
owner" into the key `(author_id, 7)` and files the subscriber under it. A
write is then a map lookup with zero queries, not one query per subscriber.
The broker holds the index, a bounded mailbox per client, and the encoded
frames. It stops one layer short of the socket: the crate ships no
transport, and the `realtime-sse` module writes one into your application.
Use it to tell other parts of a process about a write (a cache
invalidator, an audit log), to push changes to a browser over server-sent
events, or for long polling. Do not use it if you need a WebSocket: that
transport does not exist yet.

## Transports

| Transport | State | Reason |
| --- | --- | --- |
| In process (`Mailbox::recv_blocking`, `recv`, `drain`) | works | No socket. A thread or any executor reads the mailbox. |
| Long polling (`Mailbox::recv_timeout` then `drain`) | works, you write the handler | The handler returns all frames as one complete body. Every response hook runs as usual. |
| Server-sent events (`realtime-sse`) | works, generated | `brocanth create --with realtime,realtime-sse` writes `GET /realtime/events?topics=posts/*` and a `BodyStream` over the client's mailbox into the project. The code is generated and not in this crate, because this crate does not depend on `brocanth`: a transport is a response body, and a body belongs to the crate that owns the routes. |
| WebSocket (`realtime-websocket`) | **blocked** in `brocanth create` | It needs an HTTP upgrade out of the request/response shape and a frame codec. Neither is a body. That is a second exit from `brocanth-server`, not a new body type. |

The crate docs (`cargo doc -p brocanth-realtime --open`, section "The limit:
this crate does not ship the transport") describe what the generated SSE
transport does: a `BodyStream` over a `Mailbox`, `Response::events`, a
heartbeat, `lagged` for dropped frames, and an end on shutdown.

## Getting started

### In a generated project

```sh
brocanth create my_app --with realtime
brocanth add realtime
```

The module writes `src/plugins/realtime/mod.rs`. The plugin builds the broker at
`configure` and puts a `Broadcast { broker: Arc<Broker> }` on the context.
It declares these settings in the `[realtime]` section (see
`docs/module-flags.md`):

| Key | Default | Flag | Env |
| --- | --- | --- | --- |
| `max_clients` | `1000` | `--realtime.max_clients` | `$MY_APP_REALTIME__MAX_CLIENTS` |
| `max_subscriptions_per_client` | `64` | `--realtime.max_subscriptions_per_client` | `$MY_APP_REALTIME__MAX_SUBSCRIPTIONS_PER_CLIENT` |
| `queue` | `32` | `--realtime.queue` | `$MY_APP_REALTIME__QUEUE` |
| `overflow` | `drop-oldest` | `--realtime.overflow` | `$MY_APP_REALTIME__OVERFLOW` |

`overflow` is `drop-oldest`, `drop-newest` or `disconnect`.
`realtime-sse` adds the browser transport:

```sh
brocanth create my_app --with realtime,realtime-sse
```

It writes `src/plugins/realtime_sse/mod.rs` (the stream) and
`src/pages/realtime/events.rs` (the route). List a collection in `PUBLIC`
there, or gate it by a policy in `authorise`, before a browser can follow
it. `realtime-websocket` is on the menu but blocked; the CLI prints the
reason above.

### By hand

```toml
[dependencies]
brocanth-realtime = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

| Feature | Default | Adds |
| --- | --- | --- |
| `listen-notify` | off | Multi-process realtime over Postgres `LISTEN/NOTIFY` (`notify` module). Adds `serde` and `serde_json`. |
| `admin` | off | `admin::RealtimePanel`, a read-only tab for `brocanth-admin`. Adds the admin and `rsx`, still no `brocanth`. |
| `scaffold` | off | The generator module. Only the CLI turns it on. |

No feature adds an async runtime or a SQL driver.

## Examples

### Publish a change everyone sees

```rust
use brocanth_events::RowChange;
use brocanth_realtime::{Event, Gate, Row, Subscription, Topic};

let broker = brocanth_realtime::Broker::new();
let client = broker.connect().unwrap();
client
    .subscribe(Subscription::new(Topic::collection("posts").unwrap(), Gate::everyone()))
    .unwrap();

let delivery = broker.publish_with_row(&RowChange::created("posts", 42i64), &Row::new());
assert_eq!(delivery.broadcast, 1);
assert!(!delivery.rows_read, "the audience is everyone: no row is needed");

// Each frame is an SSE record, ready to write.
match client.mailbox().try_recv() {
    Some(Event::Frame(frame)) => {
        assert!(std::str::from_utf8(frame.bytes()).unwrap().starts_with("id:"));
    }
    other => panic!("expected a frame, got {other:?}"),
}
```

### See which subscriptions are on the slow path

An equality inverts into a key. A negation does not, so the broker checks
it against the row, in memory, once per publish. Three places tell you:
`Subscribed::gating`, `Broker::audit`, and `Delivery::scanned`.

```rust
use brocanth_policy::{Column, Pred};
use brocanth_realtime::{Broker, Subscription, Topic};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Post { Status }
impl Column for Post {
    const ALL: &'static [Self] = &[Post::Status];
    fn name(&self) -> &'static str { "status" }
}

let broker = Broker::new();
let client = broker.connect().unwrap();
let filed = client
    .subscribe(Subscription::gated_by(
        Topic::collection("posts").unwrap(),
        Pred::ne(Post::Status, "draft"),
    ))
    .unwrap();

assert!(filed.gating.is_scanned());
println!("why: {}", filed.gating.reason().unwrap());

let audit = broker.audit();
assert_eq!(audit[0].topic.collection_name(), "posts");
assert_eq!(audit[0].subscribers, 1);
```

### Limits and backpressure

Every limit is explicit. At `max_clients`, `connect` refuses; it never
evicts an established client. A full mailbox applies the `Overflow` policy,
and the reader gets `Event::Lagged` before the next frame, so it knows its
view is stale.

```rust
use brocanth_events::RowChange;
use brocanth_realtime::{Broker, Config, Event, Gate, Overflow, Row, Subscription, Topic};

let broker = Broker::builder()
    .config(Config {
        max_clients: 1,
        queue: 2,
        overflow: Overflow::DropOldest,
        ..Config::default()
    })
    .build();

let client = broker.connect().unwrap();
assert!(broker.connect().is_err(), "the cap refuses, it does not evict");

client
    .subscribe(Subscription::new(Topic::collection("posts").unwrap(), Gate::everyone()))
    .unwrap();
for id in 1..=5i64 {
    broker.publish_with_row(&RowChange::created("posts", id), &Row::new());
}

let events = client.mailbox().drain();
assert!(matches!(events[0], Event::Lagged { skipped: 3 }));
assert_eq!(events.len(), 3, "one gap marker, then the two newest frames");
```

### Long polling, in a handler you own

```rust,no_run
use std::time::Duration;
use brocanth_realtime::{Client, Event};

/// One poll: wait up to 25 seconds for a frame, then take the rest.
fn poll(client: &Client) -> Vec<u8> {
    let mut body = Vec::new();
    let mailbox = client.mailbox();
    let first = mailbox.recv_timeout(Duration::from_secs(25));
    for event in first.into_iter().chain(mailbox.drain()) {
        if let Event::Frame(frame) = event {
            body.extend_from_slice(frame.bytes());
        }
    }
    body
}
```

`no_run`: it waits up to 25 seconds. Keep the `Client` between requests (in
the session); open it with `Broker::connect`.

## Multi-process (`listen-notify`)

Four web processes install two `brocanth_events::ChangeSink`s each: the
local `Broker`, and a `notify::NotifySink` that puts the change on a
Postgres channel. A listener in each process decodes with `notify::decode`
and publishes into its own broker. The application implements
`notify::Notifier` (one method) against the pool it already opened. This
crate opens no connection.

## Testing

```sh
cargo test -p brocanth-realtime
cargo test -p brocanth-realtime --features listen-notify   # multi-process
cargo test -p brocanth-realtime --features admin           # the admin tab
```

No database is needed: the multi-process tests pass the notifications
through memory.

An application tests a subscriber the way the examples above do: connect a
`Client`, publish, and read the mailbox with `try_recv` or `drain`. No
socket and no runtime are needed. To test the admin tab, use
`brocanth-test`'s `Html` reader on the page and assert on what it shows,
not on CSS classes (see `crates/brocanth-test/README.md`).

## Links

- `PLAN.md` §10: the design, and the PocketBase numbers it answers.
- The crate docs: the three corrections, the fallback, the transport limit,
  long polling, and the payload seam (`FrameEmitter`).
- `docs/module-map.md`: where `realtime` and its blocked children sit.
- `docs/module-flags.md`: how the settings become flags, variables and file
  keys.
