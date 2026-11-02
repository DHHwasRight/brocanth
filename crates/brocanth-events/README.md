# brocanth-events

The seam between a layer that writes rows and a layer that reacts to them.
`RowChange` names one row in one collection and what happened to it:
created, updated (with the columns written), or deleted. `ChangeSink` is
where a change goes. The model layer publishes, realtime and the cache
subscribe, and neither side depends on the other. The crate has no
dependencies: `std` only, with `serde` as an optional feature.

Use it when you write a subscriber (an audit log, a cache invalidator, a
realtime broker) or a publisher (a store that writes rows). Do not use it
for one column's old and new value: that is `brocanth_model::dirty::Change`.
The extra word in `RowChange` keeps the two names apart. This crate has no
policy, no audience and no topic index. Realtime joins a change with
`brocanth-policy` itself.

## Getting started

```toml
[dependencies]
brocanth-events = { git = "https://github.com/DHHwasRight/brocanth.git" }
# To put a change on a LISTEN/NOTIFY channel with serde:
# brocanth-events = { git = "...", features = ["serde"] }
```

Inside this workspace: `brocanth-events.workspace = true`.

| Feature | Default | What it adds |
|---|---|---|
| `serde` | off | `Serialize` and `Deserialize` on every type. |

The crate has no `scaffold` module, and `brocanth create` does not add it
directly. `brocanth-model`, `brocanth-resource` and `brocanth-realtime`
depend on it, and `brocanth-cache` does with its `events` feature.

## Examples

### Publish a change

A publisher asks `wants` first. Building a change can allocate (the list
of changed columns), and `NoopSink` answers `false`, so an application with
no subscriber pays one virtual call per write.

```rust
use brocanth_events::{ChangeSink, NoopSink, RowChange};

// The model layer holds one sink and never asks what it is.
let sink: Box<dyn ChangeSink> = Box::new(NoopSink);

// Field names from dirty tracking are `&'static str`. They are borrowed.
let changed: Vec<&'static str> = vec!["title", "body"];
let change = RowChange::updated("posts", 42_i64, changed.as_slice());

if sink.wants(&change.collection) {
    sink.publish(&change);
}
assert!(change.changed_field("title"));
assert!(!change.changed_field("author_id"));
```

### Write a sink

`publish` returns nothing and takes `&self`. A full queue or a closed
socket must not fail a write that is already committed, so the overflow
policy is the sink's own decision. A sink that keeps a change calls
`into_owned`.

```rust
use std::sync::Mutex;

use brocanth_events::{ChangeKind, ChangeSink, RowChange, RowId};

#[derive(Default)]
struct AuditLog {
    lines: Mutex<Vec<RowChange<'static>>>,
}

impl ChangeSink for AuditLog {
    fn publish(&self, change: &RowChange<'_>) {
        if let Ok(mut lines) = self.lines.lock() {
            lines.push(change.clone().into_owned());
        }
    }

    fn wants(&self, collection: &str) -> bool {
        collection == "invoices"
    }
}

let log = AuditLog::default();
log.publish(&RowChange::deleted("invoices", "inv-2026-001"));

let lines = log.lines.lock().unwrap();
assert_eq!(lines[0].row, RowId::Text("inv-2026-001".into()));
assert_eq!(lines[0].kind, ChangeKind::Deleted);
```

### Several sinks behind one

`FanOut` holds every installed sink. It wants a collection when any one of
its sinks does, and publishes to each sink in order.

```rust
use std::sync::Arc;

use brocanth_events::{ChangeSink, FanOut, NoopSink, RowChange};

struct Counter(std::sync::atomic::AtomicUsize);

impl ChangeSink for Counter {
    fn publish(&self, _change: &RowChange<'_>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

let counter = Arc::new(Counter(Default::default()));
let sinks = FanOut::new().with(NoopSink).with(counter.clone());
assert_eq!(sinks.len(), 2);

sinks.publish(&RowChange::created("posts", 7_i64));
assert_eq!(counter.0.load(std::sync::atomic::Ordering::SeqCst), 1);
```

## Testing

```sh
cargo test -p brocanth-events
cargo test -p brocanth-events --features serde
```

No server and no environment variable. The crate has no dev-dependencies,
so the `serde` test checks the trait bounds and does not round-trip through
a format.

**Testing an application.** Install a sink like the `AuditLog` above in the
test, run the write (for a route, send the request with `brocanth-test`'s
`TestClient`), then assert on the changes it kept. Install `NoopSink` to
assert that nothing listens.

## Links

- `PLAN.md` §10: the realtime design this seam serves.
- `crates/brocanth-model/src/changes.rs`: the publisher, including
  `Pending`, which holds changes until the transaction commits.
- `crates/brocanth-cache/src/invalidate.rs`: a subscriber that turns a
  change into cache invalidation.
- `docs/modules.md`: where `brocanth-events` sits among the modules.
