# A job runner whose storage is a choice

This is the answer to the brief, in the order the brief asked for it: first
where the design is wrong, then the design.

Everything described here is built and tested. The conformance suite runs
against two backends and passes; the Postgres adapter is verified against
PostgreSQL 16.

---

## Part 1 — Where the design is wrong

### 1.1 Three lanes is one too many, and the extra one is `Scheduled`

The three lanes are cut on two different axes at once.

`Transactional` and `Bulk` differ in **durability**: must this enqueue commit
with the caller's data, and can this work ever live somewhere else.

`Scheduled` differs in **storage access pattern**: a range scan over a moving
boundary rather than a claim.

Those are not comparable, and the cost of pretending they are shows up as
soon as you write a real job. "Cancel this order in thirty minutes unless it
is paid" is transactional — losing it is a billing bug — **and** it is
delayed. Under three lanes it has no lane. You end up enqueuing a
`Transactional` job whose only body is to enqueue a `Scheduled` one, which is
two rows, two runs and a window where the first committed and the second did
not.

A delay is a property of an enqueue, not a kind of job:

```rust
client.enqueue(CancelUnpaid { order }).after(Duration::from_secs(1800))
      .in_tx(&mut tx).await?;        // atomic and delayed
```

The storage separation you want is still there. It is just the backend's
business: a Postgres adapter keeps a far-future row in `queue_scheduled` with
its own index and its own autovacuum settings, and a promoter moves it into
the claim table as it comes due. The application never says "scheduled" and
never needs to.

**This split also pays for itself on the weak backends.** SQS caps a delay at
900 seconds and JetStream has no per-message schedule at all. Because
"delayed" is a storage decision and not a lane, the same promoter that exists
for Postgres's index shape is also the thing that makes a three-day delay
work on a backend whose own ceiling is fifteen minutes. As a lane, it would
simply have been unavailable.

**So: two lanes, `Transactional` and `Bulk`, plus `run_at` on every enqueue.**

### 1.2 `Transactional` does not belong behind the backend trait at all

This one follows from your own constraint and is worth making explicit,
because it changes the shape of the trait.

You wrote that `Transactional` "always lives in the application's own
database" and "never moves to another backend". Both are true. The
conclusion is that it is not a backend.

The mechanical version of the same point: "enqueue inside the caller's
transaction" means sharing the caller's connection. No `dyn` signature can
hand a `sqlx::Transaction<'_, Postgres>` to an SQS adapter. A trait that
tried would have to take an opaque handle and downcast, which converts a
compile error into a runtime one — in the lane whose entire purpose is that
it cannot fail.

So the crate has two write paths, and only one of them is dynamic:

```rust
pub trait Backend { .. }        // Bulk only. dyn, chosen from config.

pub trait TxEnqueue {           // monomorphic over the application's database.
    fn enqueue_in_tx(&mut self, lane: Lane, job: NewJob)
        -> impl Future<Output = Result<Pushed, EnqueueError>> + Send;
}
```

`TxEnqueue` takes the lane because it owns the mapping from lane to table:
`Transactional` goes to the application's job table, `Bulk` goes to the
outbox. That is the outbox, and it is also the proof that lane and call site
are different axes — a `Bulk` job enqueued atomically is a perfectly ordinary
thing to want.

Enqueuing a `Transactional` job outside a transaction is an error, not a
fallback. There is no correct thing to do.

### 1.3 Always-outbox is right inside a transaction and wrong outside one

Outside a transaction, routing `Bulk` through the outbox costs:

| path | writes |
|---|---|
| direct | 1 insert |
| outbox, unbatched | insert + relay read + backend write + delete |
| outbox, batched to the same Postgres | insert + 1/N of a batched move |

The lane whose stated constraint is **write churn** is the lane you would be
charging two to three times the writes. That is the wrong lane to spend
uniformity on.

The uniformity you actually need is **semantic**, not mechanical. State it
once and hold every path to it:

> Enqueue to `Bulk` is at-least-once. It may duplicate. Order is not
> promised. Visibility is not immediate.

A direct enqueue satisfies that. It is in fact *stronger* — no relay means no
redelivery window — and that is the only real argument for always-outbox:
applications come to depend on the stronger behaviour of the fast path and
then break when a transaction puts them on the slow one.

The cheap answer to that is not to make everything slow. It is to make the
weak contract testable. The conformance suite ships an `Adversary` wrapper
that uses every freedom the contract allows, including returning nothing from
the claim right after a push, and the suite runs twice and requires the same
result. Dependence on immediacy is then a test failure rather than an
incident.

**So: always-outbox inside a transaction, because that is the only thing that
works. Direct outside it, by default.** `RelayConfig` can force the outbox
everywhere for a deployment that would rather have the uniformity, and the
batched same-database relay keeps the cost near 2x rather than 3x.

One cost of always-outbox that the brief does not mention and that would have
bitten later: a single relay loop is a serialisation point in front of the
highest-volume lane. Draining it needs several relays, which needs a claim,
which needs the lease machinery the queue already has. The implementation
here uses `FOR UPDATE SKIP LOCKED` on the outbox for exactly that reason.

### 1.4 What is not implementable on SQS or JetStream

Checked against the real limits, not against a feeling:

| in the contract | SQS | JetStream | verdict |
|---|---|---|---|
| exclusive claim with a lease | yes | yes | required |
| extend a lease | `ChangeMessageVisibility` | `AckProgress` | required |
| expire a lease and redeliver | automatic | `AckWait` | required |
| arbitrary future delay | **900 s max** | **no per-message schedule** | capability |
| retry backoff | visibility timeout, **12 h max** | `NakWithDelay` | capability, with a ceiling |
| unbounded unique key | **5 min FIFO dedup window** | **configured window** | capability |
| cancel a pending job by id | **no** | **no** | capability, off by default |
| exact queue depth | approximate | approximate | capability |
| server-side filter by kind | **no** | subject filter, one per consumer | capability |
| payload size | **256 KiB** | **1 MiB default** | capability, checked at enqueue |
| in-flight limit | **120,000** | per consumer | capability |
| atomic enqueue with caller data | **no** | **no** | not in the trait at all (§1.2) |
| delivery order | no (standard) | per subject | never promised |

Two of these were latent in the brief and are worth calling out:

**Numeric priority cannot exist.** Postgres gives it away with `ORDER BY
priority, available_at`. SQS has no server-side ordering or filtering, so
priority means one queue per level. An API with a priority integer is an API
that cannot move. This design has **named queues and no priority**, which is
also what Sidekiq settled on.

**Uniqueness is the sharpest edge.** `UntilSuccess` needs a key held until a
job reaches a terminal state, and neither managed backend can do it; both
offer a bounded window. So uniqueness is declared as
`None | Window(Duration) | Unbounded`, and the mismatch is caught at **boot**:

```rust
let unmet = registry.check(&backend.capabilities());
// job "billing.charge" needs uniqueness UntilSuccess, the backend offers Window(300s)
```

A capability read at request time is a silent behaviour change — deduplication
that stops deduplicating. `Worker::new` refuses to start instead.

### 1.5 `dyn Backend` is right, but not for the reason it looks like

The performance argument is empty in both directions. A claim is a network
round trip and a job runs for milliseconds to seconds; one vtable lookup and
one box per call is noise. So the question is only about where the choice is
made.

The real argument for a type parameter is stronger than the brief gives it
credit for. Brocanth is a code generator. `brocanth create` already writes a
`plugins::Installed` type alias chain into the generated project, so it could
equally write `enum Backends { Postgres(..), Sqs(..) }` and dispatch
statically while still "choosing in config". That costs nothing at runtime and
keeps `async fn` in the trait.

What decides it is §1.4. **The capability set has to be discovered at runtime
anyway.** Backends differ in what they can do, the application has to be
checked against that at boot, and no static type can make the API's *shape*
depend on the backend — a `Capabilities` struct read at startup does that work
whether dispatch is static or not. Once that exists, the type parameter buys
only monomorphisation on a path dominated by I/O, and costs a generated enum
that every third-party backend has to be added to by hand.

**So `dyn`, and manual boxing rather than `async-trait`:**

```rust
pub trait Backend: Send + Sync + 'static {        // what adapters write
    fn claim(&self, req: &ClaimRequest)
        -> impl Future<Output = Result<Vec<Claimed>, BackendError>> + Send;
}

pub trait DynBackend: Send + Sync + 'static {     // what the worker holds
    fn claim<'a>(&'a self, req: &'a ClaimRequest)
        -> BoxFuture<'a, Result<Vec<Claimed>, BackendError>>;
}

impl<T: Backend> DynBackend for T { .. }          // the bridge, written once
}
```

An adapter writes plain `async fn`. The worker holds `Arc<dyn DynBackend>`.
No proc macro, no compile-time cost, and the lifetimes are visible to the
implementor instead of being generated behind a macro.

**The one wart**, found while writing the tests and worth knowing before you
adopt it: both traits carry the same method names, and the blanket impl gives
every concrete adapter both, so importing both into one scope is ambiguous.
In practice nothing needs both — an application imports neither, an adapter
author imports `Backend`, and anything holding a backend at runtime imports
`DynBackend` — but it is a sharp edge the first time you hit it.

---

## Part 2 — The design

### 2.1 The public API

```rust
#[derive(Serialize, Deserialize)]
struct SendWelcome { user: UserId }

impl Job for SendWelcome {
    const KIND: &'static str = "mail.welcome";   // the name on the wire
    const QUEUE: &'static str = "mail";
    const MAX_ATTEMPTS: u32 = 10;
    const BACKOFF: Backoff = Backoff::exponential();
    type Error = MailError;

    async fn run(&self, cx: &JobContext) -> Result<(), MailError> { .. }
}
```

Enqueue, four shapes, one builder:

```rust
client.enqueue(SendWelcome { user }).await?;                       // bulk, now
client.enqueue(Digest).after(Duration::from_secs(3600)).await?;    // bulk, delayed
client.enqueue(ChargeCard { order }).in_tx(&mut tx).await?;        // transactional
client.enqueue(SendReceipt { order }).in_tx(&mut tx).await?;       // bulk, via outbox
```

`job.enqueue(&client)` is also available, for the Rails reading order. There
is no ambient global client: a test that cannot isolate its queue is a test
suite that goes serial.

Retries, backoff and uniqueness are associated constants with defaults, and
every one of them is overridable per call:

```rust
client.enqueue(Reindex { shard })
      .max_attempts(3)
      .unique(Uniqueness::UntilSuccess)
      .await?;
```

**Where a derive macro earns its place: nowhere, in the core.** The trait asks
for one string and one method. A derive that generated `const KIND` from the
type name would hide the single declaration that has to survive a rename — the
wire name outlives the Rust name, and making it implicit is how you get a
silent dead-letter flood on a refactor. The type erasure a registry needs is a
generic function, `register::<T>()`, not a macro. A derive earns its place
later, if `Job` grows a dozen knobs; it does not now.

**The registry is explicit, and that is a deliberate rejection of `inventory`.**
`inventory` and `linkme` register through a link section before `main`. It
reads beautifully and fails badly: a job type in a crate that nothing else
references is dropped by the linker, and the symptom is a kind that is
silently never handled. In a framework whose modules are separate crates you
meet that on the first module. An explicit list is greppable, testable, and a
generator can write it — brocanth's `Slot` system already writes exactly this
kind of list.

**An unknown job kind during a rolling deploy** is the case that shapes the
settle enum. The old worker claims a job built after it shipped. Failing it
would burn the new job's retry budget and dead-letter it before the rollout
finishes. So:

```rust
Settle::Release { not_before, reason: ReleaseReason::UnknownKind }
```

A release hands the job back **without spending an attempt**, with a few
seconds of backoff so the same worker does not spin on the same row. Where
the backend can filter server-side the worker also sends its known kinds in
the claim, so it does not pick the job up at all. And `Observer::on_unknown_kind`
carries a running count: during a deploy it rises and falls, and a count that
does not fall means the new workers never arrived and the queue is not
draining.

### 2.2 The backend trait

Four required methods. Everything else defaults to
`Err(BackendError::Unsupported)`.

```rust
fn push(&self, batch: Vec<NewJob>) -> .. Result<Vec<Pushed>, BackendError>;
```
One result per input, in order. A collision with a live unique key is
`Pushed::Duplicate`, not an error: the caller asked for one copy and there is
one. **The batch is not atomic** — a partial failure returns `Err` and the
caller must assume any prefix landed. This is why the only atomic enqueue is
`TxEnqueue`.

```rust
fn claim(&self, req: &ClaimRequest) -> .. Result<Vec<Claimed>, BackendError>;
```
Take an exclusive lease on up to `req.max` jobs. **Returning nothing is normal**
and is not an error. The backend increments the attempt counter *as part of the
claim*, so a worker that dies silently still spends an attempt and a job that
kills its worker cannot loop forever. `req.kinds` is honoured only when
`filter_by_kind` is declared; otherwise it is a hint.

```rust
fn heartbeat(&self, lease: &LeaseRef, extend: Duration) -> .. Result<LeaseState, _>;
fn settle(&self, lease: &LeaseRef, outcome: Settle)     -> .. Result<LeaseState, _>;
```

Both carry the **fence token**, and this is the part most queue designs get
wrong. A worker stalls — a garbage collection pause, a blocked syscall — its
lease expires, another worker claims the job, then the first wakes and calls
`settle(Done)`. Without a token it deletes a row another worker is actively
running. With one, the backend compares and answers `LeaseState::Lost`, the
call changes nothing, and the worker drops its copy without reporting a
failure. A duplicate run is what at-least-once means; a deleted in-progress
job is data loss.

`settle` must be idempotent for the same token: a worker whose settle timed
out will try again.

`Settle` has four arms, and the fourth is the one that is usually missing:

- `Done` — remove it.
- `Retry { at, failure }` — claimable again at `at`; the attempt is spent.
- `Dead { failure }` — to the dead letter.
- `Release { not_before, reason }` — **hand it back, attempt not spent**. The
  shutdown path and the rolling-deploy path.

Optional, each behind a capability: `approx_depth`, `cancel`, `purge`,
`sweep`.

### 2.3 The Postgres backend

Full DDL with the reasoning beside each decision is in
[`crates/brocanth-queue-postgres/src/schema.sql`](crates/brocanth-queue-postgres/src/schema.sql).

**The claim.**

```sql
WITH picked AS (
    SELECT id FROM queue_bulk
     WHERE queue = $1 AND available_at <= now() AND attempts < max_attempts
       AND ($4::text[] IS NULL OR kind = ANY($4))
     ORDER BY available_at
     FOR UPDATE SKIP LOCKED
     LIMIT $2
)
UPDATE queue_bulk j
   SET lease_token  = gen_random_uuid(),
       available_at = now() + make_interval(secs => $3),
       attempts     = j.attempts + 1
  FROM picked WHERE j.id = picked.id
RETURNING ..;
```

Without `SKIP LOCKED` every worker queues behind the first one's row lock and
the claim serialises.

**`available_at` is the whole mechanism.** It means "not claimable before
this", and a claim sets it to the lease end. So a lease *is* a row that is not
claimable yet, and **lease expiry needs no sweeper**: a worker that dies
leaves a row that becomes claimable again on its own. One column, one index,
and the failure mode that matters most needs no background process.

One consequence to keep in mind: an expired lease still carries the dead
worker's token. Nothing clears it, because expiry is a timestamp passing and
not an event anybody runs. Any query that means "not currently held" must say
`available_at <= now()` and never `lease_token IS NULL`. (The first version of
the sweep here got that wrong and left exhausted jobs in the table forever.
The test for it is `a_job_whose_worker_dies_every_time_reaches_the_dead_letter`.)

**Partitioning: on the archive, not on the queue.** The brief asks for `Bulk`
to be partitioned so cleanup is `DROP PARTITION`. That does not work, and the
reason is worth stating:

- What you want to clean up is finished jobs.
- A partition cannot be dropped while one row in it is still retrying.
- A retry can be days after the enqueue.
- So every partition keeps a long tail of stragglers, and you are back to
  row-by-row `DELETE` plus a partitioned table that the claim has to scan.

The shape that `DROP PARTITION` actually fits is append-only and strictly
ordered by the partition key. That is the archive, not the queue. So:

- `queue_bulk` **deletes on success**. It stays small, which is the thing that
  keeps the claim index shallow. No partitions.
- `queue_archive` is `PARTITION BY RANGE (finished_at)`, one partition per
  day, dropped whole. No row is ever deleted from it, so autovacuum never
  visits it. Retention is one `DROP TABLE` a day.
- The archive is opt-in (`PgConfig::archive`). Off, `Settle::Done` is a plain
  `DELETE`.

*An option that is defensible and is left off:* hash-partitioning the live
`queue_bulk` by id, with each worker preferring one partition, splits the
index hot spot and the page contention. It is only sound because the `Bulk`
lane promises no ordering — the moment someone wants global `ORDER BY
available_at` it breaks. Worth having when one table's write rate becomes the
ceiling; not worth the operational surface before that.

**`queue_scheduled` is separate**, for three reasons, in order of weight:

1. It is what makes `max_delay` unbounded on a backend that has a ceiling. A
   promoter holds the row until it is nearly due and then pushes it, so a
   three-day delay works on SQS, which caps a delay at fifteen minutes.
2. The vacuum settings disagree. The claim target churns hard and wants
   `autovacuum_vacuum_scale_factor = 0`; the delayed set is nearly static and
   does not.
3. The access patterns disagree. The claim reads the left edge of a hot index;
   the promoter range-scans a moving boundary. Mixing them makes the claim
   index hold rows the claim never reads.

Honest limitation: the scheduled index's left edge is deleted constantly,
which bloats a btree, and no storage parameter prevents that. The answer is
aggressive autovacuum plus a periodic `REINDEX CONCURRENTLY`. The schema says
so in a comment, so nobody goes looking for a cleverer setting.

**Storage parameters, with the reasoning:**

```sql
WITH (
    fillfactor = 70,
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 1000,
    autovacuum_vacuum_cost_delay = 0,
    autovacuum_analyze_scale_factor = 0,
    autovacuum_analyze_threshold = 1000
)
```

`autovacuum_vacuum_scale_factor = 0` with a flat threshold: the default `0.2`
means a ten-million-row queue waits for two million dead tuples before it
vacuums, by which point the claim index is bloated and the claim is slow. A
queue wants a vacuum at a fixed small number of dead rows and wants it at full
speed, hence `cost_delay = 0`.

`fillfactor = 70`, and **not** for the reason it is usually given. A claim
updates `available_at`, which is indexed, so the update can never be HOT. No
setting can change that: the index is what makes the row invisible. Fillfactor
still pays — the new tuple version stays on the same page, so the heap write
is local and pages split less — but it does not avoid the index write, and
believing otherwise leads to tuning the wrong thing. The real lever is
deleting on success, so there are fewer dead tuples at all.

The transactional table gets the same shape with a lower threshold (it is
small by construction). The dead letter gets defaults: it is read by a person.

**A bug worth repeating, because it is not Postgres-specific.** The first
version wrote `available_at` as a timestamp computed by the client and
compared it against the server's `now()`. Running the tests against a Postgres
whose clock was 15 ms behind the host made a job with no delay invisible for
15 ms — and the symptom is "the queue is mysteriously slow to pick up new work
and perfectly fine under load". Multi-host workers have exactly this problem
and it does not go away with NTP. Every statement in the adapter now writes
`now() + make_interval(secs => $n)` and never a client timestamp. Nothing
compares two clocks.

### 2.4 The outbox relay

```sql
CREATE TABLE queue_outbox (
    id bigserial PRIMARY KEY, kind text, queue text, payload bytea,
    run_at timestamptz, max_attempts int, unique_key text, uniqueness text,
    available_at timestamptz NOT NULL DEFAULT now(),   -- the relay's own lease
    attempts int NOT NULL DEFAULT 0, last_error text
);
```

The relay claims with the same `FOR UPDATE SKIP LOCKED` as the queue, for the
same reason: several relays must not serialise behind each other. The same
`available_at`-as-lease trick means a relay that dies strands nothing.

The loop is one batch per tick: fetch, `push`, delete what was acknowledged.
`Relay::tick()` is public and returns what it did, so a test drives it without
a timer.

**The guarantee is at-least-once and nothing more.** The relay writes to the
backend and then deletes the row; a crash in between redelivers. Every `Bulk`
job is idempotent by contract, and that is what pays for it. What the outbox
*does* add is the only thing it exists for: a job is never delivered for a
transaction that rolled back, and never lost for one that committed.

**When the bulk backend is unavailable**, the relay defers the whole batch
with exponential backoff, nothing is deleted, and the table grows. The
undelivered count is then the only honest signal, so the writer reads it:

```rust
client.enqueue(SendReceipt { order })
      .in_tx(&mut Tx::new(&mut tx).max_pending(100_000))
      .await?;      // Err(EnqueueError::OutboxFull { rows, limit })
```

Past the limit the enqueue fails and takes the caller's transaction with it.
That is the default (`OnOutboxFull::Reject`) and it is the right default:
accepting work that cannot be delivered turns a visible outage into a silent
backlog, and the backlog is discovered when the disk fills. `OnOutboxFull::Accept`
exists for applications that would rather keep serving.

The limit check is a **bounded** count — `SELECT count(*) FROM (SELECT 1 FROM
queue_outbox LIMIT $1)` — so it costs the same whether the outbox holds a
thousand rows or ten million. A check that gets slower as the backlog grows is
a check that fails exactly when it is needed.

### 2.5 The conformance suite

`crates/brocanth-queue-conformance` runs 21 cases against `Arc<dyn DynBackend>`.
Nothing in it names Postgres, SQL, a connection or a transaction.

Each case declares the capabilities it needs and is **skipped, not failed**,
when the backend does not declare them — so an adapter that supports less is
still held to everything it claims. The report distinguishes pass, fail and
skip.

Three things make it more than a smoke test:

**1. The `Adversary` wrapper.** It wraps any backend and uses every freedom
the contract allows:

- `claim` returns fewer than asked, never more than two.
- The returned batch is reversed on every other call.
- The first claim after a push returns nothing — *visibility is not promised*.
- `approx_depth` is off by one unless `exact_depth` is declared.

The suite runs twice and requires the same result case by case. A case that
passes bare and fails hostile is a case that encodes a Postgres assumption.
This is the mechanism that catches "enqueue then immediately claim", which is
the single most common leaked assumption, and it catches it without an AWS
account.

**2. A reference backend.** `brocanth-queue-memory` is an in-process
implementation with the full lease and fence semantics. A conformance suite
that has never passed proves nothing.

**3. The same store, twice, with different capabilities.**
`Memory::sqs_shaped()` is the same storage declaring a managed queue's limits:
256 KiB payloads, a 900-second delay ceiling, no kind filter, a 300-second
dedup window. Running both proves the capability gates do something, and a
test asserts that no case is skipped by *both* — a gated case nothing ever
runs is a case that proves nothing.

Current state:

Current state — `cargo run -p brocanth-queue-postgres --example report`:

```
conformance: memory    — 19 passed, 0 failed, 2 skipped   (unbounded)
  skip  a delay past the ceiling is refused (the backend has no delay ceiling)
  skip  a payload over the limit is refused, not cut (the backend has no payload limit)
conformance: adversary — 19 passed, 0 failed, 2 skipped   (same, hostile)
conformance: memory    — 19 passed, 0 failed, 2 skipped   (sqs_shaped)
  skip  a kind filter is honoured (the backend cannot filter by kind)
  skip  cancel removes a job that has not started (the backend cannot cancel a pending job)
conformance: postgres  — 19 passed, 0 failed, 2 skipped   (queue_bulk)
conformance: postgres  — 19 passed, 0 failed, 2 skipped   (queue_tx)
```

Postgres skips the two limit cases because it declares no limits, which is
the gate working. Between the unbounded and the `sqs_shaped` runs every case
executes at least once, and a test asserts it.

### 2.6 Rust specifics

**Pool size is the concurrency ceiling, and the real failure is worse than
slow.** If job bodies and the queue's own bookkeeping share one pool, then
`concurrency` running jobs can hold every connection, the heartbeat cannot get
one, **every lease expires, and every job is claimed again by another worker
that does the same.** A saturated pool becomes a redelivery storm.

So the queue takes a **separate, small control pool** for claim, heartbeat,
settle and sweep, never shared with job bodies, and the arithmetic is checked
at boot:

```rust
Budget { app_pool, control_pool, concurrency, relays, listener, headroom }
    .check()?;
// Err: the control pool has 2 connections and needs 5 for concurrency:
//      claim, heartbeat and settle must never queue behind a job.
//      Raise the pool or lower the concurrency.
```

Three for the queue's own calls, one per relay, one for the listener. The
application pool needs `concurrency + headroom`. A process that cannot work
refuses to start, which is the only way to make the symptom resemble the
cause.

`WorkerConfig::check()` separately refuses a heartbeat that cannot outrun its
lease — anything slower than a third of it, because one missed beat must not
lose the job.

**`LISTEN/NOTIFY` is a latency optimisation and never the pickup mechanism.**
`PgListener` holds a connection for its whole life, so it is counted in the
budget. Notifications sent while it is reconnecting are gone. `Worker::wake_on`
takes a `watch::Receiver` that can only ever shorten a poll; a lost
notification costs latency and never a job.

**Cancellation safety** is handled in four places, because a job future can be
dropped at any await:

1. Each job runs as its own task, so a panic is contained and arrives as
   `JoinError::is_panic()` with the message, instead of killing the
   supervisor and leaving the lease to expire.
2. The heartbeat runs in the supervisor's `select!`, not in a detached task,
   so it cannot outlive the job and keep extending a lease on a row someone
   else now owns. The fence token makes that safe even if it did.
3. `LeaseGuard` releases the lease on drop. `Drop` cannot await, so it spawns,
   and a runtime that is already shutting down may never run it — which is
   exactly why lease expiry is the backstop. The guard shortens the window; it
   does not replace it.
4. Graceful shutdown sets a flag the job can see (`cx.is_stopping()`), waits
   out `shutdown_grace`, then aborts what is left and settles
   `Release { Stopping }`. A shutdown is not a job failure and does not spend
   an attempt. The test for this is
   `a_shutdown_gives_a_running_job_back_instead_of_failing_it`.

A settle that fails transiently is retried, because a settle that never lands
leaves the job to expire and run a second time.

**What `serde` imposes on job types:** `Serialize + DeserializeOwned + Send +
Sync + 'static`. No borrowed payloads, no trait objects without a
`#[serde(tag)]` scheme, and — the one that bites — **payload compatibility is
a deployment constraint**. A field added without a default breaks every job
already in the queue, and the failure arrives as `FailureKind::Undecodable`,
which is deliberately **not retryable** and goes straight to the dead letter.
Retrying a payload the code cannot parse only burns attempts.

---

## Part 3 — Open decisions

These four were left open here and settled in `DECISIONS.md`, which records
the argument on each side and the four defects the argument uncovered.

**`Uniqueness::UntilStart`** — removed. It needed the same unbounded key
store `UntilSuccess` needs, so it bought no portability, and it cannot
re-acquire its key after a failed attempt. River and Oban both made the
equivalent inexpressible, for correctness.

**Relay placement** — in-process, and now wired, which it was not. `SKIP
LOCKED` plus a lease means N relays need no leader election, which is the
cost Oban pays for the same placement.

**Archive** — on, at seven days, once the sweep was made to carry the
partitions forward. The write is a CTE around the `DELETE` that already
happens, into a table with no index.

**`max_claim_batch`** — 64, in `PgConfig`. The worker already claims no more
than its free permits, so the old 1000 did nothing; it still mattered as a
published capability.
