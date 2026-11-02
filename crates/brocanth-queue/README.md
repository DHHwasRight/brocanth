# brocanth-queue

The job contract: two lanes, a lease, a worker, an outbox relay, and a
`Backend` trait. The storage is a choice. An application starts with its
jobs in the database it already has. It moves the high-volume jobs to a
dedicated backend when the write churn says so, and the application code
does not change. That works only if the contract is written against the
weakest backend it intends to support, so this crate is specified against
SQS and not against Postgres.

Use this crate when you write a job that must survive a change of backend,
or when you write a backend. This crate names no driver and stores
nothing: pair it with `brocanth-queue-postgres` (production) or
`brocanth-queue-memory` (tests). Do not use it for the queue that
`brocanth create --with jobs` installs. That queue is `brocanth-jobs`, a
separate implementation with its own `Job` trait. `brocanth-jobs` uses only
this crate's read side (`Inspect`), for its admin panel.

| crate | what it is |
|---|---|
| `brocanth-queue` | the contract: lanes, `Job`, `Backend`, the lease, the worker, the relay, `Inspect` |
| `brocanth-queue-memory` | an in-process backend; the reference the suite is checked against |
| `brocanth-queue-postgres` | the Postgres adapter, its schema, the outbox and the promoter |
| `brocanth-queue-conformance` | the contract as a suite that any backend must pass |

## Two lanes

`Lane::Transactional` commits with the data it refers to. It lives in the
application's own database and never moves. `Backend` does not serve it,
because "join the caller's transaction" has no `dyn` signature that an SQS
adapter can implement. `Lane::Bulk` is fire-and-forget and idempotent by
contract. It is the only lane a `Backend` serves and the only lane that
moves.

There is no third lane for scheduled work. A delay is `.after(..)` on
either lane. A Postgres adapter still keeps delayed rows in their own table
with their own index. That is storage, not API.

```text
client.enqueue(SendWelcome { user }).await?;                       // bulk, now
client.enqueue(Digest).after(Duration::from_secs(3600)).await?;    // bulk, delayed
client.enqueue(ChargeCard { order }).in_tx(&mut tx).await?;        // transactional
client.enqueue(SendReceipt { order }).in_tx(&mut tx).await?;       // bulk, via outbox
```

The lane picks the destination. The call picks the atomicity. They are
different axes, and the fourth line is the proof: a `Bulk` job, enqueued
atomically, through the outbox.

## Getting started

```toml
[dependencies]
brocanth-queue = { git = "https://github.com/DHHwasRight/brocanth.git" }
# and one backend:
brocanth-queue-postgres = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

Inside this workspace: `brocanth-queue.workspace = true`.

The crate has no features and no `scaffold` module. `brocanth create` and
`brocanth add` do not install it. It depends on `serde`, `serde_json`,
`thiserror` and `tokio`, and on nothing else.

## Examples

The examples use `brocanth-queue-memory` as the backend, so they run with
no server.

### Define a job, enqueue it, run a worker

```rust
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use brocanth_queue::{Client, DynBackend, Job, JobContext, Registry, Worker, WorkerConfig};
use brocanth_queue_memory::Memory;
use serde::{Deserialize, Serialize};

static SENT: AtomicU32 = AtomicU32::new(0);

#[derive(Serialize, Deserialize)]
struct SendWelcome {
    user: i64,
}

impl Job for SendWelcome {
    // The name on the wire. It outlives a rename of the Rust type.
    const KIND: &'static str = "mail.welcome";
    type Error = std::io::Error;

    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        SENT.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

# #[tokio::main]
# async fn main() {
let store = Arc::new(Memory::new());
let backend: Arc<dyn DynBackend> = store.clone();

let client = Client::new(backend.clone());
client.enqueue(SendWelcome { user: 7 }).await.unwrap();

let mut registry = Registry::new();
registry.register::<SendWelcome>();
let config = WorkerConfig {
    poll_idle: Duration::from_millis(10),
    ..WorkerConfig::default()
};
let worker = Worker::new(backend, Arc::new(registry), config).unwrap();

let (stop, stopped) = tokio::sync::watch::channel(false);
let running = tokio::spawn(worker.run(stopped));
# let start = std::time::Instant::now();
while !store.is_empty() {
#   assert!(start.elapsed() < Duration::from_secs(5), "the job did not run");
    tokio::time::sleep(Duration::from_millis(10)).await;
}
stop.send(true).unwrap();
running.await.unwrap();
assert_eq!(SENT.load(Ordering::SeqCst), 1);
# }
```

### A transactional job needs a transaction

A `Transactional` job exists to commit with its data. Outside a
transaction there is nothing to commit with, so the enqueue fails at the
call site. The atomic path is `.in_tx(&mut tx)`, where `tx` implements
`TxEnqueue`. `brocanth_queue_postgres::Tx` is one.

```rust
# use std::sync::Arc;
# use brocanth_queue::{Client, EnqueueError, Job, JobContext, Lane};
# use brocanth_queue_memory::Memory;
# use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
struct ChargeCard {
    order: i64,
}

impl Job for ChargeCard {
    const KIND: &'static str = "billing.charge";
    const LANE: Lane = Lane::Transactional;
    type Error = std::io::Error;

    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        Ok(())
    }
}

# #[tokio::main]
# async fn main() {
let client = Client::new(Arc::new(Memory::new()));
let refused = client.enqueue(ChargeCard { order: 1 }).await;
assert!(matches!(refused, Err(EnqueueError::NeedsTransaction { .. })));
# }
```

### The boot check refuses a backend that cannot serve a job

A job declares what it needs: `MAX_ATTEMPTS`, `BACKOFF`, `UNIQUENESS`.
`Worker::new` checks every registered job against the backend's
`Capabilities` and refuses to start when one does not fit. The failure
happens at boot, not during the migration to the new backend.

```rust
# use std::sync::Arc;
# use brocanth_queue::{Job, JobContext, Registry, StartError, Uniqueness, Worker, WorkerConfig};
# use brocanth_queue_memory::Memory;
# use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
struct NightlyInvoice;

impl Job for NightlyInvoice {
    const KIND: &'static str = "billing.nightly";
    // Needs an unbounded key store. A managed queue offers a window.
    const UNIQUENESS: Uniqueness = Uniqueness::UntilSuccess;
    type Error = std::io::Error;

    async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        Ok(())
    }
}

let mut registry = Registry::new();
registry.register::<NightlyInvoice>();

// The in-process store, declaring the limits SQS declares.
let sqs_like = Arc::new(Memory::sqs_shaped());
let refused = Worker::new(sqs_like, Arc::new(registry), WorkerConfig::default());
assert!(matches!(refused, Err(StartError::Unmet(_))));
```

### Read the queue for an operator

`Backend` has no method that reads, because SQS cannot list its own
messages. `Inspect` is a separate trait. An adapter implements it only if
it can. An application whose backend cannot serve it has no queue panel,
not a panel that lies.

```rust
# use std::time::{Duration, SystemTime};
use brocanth_queue::{Backend, Inspect, NewJob, Page, UniquenessKind, View};
use brocanth_queue_memory::Memory;

# #[tokio::main]
# async fn main() {
let store = Memory::new();
let job = |run_at| NewJob {
    kind: "report.build".into(),
    queue: "default".into(),
    payload: b"{}".to_vec(),
    run_at,
    max_attempts: 3,
    unique_key: None,
    uniqueness: UniquenessKind::None,
};
let later = SystemTime::now() + Duration::from_secs(600);
store.push(vec![job(None), job(Some(later))]).await.unwrap();

let stats = store.stats().await.unwrap();
assert_eq!((stats.ready, stats.scheduled), (1, 1));

let scheduled = store.list(View::Scheduled, Page::first(50)).await.unwrap();
assert_eq!(scheduled[0].kind, "report.build");
# }
```

## Testing

```sh
cargo test -p brocanth-queue                # unit tests and README doctests
cargo test -p brocanth-queue -p brocanth-queue-memory \
           -p brocanth-queue-postgres -p brocanth-queue-conformance
```

None of these needs a server. The Postgres suite in
`brocanth-queue-postgres` skips each test when no database is set. See
that crate's README for the variables and the Docker command.

To see the conformance report for every backend:

```sh
cargo run -p brocanth-queue-postgres --example report
```

**Testing an application.** Use `brocanth_queue_memory::Memory` as the
backend in a test. It needs no server and it enforces the same contract.
Run the test a second time against `Memory::sqs_shaped()`. A test that
passes on one and fails on the other depends on a Postgres freedom the
contract does not promise. For a route that enqueues, send the request with
`brocanth-test`'s `TestClient` and then read the queue through `Inspect`
(for example `stats().ready`).

## Links

- [`docs/DESIGN.md`](docs/DESIGN.md): what is wrong with the obvious
  design, then the design. It covers why `Scheduled` is not a lane, why
  transactional enqueue is not behind the backend trait, what SQS and
  JetStream cannot do, why dispatch is `dyn`, and why partitioning belongs
  on the archive and not on the queue.
- [`docs/DECISIONS.md`](docs/DECISIONS.md): the four decisions that
  `DESIGN.md` left open, settled with the argument on each side. It records
  the defects the argument found (a sweep that could not run on a busy
  queue, a relay nothing started, archive partitions that stopped two days
  after boot, a claim raced against the stop signal), the read side, the
  outbox dead letter, and what is still open (Part 5).
- `brocanth-queue-conformance`: how a new backend proves it meets this
  contract.
