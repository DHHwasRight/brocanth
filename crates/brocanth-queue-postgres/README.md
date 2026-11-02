# brocanth-queue-postgres

The Postgres backend for `brocanth-queue`. The claim is `FOR UPDATE SKIP
LOCKED` under a lease. A promoter moves delayed jobs into the queue. An
outbox relay delivers bulk jobs that a transaction wrote. A partitioned
archive keeps seven days of finished jobs. Postgres can do things SQS and
JetStream cannot: hold an unbounded unique key, delete a pending job by id,
count exactly. This adapter exposes those through `Capabilities` and
nowhere else. So an application that depends on one is told at boot that
it cannot move, not during the migration.

Use it as the production backend for `brocanth-queue`, and as the only
store for the `Transactional` lane. Do not use it for the queue that
`brocanth create --with jobs` installs: that is `brocanth-jobs`, which has
its own tables.

## Getting started

```toml
[dependencies]
brocanth-queue = { git = "https://github.com/DHHwasRight/brocanth.git" }
brocanth-queue-postgres = { git = "https://github.com/DHHwasRight/brocanth.git" }
sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "postgres"] }
```

Inside this workspace: `brocanth-queue-postgres.workspace = true`.

The crate has no features and no `scaffold` module. `brocanth create` and
`brocanth add` do not install it. It uses `sqlx` 0.9, the version the
model layer resolves, so an application that runs both links one driver.
Every statement is a runtime `sqlx::query`, never `query!`, so the crate
builds with no database.

The schema is `SCHEMA` (`src/schema.sql`). `migrate` applies it and creates
the archive partitions. It is safe to call from many processes at once.

## Examples

These examples need a server, so they compile but do not run (`no_run`).

### A worker with a relay and wakeups

`PgQueue::bulk` works the `queue_bulk` table. Give it the **control** pool,
not the pool that job bodies use: a claim, a heartbeat and a settle must
never wait behind a job. `Budget` sizes both pools.

```rust,no_run
use std::sync::Arc;

use brocanth_queue::{
    Budget, DynBackend, Registry, Relay, RelayConfig, Worker, WorkerConfig,
};
use brocanth_queue_postgres::{migrate, wakeups, PgOutbox, PgQueue};
use sqlx::postgres::PgPoolOptions;

# async fn demo() -> Result<(), Box<dyn std::error::Error>> {
let config = WorkerConfig::default();
let budget = Budget::for_worker(10, config.concurrency);
budget.check()?;

let control = PgPoolOptions::new()
    .max_connections(budget.control_needed())
    .connect("postgres://localhost/app")
    .await?;
migrate(&control).await?;

let backend: Arc<dyn DynBackend> = Arc::new(PgQueue::bulk(control.clone()));
let registry = Arc::new(Registry::new()); // register your jobs here
let relay = Relay::new(
    Arc::new(PgOutbox::new(control.clone())),
    backend.clone(),
    RelayConfig::default(),
);

let worker = Worker::new(backend, registry, config)?
    .relay(relay)
    .wake_on(wakeups(&control).await?);

let (stop, stopped) = tokio::sync::watch::channel(false);
let running = tokio::spawn(worker.run(stopped));
// ... at shutdown:
stop.send(true)?;
running.await?;
# Ok(())
# }
```

### Enqueue inside the application's transaction

`Tx` borrows the caller's `sqlx` transaction. A `Transactional` job goes to
`queue_tx`. A `Bulk` job goes to `queue_outbox`, and the relay delivers it
after the commit. A rollback takes both with it.

```rust,no_run
# use std::sync::Arc;
# use brocanth_queue::{Client, Job, JobContext, Lane};
# use brocanth_queue_postgres::{PgQueue, Tx};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)] struct ChargeCard { order: i64 }
# impl Job for ChargeCard {
#     const KIND: &'static str = "billing.charge";
#     const LANE: Lane = Lane::Transactional;
#     type Error = std::io::Error;
#     async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
# }
# #[derive(Serialize, Deserialize)] struct SendReceipt { order: i64 }
# impl Job for SendReceipt {
#     const KIND: &'static str = "mail.receipt";
#     type Error = std::io::Error;
#     async fn run(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
# }
# async fn demo(pool: sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
let client = Client::new(Arc::new(PgQueue::bulk(pool.clone())));

let mut tx = pool.begin().await?;
// ... insert the order with `&mut *tx` ...
client
    .enqueue(ChargeCard { order: 1 })
    .in_tx(&mut Tx::new(&mut tx))
    .await?;
client
    .enqueue(SendReceipt { order: 1 })
    .in_tx(&mut Tx::new(&mut tx))
    .await?;
tx.commit().await?; // the order and both jobs commit together
# Ok(())
# }
```

`Tx::new` refuses a bulk enqueue when the outbox holds
`DEFAULT_MAX_PENDING` undelivered rows: an outage then fails the write that
caused it, and does not fill the disk. `Tx::max_pending` changes the limit.
`Tx::unbounded` removes it.

The worker for the transactional lane is the same `Worker` over
`PgQueue::transactional(pool)`.

### Configure the table, the archive and the claim

`PgConfig` holds every knob, each with its reason in its doc comment.
`PgQueue` also implements `Inspect`, so the operator screens read it, and
`has_history` is true while `archive` is on.

```rust,no_run
use std::time::Duration;

use brocanth_queue::Inspect;
use brocanth_queue_postgres::{PgConfig, PgQueue};

# async fn demo(pool: sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
let queue = PgQueue::bulk(pool).with_config(PgConfig {
    archive_retention: Duration::from_secs(14 * 24 * 60 * 60),
    max_claim_batch: 32,
    ..PgConfig::default()
});

let stats = queue.stats().await?;
println!("{} ready, {} dead", stats.ready, stats.dead);
# Ok(())
# }
```

The archive records **settles, not executions**. Under at-least-once
delivery it is a lower bound on what ran, never a count of it.

## Testing

```sh
cargo test -p brocanth-queue-postgres
```

Without a database, each Postgres test prints `skipped:` and returns. To
run them, start a server and set `BROCANTH_QUEUE_TEST_DATABASE_URL`:

```sh
docker run -d --name bq-pg -e POSTGRES_PASSWORD=pg -e POSTGRES_DB=queue \
    -p 55433:5432 postgres:16-alpine
BROCANTH_QUEUE_TEST_DATABASE_URL="postgres://postgres:pg@127.0.0.1:55433/queue" \
    cargo test -p brocanth-queue-postgres
```

| Variable | Effect |
|---|---|
| `BROCANTH_QUEUE_TEST_DATABASE_URL` | The database these tests use. This crate reads only this variable. |
| `BROCANTH_QUEUE_REQUIRE_POSTGRES=1` | A missing URL fails the run. Set it in CI: a suite that quietly does not run is worse than none. |
| `BROCANTH_TEST_DATABASE_URL` | The workspace-wide variable. This crate does **not** read it today; set the queue variable too. |

**Isolation.** These tests do not make a schema per test. They run
`migrate` against the given database and share its `queue_*` tables. A
lock in the test file runs them one at a time, and each test clears or
measures only its own queue names. Give the suite a database of its own,
not one that another suite or an application uses. The conformance suite
names its queues with a prefix unique to the run, so a second run does not
meet the rows of the first.

The suite runs the conformance cases on `queue_bulk` and `queue_tx`, the
outbox suite on `PgOutbox`, and the transaction, relay, sweep, archive and
screen cases. To print the report for every backend (Postgres is included
when the variable is set):

```sh
cargo run -p brocanth-queue-postgres --example report
```

**Testing an application.** Test job logic against
`brocanth-queue-memory`, which needs no server. Keep one test, gated on the
same variables, that runs the real `Tx` path against Postgres, because
only Postgres proves that a rollback takes the job with it.

## Links

- `brocanth-queue` README: the contract and the two lanes.
- [`../brocanth-queue/docs/DESIGN.md`](../brocanth-queue/docs/DESIGN.md):
  the two tables, the promoter, the outbox, and partitioning on the archive.
- [`../brocanth-queue/docs/DECISIONS.md`](../brocanth-queue/docs/DECISIONS.md):
  the archive default (§2.3), `max_claim_batch` (§2.4), relay placement
  (§2.2), the derived screen states (Part 3), and the outbox dead letter
  (Part 4).
- `src/schema.sql`: every storage parameter, with its reason beside it.
