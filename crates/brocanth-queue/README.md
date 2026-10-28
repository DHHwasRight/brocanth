# brocanth-queue

A job runner whose storage is a choice. An application starts with its jobs
in the database it already has and moves the high-volume ones to a dedicated
backend when the write churn says so, without the application changing.

That only works if the contract is written against the weakest backend it
intends to support, so this one is specified against SQS and not against
Postgres.

**Not yet wired into brocanth.** It is a standalone workspace so the contract
can be settled before it is integrated.

| crate | what it is |
|---|---|
| `brocanth-queue` | the contract: lanes, `Job`, `Backend`, the lease, the worker, the relay |
| `brocanth-queue-memory` | an in-process backend; the reference the suite is checked against |
| `brocanth-queue-postgres` | the Postgres adapter, its schema, the outbox and the promoter |
| `brocanth-queue-conformance` | the contract as a suite any backend can be held to |

## Two lanes

`Transactional` commits with the data it refers to, lives in the
application's own database, and never moves. `Bulk` is fire-and-forget,
idempotent by contract, and is the only lane a backend serves.

There is no third lane for scheduled work: a delay is `.after(..)` on either
lane. A Postgres adapter still keeps delayed rows in their own table with
their own index — that is storage, not API.

```rust
client.enqueue(SendWelcome { user }).await?;                       // bulk, now
client.enqueue(Digest).after(Duration::from_secs(3600)).await?;    // bulk, delayed
client.enqueue(ChargeCard { order }).in_tx(&mut tx).await?;        // transactional
client.enqueue(SendReceipt { order }).in_tx(&mut tx).await?;       // bulk, via outbox
```

The lane picks the destination; the call picks the atomicity. They are
different axes, and the fourth line is the proof.

## Running the tests

```sh
cargo test --workspace            # no server needed
cargo clippy --workspace --all-targets -- -D warnings
```

With a Postgres:

```sh
docker run -d --name bq-pg -e POSTGRES_PASSWORD=pg -e POSTGRES_DB=queue \
    -p 55433:5432 postgres:16-alpine
BROCANTH_QUEUE_TEST_DATABASE_URL="postgres://postgres:pg@127.0.0.1:55433/queue" \
    cargo test -p brocanth-queue-postgres -- --test-threads=1
```

Without the variable each Postgres test prints that it was skipped. In CI,
set `BROCANTH_QUEUE_REQUIRE_POSTGRES=1` as well and a missing URL fails the
run instead of quietly passing.

To see every backend's conformance report:

```sh
cargo run -p brocanth-queue-postgres --example report
```

## Why it is shaped this way

[`DESIGN.md`](DESIGN.md) — what is wrong with the obvious design, then the
design. It covers why `Scheduled` is not a lane, why transactional enqueue is
not behind the backend trait, what SQS and JetStream cannot do, why dispatch
is `dyn`, and why partitioning belongs on the archive and not on the queue.

[`DECISIONS.md`](DECISIONS.md) — the four decisions `DESIGN.md` left open,
settled, with the argument on each side. It also records the four defects
the argument uncovered: a sweep that could not run on a busy queue, a relay
nothing started, archive partitions that stopped two days after boot, and a
claim raced against the stop signal.
