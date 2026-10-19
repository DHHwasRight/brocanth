//! Background jobs for brocanth: a database-backed queue with retries, a dead
//! letter, scheduling, recurring work and a worker pool.
//!
//! This is a plugin. The core is a router handing a request and a context to a
//! controller, it has exactly three dependencies, and a test in it fails the
//! build if a fourth kind arrives — no SQL driver, no async runtime, none of
//! what is in here. The arrow points from this crate to the core and never
//! back, so an application that does not name [`JobsPlugin`] in its `main`
//! links no Tokio, no `sqlx` and no queue.
//!
//! # Two failures this was built to avoid
//!
//! **Loco.rs shipped a 1.0 job queue with no retries.** Its own CLI
//! documentation says *"There is no automatic retry or backoff in any queue
//! driver, so without `retry` a failed job is terminal."* Here, retries with
//! exponential backoff and jitter are a `const` on the [`Job`] trait with a
//! working default, and a job that runs out of attempts lands in a dead-letter
//! table holding its payload, its error, its attempt count and its backtrace
//! where one was captured. It is inspectable with [`Queue::dead_jobs`] and
//! re-enqueueable with [`Queue::requeue_dead`]. A failed job is never terminal
//! and never silently gone.
//!
//! **Rails configures Solid Queue in `production.rb` only**, so development and
//! test run a different queue and a job that works locally fails in production.
//! Here the backend is a **Cargo feature**, chosen once in `Cargo.toml`, and a
//! Cargo feature cannot vary by environment. There is no in-memory mode, on
//! purpose: an in-memory queue is a second implementation, and a second
//! implementation is exactly the trap.
//!
//! # Which backend
//!
//! **Postgres is the default**, because it is brocanth's base: the same
//! database holds the models, the cache and the blobs, and `FOR UPDATE SKIP
//! LOCKED` — the one mechanism this queue is built on — is a Postgres
//! mechanism. **SQLite is supported** for a single-machine deployment that
//! wants no server; it claims work under a whole-file write lock instead, so
//! no job is ever claimed twice but claims serialise rather than overlap. The
//! `store` module carries the two statements side by side.
//!
//! The crate's own tests run on SQLite with no server, through a
//! dev-dependency on itself with the `sqlite` feature on, and on Postgres when
//! `BROCANTH_JOBS_TEST_POSTGRES` or `BROCANTH_TEST_DATABASE_URL` names one.
//!
//! # The shape of it
//!
//! ```no_run
//! use brocanth_jobs::{Job, JobContext, Queue, Worker};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct SendWelcome {
//!     user_id: i64,
//! }
//!
//! impl Job for SendWelcome {
//!     const NAME: &'static str = "send_welcome";
//!     const QUEUE: &'static str = "mailers";
//!     const MAX_ATTEMPTS: u32 = 10;
//!     type Error = std::io::Error;
//!
//!     async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
//!         // send the mail
//!         Ok(())
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let queue = Queue::connect("postgres://localhost/app").await?;
//!     queue.migrate().await?;
//!
//!     queue.enqueue(SendWelcome { user_id: 7 }).await?;
//!
//!     let worker = Worker::builder(&queue)
//!         .queues(["mailers"])
//!         .concurrency(8)
//!         .register::<SendWelcome>()
//!         .start()?;
//!
//!     worker.shutdown(std::time::Duration::from_secs(30)).await?;
//!     Ok(())
//! }
//! ```
//!
//! Enqueuing is type-checked: `enqueue` takes the job value itself, so there is
//! no untyped payload argument to get wrong, and a payload a job cannot accept
//! is a struct-literal error rather than a decode failure on a worker at three
//! in the morning.
//!
//! # Where the arguments are
//!
//! Each design decision is argued where it lives, because an argument far from
//! the code it justifies rots:
//!
//! * [`store`] — `FOR UPDATE SKIP LOCKED` on Postgres, and what SQLite does
//!   instead, which is different in mechanism and identical in guarantee.
//! * [`worker`] — **the runtime question**: which runtime runs the workers, is
//!   it the one serving requests, and what that costs.
//! * [`listen`] — **being told there is work instead of asking twice a
//!   second.** `pg_notify` rides inside the insert, so it is delivered when
//!   the insert commits and never before the row is visible. It can only
//!   ever shorten a sleep, so losing every notification costs latency and
//!   no job. SQLite has no equivalent and polls.
//! * [`clock`] — why time is a `BIGINT` and a parameter.
//! * [`backoff`] — exponential, capped, jittered, and why the jitter matters.
//! * [`cron`] — five fields, UTC only, and why there is no time zone.
//! * [`events`] — observability without choosing anybody's logging framework.
//! * [`Misfire`] — **what a recurring job does when a worker was down across
//!   its window.** Short version: it skips, it counts what it skipped, and
//!   [`Misfire::FireOnce`] is there for the jobs that must not.
//! * [`plugin`] — the five-hook lifecycle, and why this plugin autowires
//!   nothing.
//!
//! # On `apalis`
//!
//! `apalis` 1.0.0 was published on 2026-10-12, with `apalis-postgres`,
//! `apalis-sqlite` and `apalis-redis` 1.0.0 the same morning — the first stable
//! anchor the Rust job-queue story has had. It was evaluated seriously and not
//! used. `apalis-postgres` gets the hard part right in a way worth copying, and
//! this crate copies it: `FOR UPDATE SKIP LOCKED`, orphan reclaim keyed on a
//! worker heartbeat, and a unique index for idempotency. The reasons not to
//! wrap it are specific:
//!
//! 1. **Retry backoff is in-process.** `BackoffRetryPolicy` is a
//!    `tower::retry::Policy`, so the delay is slept inside the worker while it
//!    holds a concurrency slot and the row's lock. The database-level path does
//!    not back off at all — `apalis-postgres`'s `handle_result` never touches
//!    `run_at`, so a failed task is re-fetched on the very next poll. For
//!    delays measured in minutes and hours that model is wrong in kind, not in
//!    degree.
//! 2. **There is no dead-letter table.** An exhausted task becomes
//!    `status = 'Killed'` in the same `jobs` table, and `vacuum.sql` deletes
//!    `Killed` rows. The record of the failure and the thing that deletes
//!    failure records are the same table.
//! 3. **It chooses your log layer.** `tracing` is in `apalis`'s default feature
//!    set. PLAN.md §8 does not allow a framework to make that choice for an
//!    application, and turning it off leaves a tower-shaped framework with its
//!    tower layers off.
//! 4. **Its model is tower, all the way down.** A job is a
//!    `Service<Request<T, Ctx>>` and a worker is a `ServiceBuilder` stack.
//!    PLAN.md §6.4 is explicit that this project defines its own middleware
//!    trait and adapts tower behind it rather than exposing `Service` and
//!    `Layer`, because unreadable trait-bound errors are the loss it is
//!    planning for. Wrapping apalis means either re-exposing those bounds or
//!    building a facade over every layer — most of the work of writing the
//!    queue, with none of the control over its storage semantics.
//!
//! Had points 1 and 2 gone the other way, wrapping would have beaten rewriting
//! and this crate would be a thin adapter. They did not.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(not(any(feature = "postgres", feature = "sqlite")))]
compile_error!(
    "brocanth-jobs needs a database to be a queue. Enable the `postgres` \
     feature (the default) or the `sqlite` feature. There is no in-memory \
     backend on purpose: a queue that is a different implementation in \
     development is the trap this crate exists to avoid."
);

pub mod backoff;
pub mod clock;
pub mod cron;
pub mod events;
pub mod job;
pub mod listen;
pub mod plugin;
pub mod queue;
pub mod registry;
#[cfg(feature = "scaffold")]
pub mod scaffold;
pub mod worker;

mod error;
mod panic_hook;
mod store;

pub use backoff::{Backoff, Prng};
pub use clock::{Clock, ManualClock, SystemClock};
pub use cron::Cron;
pub use error::{Error, Result};
pub use events::{EventFn, JobEvent, JobEvents, Silent};
pub use job::{FailureKind, Job, JobContext, JobFailure, JobId};
pub use panic_hook::capture_backtraces;
pub use plugin::{JobsConfig, JobsPlugin};
pub use queue::{Enqueue, JobState, Misfire, Queue, Reclaimed, Recurring};
pub use registry::JobRegistry;
pub use store::{Backend, DeadJob, Enqueued, QueueDepth, RunningJob, Stats, WaitingJob};
pub use worker::{ShutdownReport, Worker, WorkerBuilder, WorkerConfig, WorkerHandle};

/// Everything an application that enqueues and runs jobs needs, in one `use`.
pub mod prelude {
    pub use crate::{
        Backoff, Job, JobContext, JobEvent, JobEvents, JobId, JobRegistry, JobsConfig, JobsPlugin,
        Misfire, Queue, Worker,
    };
}
