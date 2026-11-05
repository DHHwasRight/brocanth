# brocanth-jobs

Background jobs for brocanth: a database-backed queue with retries, a dead
letter, delayed and recurring jobs, and a worker pool. A job that fails is
retried with exponential backoff and jitter. A job that runs out of
attempts goes to a dead-letter table with its payload and its error, and an
operator can requeue it. The backend is a Cargo feature: Postgres by
default, SQLite for one machine with no server. It is a plugin, so an
application that does not register `JobsPlugin` links no Tokio, no `sqlx`
and no queue.

Use it for work that must not run inside the request that asked for it:
mail, reports, a nightly sweep. This is the queue that `brocanth create
--with jobs` installs. Do not use it when a job must be able to move to SQS
or another managed queue later. That is `brocanth-queue`, a separate
contract with its own backends. The two share one admin panel through
`brocanth_queue::Inspect`.

## Getting started

```toml
[dependencies]
brocanth-jobs = { git = "https://github.com/DHHwasRight/brocanth.git" }
# One machine, no Postgres:
# brocanth-jobs = { git = "...", default-features = false, features = ["sqlite"] }
```

Inside this workspace: `brocanth-jobs.workspace = true`.

| Feature | Default | What it adds |
|---|---|---|
| `runtime` | on | The queue itself: everything but `scaffold`. Each backend turns it on. |
| `postgres` | on | The Postgres store. The claim is `FOR UPDATE SKIP LOCKED`. |
| `sqlite` | off | The SQLite store. Claims serialise under the file's write lock. |
| `admin` | off | `admin::JobsPanel`, the queue's tab in `brocanth-admin`. |
| `scaffold` | off | The `jobs` module for `brocanth create`. Only the CLI turns it on. |

Turn off the default features when you choose `sqlite`. With both on, both
drivers compile. With neither, the build fails with a message that says
which feature to turn on.

### As a module

```sh
brocanth create my-app --with jobs
# or, in an existing project:
brocanth add jobs
```

The module writes `src/plugins/jobs/mod.rs`: a plugin that opens the queue
from a URL, starts a worker in the process, and puts the `Queue` on the
request context. It declares these settings in the `[jobs]` section:

| Key | Default | Flag | Environment |
|---|---|---|---|
| `url` | `postgres://localhost/development` | none (secret) | `$<APP>_JOBS__URL` |
| `migrate` | `true` | `--jobs.migrate` | `$<APP>_JOBS__MIGRATE` |
| `worker` | `true` | `--jobs.worker` | `$<APP>_JOBS__WORKER` |
| `queues` | `["default"]` | none (a list) | by index, with `config` only |
| `concurrency` | `5` | `--jobs.concurrency` | `$<APP>_JOBS__CONCURRENCY` |

`url` has no flag because a URL can carry a password, and every user on the
host can read a process's arguments. `migrate` is for development. In
production, put the statements from `Queue::schema_sql` in a migration
somebody reviewed. With the `config` module, the binary refuses a flag
unless `WITH_CONFIG = true` in `src/flags.rs`. See `docs/module-flags.md`.

## Examples

The examples use the SQLite store in a temporary directory, so they run
with no server. On Postgres, pass a `postgres://` URL to `Queue::connect`.

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
# let dir = tempfile::tempdir()?;
# let url = format!("sqlite://{}", dir.path().join("jobs.db").display());
let queue = Queue::connect(&url).await?;
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
# let dir = tempfile::tempdir()?;
# let url = format!("sqlite://{}", dir.path().join("jobs.db").display());
# let queue = Queue::connect(&url).await?;
# queue.migrate().await?;
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
assert_eq!(dead[0].job_name, "charge_card");
assert!(queue.requeue_dead(dead[0].id).await?.is_some());
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
# let dir = tempfile::tempdir()?;
# let url = format!("sqlite://{}", dir.path().join("jobs.db").display());
# let queue = Queue::connect(&url).await?;
# queue.migrate().await?;
queue
    .recurring("nightly-billing", "0 3 * * *", NightlyBilling)
    // Must run once a day even if the worker was down at 03:00.
    .misfire(Misfire::FireOnce)
    .install()
    .await?;
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

The suite runs on SQLite with no server. The crate has a dev-dependency on
itself with `sqlite` and `admin` on, so a test build always has the SQLite
store, and a library build keeps only the driver it asked for. Each test
gets its own SQLite file in its own temporary directory.

The default backend is Postgres, so the SQLite suite does not test the
default code path. `tests/postgres.rs` does, when a server is set:

```sh
BROCANTH_JOBS_TEST_POSTGRES=postgres://localhost/brocanth_jobs_test \
    cargo test -p brocanth-jobs --test postgres
```

| Variable | Effect |
|---|---|
| `BROCANTH_JOBS_TEST_POSTGRES` | The database for `tests/postgres.rs`. Read first. |
| `BROCANTH_TEST_DATABASE_URL` | The workspace-wide variable. Used when the first is not set. It is the only variable the History test in `tests/panel.rs` reads. |

Without either, each Postgres test prints `!! SKIPPED <test>` to stderr,
past the output capture, and passes.

**Isolation.** The tests in `tests/postgres.rs` share the queue tables in
the given database. They never truncate. Each test works in its own queue
name with its own token, as two applications on one database would. Two
tests make their own schema and drop it after: the migration race in
`tests/postgres.rs` and the History test in `tests/panel.rs`. Run one
`cargo test` process per database at a time: `tick_schedules`,
`reclaim_orphans` and `stats` act on the whole queue.

**Testing an application.** Open a `Queue` on a test database (a SQLite
file in a temporary directory, or a Postgres database from the variables
above), and give it to the application. For a route that enqueues, send
the request with `brocanth-test`'s `TestClient`, then assert on
`queue.ready(..)`, `queue.scheduled(..)` or `queue.stats()`. To test the
job itself, run a `Worker` with the job registered and wait for the queue
to empty, as the second example does.

## Links

- `docs/modules.md`: how `jobs` reaches a project, and why `ShutdownPhase`
  has two phases.
- `docs/module-flags.md`: how the `[jobs]` settings become flags and
  variables.
- `docs/backoffice.md`: the admin seam that `JobsPanel` plugs into.
- `docs/observability.md`: the job span, log line and counter, and the
  readiness check.
- The crate docs (below, and in `src/`): each design argument sits in the
  module it justifies: `store` (the claim on each backend), `worker` (the
  runtime question), `listen` (`pg_notify`), `cron`, `Misfire`, `backoff`.
