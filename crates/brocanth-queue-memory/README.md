# brocanth-queue-memory

An in-process backend for `brocanth-queue`. It has two jobs. A test needs a
queue with no server. The conformance suite needs a backend to check itself
against, because a suite that has never passed proves nothing. `Memory`
takes its `Capabilities` as a constructor argument, so the same storage
runs once unbounded and once with the limits SQS declares. That is how a
Postgres assumption in a test is found without an SQS account.

Use it in tests, in examples, and as the reference when you write a
backend. Do not use it in production: it keeps everything in one process
and loses everything when the process stops. It implements `Backend`,
`Inspect` (the operator screens), and, as `MemoryOutbox`, `Outbox` and
`OutboxInspect`.

## Getting started

```toml
[dev-dependencies]
brocanth-queue = { git = "https://github.com/DHHwasRight/brocanth.git" }
brocanth-queue-memory = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

Inside this workspace: `brocanth-queue-memory.workspace = true`.

The crate has no features and no `scaffold` module. `brocanth create` does
not install it. Its only dependency is `brocanth-queue`.

## Examples

### A queue for a test

`Memory::new()` declares `Capabilities::unbounded()`. `len`, `is_empty` and
`dead_letter` let a test look inside without a worker.

```rust
use std::sync::Arc;

use brocanth_queue::{Client, DynBackend, Job, JobContext};
use brocanth_queue_memory::Memory;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Ping;

impl Job for Ping {
    const KIND: &'static str = "test.ping";
    type Error = std::io::Error;

    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        Ok(())
    }
}

# #[tokio::main]
# async fn main() {
let store = Arc::new(Memory::new());
let backend: Arc<dyn DynBackend> = store.clone();
let client = Client::new(backend);

client.enqueue(Ping).await.unwrap();
assert_eq!(store.len(), 1);
assert!(store.dead_letter().is_empty());
# }
```

### The same store with a managed queue's limits

`Memory::sqs_shaped()` declares a 256 KiB payload limit, a 15 minute delay
ceiling, a 5 minute dedup window and a claim batch of 10, and enforces
them. An enqueue that SQS would refuse fails here too, at the call site.
`Memory::with_capabilities` takes any other set.

```rust
# use std::sync::Arc;
# use std::time::Duration;
# use brocanth_queue::{Client, EnqueueError, Job, JobContext};
# use brocanth_queue_memory::Memory;
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)]
# struct Ping;
# impl Job for Ping {
#     const KIND: &'static str = "test.ping";
#     type Error = std::io::Error;
#     async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
# }
# #[tokio::main]
# async fn main() {
let client = Client::new(Arc::new(Memory::sqs_shaped()));

let refused = client.enqueue(Ping).after(Duration::from_secs(3600)).await;
assert!(matches!(refused, Err(EnqueueError::DelayTooLong { .. })));

// Inside the ceiling, it is accepted.
client.enqueue(Ping).after(Duration::from_secs(60)).await.unwrap();
# }
```

### An outbox and a relay

`MemoryOutbox` is the outbox store with no database.
`MemoryOutbox::write` stands in for the caller's commit. A `Relay` moves
the rows to the bulk backend. A worker runs one with `Worker::relay`; a
test can call `tick` itself.

```rust
use std::sync::Arc;

use brocanth_queue::{NewJob, Relay, RelayConfig, UniquenessKind};
use brocanth_queue_memory::{Memory, MemoryOutbox};

# #[tokio::main]
# async fn main() {
let outbox = Arc::new(MemoryOutbox::new());
let store = Arc::new(Memory::new());

outbox.write(NewJob {
    kind: "mail.receipt".into(),
    queue: "default".into(),
    payload: b"{}".to_vec(),
    run_at: None,
    max_attempts: 5,
    unique_key: None,
    uniqueness: UniquenessKind::None,
});

let mut relay = Relay::new(outbox.clone(), store.clone(), RelayConfig::default());
let tick = relay.tick().await.unwrap();
assert_eq!(tick.delivered, 1);
assert_eq!(store.len(), 1);
# }
```

## Testing

```sh
cargo test -p brocanth-queue-memory
```

No server and no environment variable. `tests/worker.rs` runs the worker
end to end against this backend: retries, the dead letter, a panic, a
rolling deploy, shutdown, the relay. `tests/inspect.rs` holds the operator
screens to the same cases the Postgres adapter meets. The conformance
suite runs against this backend in `brocanth-queue-conformance`.

**Testing an application.** Build the backend in the test, keep an
`Arc<Memory>` for the assertions, and give the application an
`Arc<dyn DynBackend>` clone. For a route that enqueues, send the request
with `brocanth-test`'s `TestClient`, then check `len()` or the
`Inspect::stats` counts.

## Links

- `brocanth-queue` README: the contract and the two lanes.
- [`../brocanth-queue/docs/DECISIONS.md`](../brocanth-queue/docs/DECISIONS.md),
  Part 3: why the memory backend computes the four screen states from
  different columns than Postgres does, and why both are tested.
