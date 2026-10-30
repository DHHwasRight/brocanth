# Decisions

`DESIGN.md` Part 3 left four decisions open. This file settles them, records
what the argument was on each side, and lists the four defects the argument
uncovered — which mattered more than the decisions did. Part 3 covers the
read side, which `DESIGN.md` never described at all. Part 4 gives the
outbox a dead letter, which Part 5 used to list as open.

Everything here is implemented. The test named beside each item is the one
that fails if it is undone.

---

## Part 1 — The defects

Three of the four open decisions turned out to rest on a loop that did not
run. The decisions were sound; the machinery under them was missing.

### 1.1 The sweep could not run on a queue that had work

`sweep.tick()` was an arm of a `select!` that the claim loop only reached
when a claim came back empty. A queue with a standing backlog never swept.

What stopped: the promoter that moves `queue_scheduled` into `queue_bulk`,
which is step 1 of the sweep. **Every job enqueued with `run_at` further out
than `schedule_after` (300 s) silently never ran.** Also stopped: exhausted
rows reaching the dead letter, and `queue_dedup` shrinking.

The failure needed load to appear, which is why no test found it. The
end-to-end tests all drained their queue, so every one of them reached the
empty-claim branch.

The sweep now runs on its own task. A backend that answers `Unsupported` is
asked once and then left alone, so a backend with no sweep does not fill the
log with the same error for the life of the process.

> `the_sweep_runs_while_the_queue_is_never_empty`, which keeps a feeder
> running so no claim ever returns nothing.

### 1.2 Nothing ever started the relay

`Relay` was public, documented, tested in isolation and constructed by no
one. `Budget::control_needed` reserved a connection for it. `DESIGN.md` said
"the default wiring is in-process". There was no wiring.

A process that enqueued a bulk job inside a transaction therefore wrote a row
to `queue_outbox` that no reader ever came for.

`Tx::new` also set `max_pending: None`, and `RelayConfig::max_pending` and
`on_full` were read by no code at all. So the back-pressure that §2.4
describes — refuse the enqueue rather than grow a backlog nobody can see —
was not merely off by default, it was unreachable.

`Worker::relay()` now takes a relay and runs it. `Tx::new` starts at
`DEFAULT_MAX_PENDING`; `Tx::unbounded` is the opt-out.

> `a_worker_relay_delivers_what_a_transaction_left_behind`.

### 1.3 Archive partitions stopped two days after boot

`ensure_archive_partitions(pool, 2)` ran once, in `migrate`. Its own doc
comment said "call it at boot and on a timer". There was no timer.

With `archive` on, day three has no partition to insert into. The archiving
`Settle::Done` then fails; the error is not transient, so `settle_with_retries`
gives up without deleting the row; the lease expires and the job runs again.
A retention detail becomes duplicate execution across the whole queue.

The sweep now carries the partitions forward and drops the ones past
retention. Only the `Bulk` table does it, so two queues on one database do not
fight over the same partitions.

Doing it from every worker exposed a second problem: `CREATE TABLE IF NOT
EXISTS` is not atomic against another session running the same statement.
Both see the table missing, both create it, and the loser gets a
duplicate-object error instead of the silence `IF NOT EXISTS` implies. That
is the normal outcome on a fleet, so `42P07` and `23505` are now not errors
here. The workspace's own Postgres suite reproduced this immediately: eight
tests calling `migrate` at once failed seven times.

> `the_sweep_carries_the_archive_partitions_forward`,
> `a_finished_job_is_archived_by_default`.

### 1.4 Two smaller ones

**The claim was raced against the stop signal.** Dropping a claim future does
not undo it: the `UPDATE ... RETURNING` commits on the server, and the rows
come back to nobody — an attempt spent, a lease token no process has seen,
invisible until it expires. The claim is now awaited to completion. The cost
is one round trip of shutdown latency.

**A failed heartbeat was reported and ignored.** The job kept running past a
lease another worker was entitled to claim, so two workers ran the same body
with neither aware of the other. `LeaseRef::expires_at` was read by nothing.
The worker now stops at the expiry it was given, aborts the body and settles
nothing — what it already did for a lease it was told it had lost.

> `a_job_whose_lease_expires_is_abandoned_and_not_settled`.

---

## Part 2 — The four decisions

### 2.1 `Uniqueness::UntilStart` — removed

Hold the key while the job waits, release it when a worker picks the job up.

| Keep | Remove |
| --- | --- |
| The only spelling of "collapse the backlog, let a retry coexist" | It cannot be re-acquired: a job that is picked up and fails holds no key for its whole backoff, hours on the default schedule |
| Two lines of implementation, already written | It bought no portability — it needs the same unbounded key store `UntilSuccess` needs |
| The boot gate makes the limitation loud | `Registry::check` fails the whole process, so one such job blocks a migration rather than staying behind |
| | Zero test coverage: `grep UntilStart` found nothing outside the source |

**Removed.** The deciding evidence was the field, not the code. River made
the pending states non-removable from a unique specification (PR #590) and
Oban warns at compile time on the partial state lists that expressed the same
idea — both because duplicates land in the gaps between states. Of the four
systems that still ship it, all four document advice against using it.

The case it served is a periodic tick that must not pile up. That belongs to
whatever schedules the tick, which can ask "is a live copy present" in one
query, on any backend.

`Window(Duration)` is the portable spelling. `UntilSuccess` is the
Postgres-grade one.

### 2.2 Relay placement — in-process, and now actually wired

| In the worker | Its own process |
| --- | --- |
| No second deployable, which is the entire premise for an app on one Postgres | It dies with the most frequently restarted process |
| `FOR UPDATE SKIP LOCKED` plus a lease makes N relays safe with no leader election | A worker fleet is sized by job runtime, not relay throughput |
| `Budget` already reserves its connection, so it cannot starve a job | A window with no worker leaves the outbox unread and nothing says so |

**In-process stays the default.** The usual price of that choice is
coordination: Oban runs the same kind of loop inside the app and pays for it
with a leader election, a 30 s lease and a documented failure mode when
leadership cannot transfer. This design does not need any of it, because the
outbox claim already takes rows with `SKIP LOCKED` under a lease. Rails ships
the opposite default — `solid_queue` forks a separate dispatcher — but it also
ships `SOLID_QUEUE_IN_PUMA` for small apps, which is this case.

Two things were changed rather than left as written:

- The relay outlives the jobs. A job finishing inside the shutdown grace can
  still enqueue bulk work, so the relay is stopped last and then drains.
  `RelayConfig::drain` bounds that.
- `Tx::new` defaults to a limit. An unbounded outbox is not the safer
  default: when the backend is unreachable the table grows at the rate the
  application writes, silently, and the outage is found when the disk fills.

**Open, and the one fact that settles it:** whether worker replicas can reach
zero while the web tier serves. If they can, the relay also belongs in the web
process — and that needs a `Budget` constructor for a process that runs no
worker, which does not exist yet.

### 2.3 Archive — on by default, seven days

| Off | On |
| --- | --- |
| The live table staying small is what keeps the claim fast | The write is one statement: `WITH gone AS (DELETE … RETURNING) INSERT …` — same transaction, same commit, no extra round trip |
| Extra WAL on the lane whose stated constraint is write churn | `queue_archive` carries no index at all, so there is no index maintenance, and nothing is ever deleted from it so autovacuum never visits |
| 24 hours answers almost no real question | Partition-per-day means retention costs one `DROP TABLE` |
| | Nobody turns a history on before the incident that needs it |

**On, at seven days.** The field is unanimous on the default: River keeps
completed jobs 24 h, `solid_queue` 1 day, Oban's own installer writes 1 day,
GoodJob 14. Seven rather than one: a job that fails quietly on a Friday
evening is asked about on Monday, and a one-day window has already dropped
the answer.

GoodJob is the precedent worth copying exactly — it flipped this default to
on **in the same release** that made cleanup automatic. Which is why §1.3 had
to be fixed before this decision could be taken at all.

**Recorded limitation:** the archive stores *settles*, not *executions*. A job
that ran, had its effect, then lost its lease and was retried leaves one row,
not two. Under at-least-once delivery it is a lower bound on what ran, never a
count of it. That is in the `PgConfig::archive` doc comment, because the wrong
reading — "no row means it did not run" — is exactly the reading someone
reaches for during the incident the archive exists for.

### 2.4 `max_claim_batch` — 64, and it was never the lever

The brief asked whether 1000 is the right number. It was not a number that
did anything: `worker.rs` already claims `free.min(min(max_claim_batch,
concurrency))`, so with the default concurrency of 8 the effective batch is 8.
The declared 1000 was unreachable through `Worker`.

It still mattered as a published capability, because a caller driving the
backend directly gets what it declares. One claim writes one `available_at`
for the whole batch, so every row in it shares a deadline: a batch larger than
the caller can start inside one lease expires while it is still held, with an
attempt already spent on each row.

**Set to 64 and moved to `PgConfig`.** The field's range is 1 to about 50 —
River fetches exactly its free slots and says in a comment that it chose
latency over prefetch throughput; Oban demands only what it can run;
`solid_queue` claims its available capacity; SQS caps a receive at 10.

**Not done, deliberately:** the suggestion to validate `concurrency × job
duration` against the lease in `WorkerConfig::check`. Job duration is not
knowable at boot, and because the batch is already bounded by free permits
there is no queued remainder to go stale. The check would reject
configurations that work.

---

## Part 3 — The screens

`DESIGN.md` never said how an operator sees the queue, and `Backend` has no
method that reads. That was deliberate — the contract is written against
SQS, which cannot list its own messages — but it left the queue with no way
to answer "what is waiting", "what is running", "what gave up".

### 3.1 A separate trait, not more of `Backend`

`Inspect` and `DynInspect` sit beside `Backend` and `DynBackend`. An adapter
that can serve the screens implements it; one that cannot simply does not,
and the application holds `Option<Arc<dyn DynInspect>>`. A backend that
cannot answer gives no panel rather than a panel that lies, and the question
is settled at boot.

Putting `list` in `Backend` would make every adapter answer a question the
weakest one has no way to answer, which is the one rule this crate has.

### 3.2 Four states, derived and not stored

There is no `state` column and adding one is the mistake the schema exists
to avoid. A state is a predicate over the two columns the claim already
maintains:

| View | `available_at` | `lease_token` | `attempts` |
| --- | --- | --- | --- |
| Ready | past | — | under the limit |
| Running | future | set | — |
| Retrying | future | null | above zero |
| Scheduled | future | null | zero |

Two processes serving these screens always agree, because both read the same
rows. Nothing caches and nothing is session state, so a panel mounted in
three replicas behind a load balancer needs no sticky sessions.

The memory backend computes the same four from different columns. A case
that passes on one adapter and fails on the other is the reason both are
tested.

### 3.3 Two things were not in the datastore

**The worker name was thrown away.** `ClaimRequest::worker` was documented
as "a name for the claiming process, for the admin screens" and no backend
stored it. Without it a worker roster has to be its own table with its own
heartbeat, which is a second kind of state that can disagree with the first.
It is now `claimed_by` and `claimed_at` on the job row, written by the claim,
which was already an `UPDATE` of that row. No index: the running screen reads
the right edge of the claim index, and a partial index would add a write to
every claim and every settle to speed up a button a person presses.

**A dead row could not be rebuilt.** `queue_dead` stored no `max_attempts`
and no `unique_key`. A dead letter you cannot requeue is a log line with
extra steps.

### 3.4 What the buttons refuse

`run_now` and `discard` refuse a job a worker is holding. Clearing a live
lease is how two workers come to run one job, which is the thing the lease
exists to stop. An *expired* lease still carries the dead worker's token —
expiry is `available_at` passing, not an event anybody runs — so an orphaned
job can still be pushed along, which is the case the button exists for.

`run_now` does not reset `attempts`. The retry budget is the operator's to
spend and an impatient click should not buy another one.

`requeue` is two statements in one transaction and not one CTE: the delete
must not happen when the insert does not. A unique key the live table still
holds makes the insert do nothing, and the corpse stays, because it is the
only record of why that one died.

### 3.5 The graph, which §2.3 made possible

The panel used to say "no graph: this queue deletes the row when the job
succeeds, so the history that graph needs does not exist". With §2.3's
seven-day archive partitioned on `finished_at` it is one aggregate per
partition, and the range prunes to the days it touches.

`Inspect::finished` reads it and the History tab draws it: twenty-four
hourly bars, a per-kind table under them. The tab is absent when
`has_history` is false, and the route is refused as well — a pasted URL is
how somebody reaches a screen the tab was hiding. A chart of zeroes reads
like an outage, so a queue that keeps nothing shows no chart.

The bucket is **epoch arithmetic, not `date_trunc`**. `date_trunc`
truncates in the session's time zone, so the same query over the same rows
returns different buckets depending on a setting the caller cannot see, and
the axis the caller drew will not line up with them. Both ends floor
`epoch / width * width` instead, which is the same number in every session.

The axis is generated and then filled. An hour with nothing finished has no
row, and a chart that simply omits it reads as a quieter day than it was.

It counts **settles, not executions**, for the reason §2.3 records — and the
screen says so in a line under the chart, because the wrong reading is the
one somebody reaches for during the incident the history exists for.

---

## Part 4 — The outbox dead letter

`tick` deferred the whole batch on any error and never capped attempts. So
one row the backend will never accept — a payload over an SQS limit — held
back the 500 rows batched with it on every tick, and then the table. The
outbox had no way out for a row and no budget for one.

### 4.1 Each row has its own outcome

A batch the backend refuses is split, and the relay pushes each row alone.
`push` returns one error for the whole batch, and the error does not say
which row caused it. A push per row finds out, and lets every other row
through. Rows the batch already wrote are written again; at-least-once
allows that. The split costs one round trip per row, once, on the tick
that meets the bad row.

| The backend answers | The relay |
| --- | --- |
| accepted, or a duplicate | deletes the row |
| `Rejected` | buries the row at once |
| `Other` | defers the row on its own backoff, buries it at `max_attempts` |
| `Unavailable` | releases the row and every row after it |

`BackendError::Rejected` already meant "retrying the same call cannot
help", so the distinction needed no new variant. A row that gets it does
not wait for the cap.

### 4.2 An outage spends no attempt

The fetch spends an attempt on each row, as the claim does for a job. A
relay that dies holding a row has then still spent it, so a row that
crashes the relay cannot loop for ever: the next tick finds it past the cap
and buries it unpushed.

`Unavailable` is not the row's fault. `Outbox::release` gives the attempt
back, which is what `Settle::Release` does for a job. Without it an hour of
downtime moves the whole outbox into its dead letter, and the cap would
measure the outage instead of the row.

Only an outage slows the relay. A row that fails on its own waits on its
own backoff, so the next tick runs at once for everything else.

`max_attempts` defaults to 25: about ten minutes on the default backoff.

### 4.3 Its own table, not `queue_dead`

`queue_outbox_dead` is a second dead letter, and `OutboxInspect` reads it
with the four calls `Inspect` has for jobs — `dead`, `dead_one`, `requeue`,
`forget` — over the same `DeadRow`.

| Reuse `queue_dead` | Its own table |
| --- | --- |
| One screen, one table | A row here never reached the backend. A requeue must put it back in the outbox, not on a queue the backend may not even own |
| | `queue_dead` cannot rebuild the job: it has no `run_at` and no `uniqueness` |
| | `queue_dead` already mixes two lanes (Part 5); a third source makes it worse |

**Its own table.** The requeue is one CTE, where the job requeue is two
statements. The job requeue can meet a live unique key and must then keep
the corpse; `queue_outbox` has no unique index, so its insert always
writes.

`OutboxInspect` is a separate trait from `Outbox` so a panel can hold
`Arc<dyn DynOutboxInspect>`, as it holds `DynInspect`. Unlike `Inspect`,
every outbox store can serve it, because the outbox is always a table the
application owns.

The memory crate now has `MemoryOutbox`, so the outbox suite in
`brocanth-queue-conformance` runs against both stores.

> `one poisoned row does not block its batch`,
> `a row that keeps failing reaches the dead letter at the cap`,
> `an outage spends no attempt`,
> `a row that is never settled is not fetched for ever`,
> `a dead row can be requeued` — the outbox suite, on memory and Postgres.

---

## Part 5 — What is still open

- **Where the relay lives when no worker runs.** See §2.2.
- **A backend cannot reject one row of a batch.** `Pushed` has no
  per-row failure, so the relay splits a refused batch to find the bad row
  (§4.1). SQS `SendMessageBatch` answers per entry. An SQS adapter wants
  `Pushed::Rejected`, and then the split is only the fallback.
- **The outbox dead letter has no screen.** `DynOutboxInspect` exists;
  `brocanth-jobs` does not mount it yet.
- **`max_claim_batch = 64` is still a guess**, now a defensible one. The
  measurement that would settle it is p99 claim latency and WAL bytes per
  claimed row against batch size and concurrent claimers. A pgbench script
  over the claim CTE produces it in about twenty minutes.
- **Retired job kinds are never cleaned up.** A row whose `kind` nothing
  registers is not claimed, and is not swept either because its attempts are
  not spent. It sits at the front of the claim's scan order indefinitely.
  The screens make it visible for the first time: it is a `Ready` row whose
  age never falls.
- **Where the panel lives.** The screens exist as a trait; the HTML does
  not. `brocanth-jobs` already ships six of them — a dashboard, running,
  retries, scheduled, dead, every button a POST, locked behind a policy that
  denies by default — against its own store. Pointing them at `Inspect`
  makes one panel serve both queues, and costs `brocanth-jobs` a dependency
  on this crate with no new transitive dependency at all: the core names
  serde, serde_json, thiserror and tokio, which is the set it already has.
  What has no answer yet is how the two repositories refer to each other.
  Neither has a remote, so a git dependency is not available and a relative
  path makes them one unit without making them one workspace.
- **The dead letter is one table for two lanes.** `queue_dead` has no column
  saying which of `queue_bulk` and `queue_tx` a row came from, so a
  transactional queue's screens list the bulk lane's corpses too. Splitting
  on the `queue` name works and is what a deployment should do; a column
  would be exact.

---

## Verified

```
cargo test --workspace                      58 passed, 0 failed
cargo clippy --workspace --all-targets      clean under -D warnings
cargo fmt --all --check                     clean

(15 of those are the Postgres suite, which returns early with no
server. With one, they run for real:)

PostgreSQL 16, BROCANTH_QUEUE_REQUIRE_POSTGRES=1
  cargo test -p brocanth-queue-postgres     15 passed, 0 failed, three runs

conformance: memory    — 19 passed, 0 failed, 2 skipped   (unbounded)
conformance: adversary — 19 passed, 0 failed, 2 skipped
conformance: memory    — 19 passed, 0 failed, 2 skipped   (sqs_shaped)
conformance: postgres  — 19 passed, 0 failed, 2 skipped   (queue_bulk)
conformance: postgres  — 19 passed, 0 failed, 2 skipped   (queue_tx)
```

The two skips are the payload-limit and delay-ceiling cases. Postgres
declares neither limit, so the cases gate off rather than passing silently.

After Part 4:

```
cargo test -p brocanth-queue -p brocanth-queue-memory \
           -p brocanth-queue-postgres -p brocanth-queue-conformance
                                            60 passed, 0 failed
cargo clippy --all-targets                  clean under -D warnings
cargo fmt --check, the four queue crates    clean

PostgreSQL 15, BROCANTH_QUEUE_REQUIRE_POSTGRES=1
  cargo test -p brocanth-queue-postgres     16 passed, 0 failed

outbox: memory    — 5 passed, 0 failed
outbox: postgres  — 5 passed, 0 failed
```

A mutation check: with the refund taken out of `PgOutbox::release`, `an
outage spends no attempt` fails on Postgres.
