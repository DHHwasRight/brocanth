# brocanth-jobs

Background jobs for brocanth: typed jobs, delayed and recurring jobs,
events and an admin panel. The engine under it is `brocanth-queue`: its
worker claims a job under a lease, retries a failure with exponential
backoff and jitter, and moves a job that runs out of attempts to a dead
letter with its payload and its error, where an operator can requeue it.
The store is a Cargo feature: Postgres by default, memory for development
and tests. It is a plugin, so an application that does not register
`JobsPlugin` links no Tokio, no `sqlx` and no queue.

Use it for work that must not run inside the request that asked for it:
mail, reports, a nightly sweep. This is the queue that `brocanth create
--with jobs` installs. There is one job system: a job enqueued here is a
`brocanth-queue` job, and `brocanth-queue/docs/DECISIONS.md` Part 6
records the move.

## Getting started

```toml
[dependencies]
brocanth-jobs = { git = "https://github.com/DHHwasRight/brocanth.git" }
# For tests and development, in memory:
# brocanth-jobs = { git = "...", features = ["memory"] }
```

Inside this workspace: `brocanth-jobs.workspace = true`.

| Feature | Default | What it adds |
|---|---|---|
| `runtime` | on | The queue itself: everything but `scaffold`. Each store turns it on. |
| `postgres` | on | The Postgres store (`brocanth-queue-postgres`). The claim is `FOR UPDATE SKIP LOCKED`. |
| `memory` | off | The memory store (`brocanth-queue-memory`), for tests and development. Jobs are gone at restart. |
| `admin` | off | `admin::JobsPanel`, the queue's tab in `brocanth-admin`. |
| `scaffold` | off | The `jobs` and `jobs-memory` modules for `brocanth create`. Only the CLI turns it on. |

With no store, the build fails with a message that says which feature to
turn on. Postgres is the only database (2026-11-11): RLS and one database
keep the framework simple.

### As a module

```sh
brocanth create my-app --with jobs
# or, in an existing project:
brocanth add jobs
```

The module writes `src/plugins/jobs/mod.rs`: a plugin that opens the
queue, starts a worker in the process, and puts the `Queue` on the request
context. Where the jobs live depends on the other modules:

- with `db`, the queue runs on the `db` pool, and `[jobs]` has no `url`;
- without `db`, the queue opens its own pool from `[jobs] url`;
- with `jobs-memory` (a child of `jobs`, off by default), the queue lives
  in the process. `brocanth check --deploy` reports it as an error
  (`deploy/development-only`).

With `admin`, the queue is a tab at `/admin/jobs`. It is an integration:
`integrate.admin` in `src/modules/jobs.rs` turns it off.

The `[jobs]` section holds these settings:

| Key | Default | Flag | Environment | When |
|---|---|---|---|---|
| `url` | `postgres://localhost/development` | none (secret) | `$<APP>_JOBS__URL` | without `db` and `jobs-memory` |
| `migrate` | `true` | `--jobs.migrate` | `$<APP>_JOBS__MIGRATE` | without `jobs-memory` |
| `worker` | `true` | `--jobs.worker` | `$<APP>_JOBS__WORKER` | always |
| `queues` | `["default"]` | none (a list) | by index, with `config` only | always |
| `concurrency` | `5` | `--jobs.concurrency` | `$<APP>_JOBS__CONCURRENCY` | always |

`url` has no flag because a URL can carry a password, and every user on the
host can read a process's arguments. `migrate` is for development. In
production, put the statements from `Queue::schema_sql` in a migration
somebody reviewed. With the `config` module, the binary refuses a flag
unless `flags_with_config: true` in `src/modules/brocanth.rs`. See `docs/module-flags.md`.

## Examples

The examples use the memory store, so they run with no server. On
Postgres, pass a `postgres://` URL to `Queue::connect`, or a pool you
already have to `Queue::from_pool`.

### Define a job and enqueue it

`enqueue` takes the job value itself. A payload that the job cannot accept
is a compile error, not a decode failure on a worker.

```rust
use brocanth_jobs::{Job, JobContext, Queue};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct SendWelcome {
    user_id: i64,
}

impl Job for SendWelcome {
    const NAME: &'static str = "send_welcome";
    const QUEUE: &'static str = "mailers";
    const MAX_ATTEMPTS: u32 = 10;
    type Error = std::io::Error;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        Ok(())
    }

    // One queued copy per user at a time.
    fn unique_key(&self) -> Option<String> {
        Some(format!("welcome:{}", self.user_id))
    }
}

# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
let queue = Queue::memory(); // or Queue::connect("postgres://...").await?
queue.migrate().await?;

queue.enqueue(SendWelcome { user_id: 7 }).await?;
queue
    .enqueue(SendWelcome { user_id: 8 })
    .in_(std::time::Duration::from_secs(3600))
    .await?;

let stats = queue.stats().await?;
assert_eq!((stats.ready, stats.scheduled), (1, 1));
# Ok(())
# }
```

### Run a worker, then read the dead letter

A worker claims by queue and runs every job type it has registered. A
failing job is retried up to `MAX_ATTEMPTS`, then lands in the dead letter.
`requeue_dead` puts it back.

```rust
# use std::time::{Duration, Instant};
use brocanth_jobs::{Job, JobContext, Queue, Worker};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct ChargeCard {
    order: i64,
}

impl Job for ChargeCard {
    const NAME: &'static str = "charge_card";
    const MAX_ATTEMPTS: u32 = 1;
    type Error = String;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        Err(format!("the card for order {} was declined", self.order))
    }
}

# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let queue = Queue::memory();
queue.enqueue(ChargeCard { order: 42 }).await?;

let worker = Worker::builder(&queue)
    .queues(["default"])
    .concurrency(4)
    .poll_interval(Duration::from_millis(20))
    .register::<ChargeCard>()
    .start()?;

# let start = Instant::now();
while queue.stats().await?.dead == 0 {
#   assert!(start.elapsed() < Duration::from_secs(10), "the job did not die");
    tokio::time::sleep(Duration::from_millis(20)).await;
}
worker.shutdown(Duration::from_secs(5)).await?;

let dead = queue.dead_jobs(10, 0).await?;
assert_eq!(dead[0].kind, "charge_card");
assert!(queue.requeue_dead(&dead[0].id).await?.is_some());
# Ok(())
# }
```

### A recurring job

Five cron fields, UTC only. The application installs each schedule by
name. A second `install` with the same expression leaves the next run
alone, so a restart does not push every schedule back by a period.
`Misfire` decides what happens when no worker was up across a window.

```rust
# use brocanth_jobs::{Job, JobContext, Misfire, Queue};
# use serde::{Deserialize, Serialize};
#[derive(Serialize, Deserialize)]
struct NightlyBilling;

impl Job for NightlyBilling {
    const NAME: &'static str = "nightly_billing";
    type Error = std::io::Error;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        Ok(())
    }
}

# #[tokio::main]
# async fn main() -> Result<(), Box<dyn std::error::Error>> {
# let queue = Queue::memory();
queue
    .recurring("nightly-billing", "0 3 * * *", NightlyBilling)
    // Must run once a day even if the worker was down at 03:00.
    .misfire(Misfire::FireOnce)
    .install()
    .await?;
# Ok(())
# }
```

### Enqueue in a transaction

On Postgres, `in_tx` writes the job in the caller's transaction, to the
engine's outbox. The job commits with the data it refers to, or not at
all. After the commit, the relay that every worker runs moves it to the
queue.

```rust,no_run
# use brocanth_jobs::{Job, JobContext, Queue};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)] struct ChargeCard { order: i64 }
# impl Job for ChargeCard {
#     const NAME: &'static str = "charge_card";
#     type Error = std::io::Error;
#     async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
# }
# async fn demo() -> Result<(), Box<dyn std::error::Error>> {
let queue = Queue::connect("postgres://localhost/app").await?;
let mut tx = queue.pg_pool().expect("a Postgres queue").begin().await?;
// ... insert the order with `&mut *tx` ...
queue.enqueue(ChargeCard { order: 7 }).in_tx(&mut tx).await?;
tx.commit().await?;
# Ok(())
# }
```

### Register the plugin and the admin tab

`JobsPlugin` takes a queue that is already open, because a pool is made
asynchronously and `Plugin::new` is not. The application opens it in
`main` and keeps the worker handle for the shutdown. This example opens a
Postgres URL, so it compiles but does not run (`no_run`).

```rust,no_run
# use std::sync::Arc;
# use brocanth_jobs::{Job, JobContext, JobRegistry, JobsConfig, JobsPlugin, Queue};
# use serde::{Deserialize, Serialize};
# #[derive(Serialize, Deserialize)] struct SendWelcome { user_id: i64 }
# impl Job for SendWelcome {
#     const NAME: &'static str = "send_welcome";
#     type Error = std::io::Error;
#     async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
# }
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;

# async fn demo() -> Result<(), Box<dyn std::error::Error>> {
let queue = Queue::connect("postgres://localhost/app").await?;

let mut jobs = JobRegistry::new();
jobs.register::<SendWelcome>();

let app = AppBuilder::new()
    .plugin(JobsPlugin::new(
        JobsConfig::new(queue.clone()).with_workers(jobs).migrating(),
    ))
    .build()?;

// With the `admin` feature: the queue's tab in the backoffice.
let panel = brocanth_jobs::admin::JobsPanel::new(Arc::new(queue.clone()));
let admin = brocanth_admin::AdminConfig::new().panel(panel);
# let _ = (app, admin);
# Ok(())
# }
```

## Testing

```sh
cargo test -p brocanth-jobs
```

The behaviour suite (`tests/queue.rs`, `tests/recurring.rs`,
`tests/panel.rs`) runs once per store: `memory::<test>` always, and
`postgres::<test>` when a server is set. `tests/postgres.rs` holds what
only Postgres has: the migration race, the move from the first queue's
tables, the transactional enqueue and `LISTEN/NOTIFY`.

```sh
BROCANTH_JOBS_TEST_POSTGRES=postgres://localhost/brocanth_jobs_test \
    cargo test -p brocanth-jobs
```

| Variable | Effect |
|---|---|
| `BROCANTH_JOBS_TEST_POSTGRES` | The database for the Postgres tests. Read first. |
| `BROCANTH_TEST_DATABASE_URL` | The workspace-wide variable. Used when the first is not set. |

Without either, each Postgres test prints `!! SKIPPED <test>` to stderr,
past the output capture, and passes.

**Isolation.** Each Postgres test creates a schema of its own and works in
it through `search_path`. The tests run in parallel and truncate nothing.

**Testing an application.** Give the application `Queue::memory()`, or a
`Queue` on a test database. For a route that enqueues, send the request
with `brocanth-test`'s `TestClient`, then assert on `queue.ready(..)`,
`queue.scheduled(..)` or `queue.stats()`. To test the job itself, run a
`Worker` with the job registered and wait for the queue to empty, as the
second example does.

## Links

- `docs/modules.md`: how `jobs` reaches a project, and why `ShutdownPhase`
  has two phases.
- `docs/module-flags.md`: how the `[jobs]` settings become flags and
  variables.
- `docs/backoffice.md`: the admin seam that `JobsPanel` plugs into.
- `docs/observability.md`: the job span, log line and counter, and the
  readiness check.
- The crate docs (below, and in `src/`): each design argument sits in the
  module it justifies: `worker` (the runtime question, and what the
  engine does), `queue` (the store), `registry`, `cron`, `Misfire`,
  `backoff`.
- `crates/brocanth-queue/docs/DECISIONS.md` Part 6: one job system.
