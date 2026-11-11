//! The queue: the handle an application enqueues through, and everything you
//! can do to work that is already in it.
//!
//! A [`Queue`] is cheap to clone — it is an `Arc` around the engine's
//! backend, its read side, the schedule store, a clock and an event sink —
//! so it goes in the context, in a handler, in a hook, anywhere.
//!
//! # The engine
//!
//! Every job lives in a `brocanth-queue` backend: Postgres by default
//! (`brocanth-queue-postgres`, the `queue_*` tables), or the memory backend
//! (`brocanth-queue-memory`) behind the `memory` feature, for tests and
//! development. This crate adds what the engine does not have: typed jobs,
//! recurring schedules, the event sink and the admin panel.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use brocanth_queue::{
    DeadId, DeadRow, DynBackend, DynInspect, JobRow, NewJob, Page, Pushed, UniquenessKind, View,
};

use crate::clock::{from_millis, to_millis, Clock, SystemClock};
use crate::cron::Cron;
use crate::error::{Error, Result};
use crate::events::{JobEvent, JobEvents, Silent};
use crate::job::{Job, JobId};
use crate::schedule::{ScheduleRow, Schedules};

/// What a recurring schedule does about occurrences it slept through.
///
/// A worker that was down from 22:00 to 06:00 wakes up owing eight runs of an
/// hourly job. There are two honest answers and this crate makes you pick one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Misfire {
    /// **Drop them. The default.**
    ///
    /// The schedule jumps to the next occurrence in the future and the missed
    /// ones never run. They are counted, so `missed` on the schedule row tells
    /// you it happened.
    ///
    /// This is the default because catching up is usually the more expensive
    /// mistake. Eight hourly reports firing in the same second of a recovery is
    /// a thundering herd arriving exactly when the system is least able to take
    /// it, and for most recurring work — "send the digest", "expire old
    /// sessions", "refresh the cache" — the run you skipped is made redundant
    /// by the run you are about to do anyway. Seven identical digests is a
    /// worse outcome than one late digest.
    #[default]
    Skip,

    /// **Fire the most recent one, drop the rest.**
    ///
    /// Still exactly one job, but it runs even though its window has passed.
    /// Choose this when a run doing nothing is itself the failure — a nightly
    /// billing sweep that must happen once per day whether or not the worker
    /// was up at three in the morning.
    ///
    /// The job's payload is the one that was registered, so it cannot tell
    /// which occurrence it is firing for. **Work that must process a specific
    /// window should not be a recurring job at all**: have the recurring job
    /// enqueue dated jobs, one per window, with a unique key per window, and
    /// then the catch-up question answers itself and is idempotent besides.
    FireOnce,
}

impl Misfire {
    fn as_str(self) -> &'static str {
        match self {
            Misfire::Skip => "skip",
            Misfire::FireOnce => "fire_once",
        }
    }

    fn from_str(text: &str) -> Self {
        match text {
            "fire_once" => Misfire::FireOnce,
            _ => Misfire::Skip,
        }
    }
}

/// How many occurrences a single tick will walk before giving up.
///
/// A `* * * * *` schedule that has not been ticked for a year is 525,600
/// occurrences, and walking all of them to decide to skip them is pointless.
const MAX_OCCURRENCES_PER_TICK: u32 = 100_000;

/// How long the unique key of one schedule occurrence is held.
///
/// Long enough that two workers ticking the same occurrence minutes apart
/// still enqueue it once, even when the first copy already ran. Every
/// occurrence has its own key, so the length never delays the next one.
const OCCURRENCE_KEY_HELD: Duration = Duration::from_secs(24 * 60 * 60);

// ---------------------------------------------------------------------------
// Backend
// ---------------------------------------------------------------------------

/// The store a queue runs on. Each one is a Cargo feature.
#[non_exhaustive]
pub enum Backend {
    /// Postgres, on this pool: the engine's `queue_*` tables and this
    /// crate's `brocanth_job_schedules`.
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    /// The process's memory. **For tests and development only**: every job
    /// is gone when the process stops, and two processes do not share one.
    /// `brocanth check --deploy` reports a project that ships it.
    #[cfg(feature = "memory")]
    Memory,
}

#[cfg(feature = "postgres")]
impl From<sqlx::PgPool> for Backend {
    fn from(pool: sqlx::PgPool) -> Self {
        Backend::Postgres(pool)
    }
}

/// What the queue holds besides the engine's two handles, per backend.
enum Store {
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    #[cfg(feature = "memory")]
    Memory,
}

struct Inner {
    backend: Arc<dyn DynBackend>,
    inspect: Arc<dyn DynInspect>,
    schedules: Schedules,
    store: Store,
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

/// The queue [`Queue::install`] recorded.
static INSTALLED: std::sync::OnceLock<Queue> = std::sync::OnceLock::new();

/// The queue.
///
/// ```no_run
/// # use brocanth_jobs::{Job, JobContext, Queue};
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Serialize, Deserialize)] struct SendWelcome { user_id: i64 }
/// # impl Job for SendWelcome {
/// #     const NAME: &'static str = "send_welcome";
/// #     type Error = std::io::Error;
/// #     async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
/// # }
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let queue = Queue::connect("postgres://localhost/app").await?;
/// queue.migrate().await?;
///
/// queue.enqueue(SendWelcome { user_id: 7 }).await?;
/// queue
///     .enqueue(SendWelcome { user_id: 8 })
///     .in_(std::time::Duration::from_secs(300))
///     .unique("welcome:8")
///     .await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Queue {
    inner: Arc<Inner>,
    clock: Arc<dyn Clock>,
    events: Arc<dyn JobEvents>,
}

impl Queue {
    /// Opens a queue from a database URL.
    ///
    /// `postgres://…` or `postgresql://…` needs the `postgres` feature, the
    /// default. A URL whose backend was not compiled in is an error that says
    /// which Cargo feature to turn on, because that is where the fix is.
    pub async fn connect(url: &str) -> Result<Self> {
        let queue = Self::connect_lazy(url)?;
        // One connection now, so a wrong URL or password fails here and not
        // on the first enqueue.
        queue.ping().await?;
        Ok(queue)
    }

    /// Builds a queue from a database URL and opens no connection. The first
    /// statement opens one.
    ///
    /// For a synchronous stage, which must not wait for I/O: a plugin calls
    /// this in `before_start`, and runs [`Queue::migrate`] and its first
    /// check later, from async code. The pool spawns its upkeep task, so call
    /// it inside a Tokio runtime.
    pub fn connect_lazy(url: &str) -> Result<Self> {
        let scheme = url.split(':').next().unwrap_or("");
        match scheme {
            #[cfg(feature = "postgres")]
            "postgres" | "postgresql" => Ok(Self::from_pool(
                sqlx::postgres::PgPoolOptions::new()
                    .connect_lazy(url)
                    .map_err(|e| {
                        Error::Backend(format!(
                            "the Postgres URL does not parse: {e}\n\
                             help: write `postgres://user:password@host:5432/database`"
                        ))
                    })?,
            )),
            #[cfg(not(feature = "postgres"))]
            "postgres" | "postgresql" => Err(Error::Backend(
                "this build of brocanth-jobs has no Postgres driver\n\
                 help: add `features = [\"postgres\"]` to its dependency in Cargo.toml"
                    .to_string(),
            )),
            "sqlite" => Err(Error::Backend(
                "brocanth-jobs runs on brocanth-queue, which has no SQLite backend yet\n\
                 help: use a `postgres://` URL, or `Queue::memory()` for tests and development"
                    .to_string(),
            )),
            other => Err(Error::Backend(format!(
                "`{other}` is not a database this queue knows; the URL must start \
                 with `postgres://`\n\
                 help: for tests and development, `Queue::memory()` needs no URL"
            ))),
        }
    }

    /// Builds a queue on a store the application already has.
    ///
    /// This is the path for an application that also uses `brocanth-db` or
    /// `brocanth-model`: hand it the same `PgPool`, and the queue and the
    /// models share one set of connections rather than opening a second pool
    /// against the same database.
    pub fn from_pool(backend: impl Into<Backend>) -> Self {
        let inner = match backend.into() {
            #[cfg(feature = "postgres")]
            Backend::Postgres(pool) => {
                let pg = Arc::new(brocanth_queue_postgres::PgQueue::bulk(pool.clone()));
                Inner {
                    backend: pg.clone(),
                    inspect: pg,
                    schedules: Schedules::Postgres(pool.clone()),
                    store: Store::Postgres(pool),
                }
            }
            #[cfg(feature = "memory")]
            Backend::Memory => {
                let memory = Arc::new(brocanth_queue_memory::Memory::new());
                Inner {
                    backend: memory.clone(),
                    inspect: memory,
                    schedules: Schedules::Memory(Default::default()),
                    store: Store::Memory,
                }
            }
        };
        Self {
            inner: Arc::new(inner),
            clock: Arc::new(SystemClock),
            events: Arc::new(Silent),
        }
    }

    /// A queue in this process's memory. **For tests and development
    /// only**: see [`Backend::Memory`].
    #[cfg(feature = "memory")]
    pub fn memory() -> Self {
        Self::from_pool(Backend::Memory)
    }

    /// Replaces the clock. See [`crate::clock`] for why there is one.
    ///
    /// The clock drives the recurring schedules. A job's own delay is the
    /// backend's business, and Postgres measures it with the server's clock.
    #[must_use]
    pub fn with_clock(mut self, clock: impl Clock) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Installs an event sink. The default drops events on the floor.
    #[must_use]
    pub fn with_events(mut self, events: impl JobEvents) -> Self {
        self.events = Arc::new(events);
        self
    }

    /// Which backend this queue is on: `"postgres"` or `"memory"`.
    pub fn backend(&self) -> &'static str {
        self.inner.backend.name()
    }

    /// The current time, from this queue's clock.
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.now()
    }

    /// One cheap read, to prove the store answers. A readiness check calls
    /// it.
    pub async fn ping(&self) -> Result<()> {
        self.inner.inspect.stats().await?;
        Ok(())
    }

    /// Creates the tables if they are not there, and moves the rows of the
    /// first queue's tables into the engine's.
    ///
    /// Safe to call on every boot, from several processes at once. See
    /// [`Queue::schema_sql`] for the statements an application keeps in
    /// `migrations/` instead. The memory backend has nothing to create.
    pub async fn migrate(&self) -> Result<()> {
        match &self.inner.store {
            #[cfg(feature = "postgres")]
            Store::Postgres(pool) => {
                brocanth_queue_postgres::migrate(pool).await?;
                self.inner.schedules.migrate().await?;
                crate::legacy::move_rows(pool).await?;
                Ok(())
            }
            #[cfg(feature = "memory")]
            Store::Memory => Ok(()),
        }
    }

    /// The DDL [`Queue::migrate`] runs, as statements.
    ///
    /// For an application whose rule is that the schema lives in files a human
    /// reviewed — which is §4.3 of the plan, and the right rule — paste these
    /// into a migration and never call [`Queue::migrate`]. The engine's
    /// archive needs a partition per day: a worker creates them as it
    /// sweeps.
    pub fn schema_sql(&self) -> Vec<&'static str> {
        match &self.inner.store {
            #[cfg(feature = "postgres")]
            Store::Postgres(_) => brocanth_queue_postgres::SCHEMA
                .split(";\n")
                .map(str::trim)
                .filter(|s| !s.is_empty() && !s.lines().all(|l| l.trim_start().starts_with("--")))
                .chain(std::iter::once(crate::schedule::SCHEMA))
                .collect(),
            #[cfg(feature = "memory")]
            Store::Memory => Vec::new(),
        }
    }

    /// Makes this queue the one [`Queue::installed`] answers. The first
    /// call wins; a later one does nothing.
    ///
    /// For a generated admin, which mounts its panels before any plugin has
    /// opened a queue: `JobsPanel::installed()` asks here when it renders.
    /// A process builds one application, so "the first" is "the one". A
    /// test that builds several hands the panel its queue instead.
    pub fn install(&self) {
        let _ = INSTALLED.set(self.clone());
    }

    /// The queue [`Queue::install`] recorded, if one did.
    pub fn installed() -> Option<Queue> {
        INSTALLED.get().cloned()
    }

    /// The engine's backend, for a worker or for a test that drives the
    /// engine directly.
    pub fn engine(&self) -> &Arc<dyn DynBackend> {
        &self.inner.backend
    }

    /// The engine's read side, which the admin panel renders.
    pub fn reader(&self) -> &Arc<dyn DynInspect> {
        &self.inner.inspect
    }

    // -- enqueue ------------------------------------------------------------

    /// Puts a job on its queue.
    ///
    /// Returns a builder that is also a future, so the simple case is
    /// `queue.enqueue(job).await?` and the elaborate one chains `.at()`,
    /// `.in_()`, `.queue()` or `.unique()` first, or ends in
    /// `.in_tx(&mut tx)` to commit the job with the caller's data.
    ///
    /// # The payload is the job
    ///
    /// There is no separate payload argument, so there is nothing to get out of
    /// step with the job that will receive it. A value that is not a [`Job`]
    /// cannot be enqueued:
    ///
    /// ```compile_fail
    /// # async fn example(queue: &brocanth_jobs::Queue) {
    /// // `&str` is not a `Job`, and this is where you find out.
    /// let _ = queue.enqueue("send_welcome").await;
    /// # }
    /// ```
    ///
    /// and a payload the job cannot accept is a struct-literal error at the
    /// enqueue site rather than a decode failure on a worker at three in the
    /// morning:
    ///
    /// ```compile_fail
    /// use brocanth_jobs::{Job, JobContext};
    /// use serde::{Deserialize, Serialize};
    ///
    /// #[derive(Serialize, Deserialize)]
    /// struct SendWelcome { user_id: i64 }
    ///
    /// impl Job for SendWelcome {
    ///     const NAME: &'static str = "send_welcome";
    ///     type Error = std::io::Error;
    ///     async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
    /// }
    ///
    /// # async fn example(queue: &brocanth_jobs::Queue) {
    /// // `SendWelcome` has no `email` field.
    /// let _ = queue.enqueue(SendWelcome { email: "a@b.c".to_string() }).await;
    /// # }
    /// ```
    pub fn enqueue<J: Job>(&self, job: J) -> Enqueue<'_, J> {
        Enqueue {
            queue: self,
            job,
            override_queue: None,
            override_unique: None,
            run_at: None,
        }
    }

    // -- inspection ---------------------------------------------------------

    /// Counts by state, per queue and in total, plus the age of the oldest
    /// runnable job.
    pub async fn stats(&self) -> Result<Stats> {
        let total = self.inner.inspect.stats().await?;
        let depths = self.inner.inspect.depths().await?;
        Ok(Stats {
            ready: total.ready,
            retrying: total.retrying,
            scheduled: total.scheduled,
            running: total.running,
            dead: total.dead,
            oldest_ready_age: total.oldest_ready,
            queues: depths
                .into_iter()
                .map(|d| QueueDepth {
                    queue: d.queue,
                    ready: d.ready,
                    retrying: d.retrying,
                    scheduled: d.scheduled,
                    running: d.running,
                    oldest_ready_age: d.oldest_ready,
                })
                .collect(),
        })
    }

    /// One job's state, attempt count and next run. `None` once it has
    /// finished and is gone.
    pub async fn inspect(&self, id: &JobId) -> Result<Option<JobState>> {
        let Some(row) = self.inner.inspect.job(id).await? else {
            return Ok(None);
        };
        let running = row.claimed_by.is_some() && row.available_at > SystemTime::now();
        Ok(Some(JobState {
            state: if running { "running" } else { "pending" }.to_string(),
            attempts: row.attempt,
            run_at: row.available_at.into(),
        }))
    }

    /// One waiting or running job, whole.
    pub async fn job(&self, id: &JobId) -> Result<Option<JobRow>> {
        Ok(self.inner.inspect.job(id).await?)
    }

    /// Jobs that could run right now, oldest first.
    pub async fn ready(&self, limit: u32, offset: u32) -> Result<Vec<JobRow>> {
        self.list(View::Ready, limit, offset).await
    }

    /// What every worker is holding right now, oldest claim first.
    pub async fn busy(&self, limit: u32, offset: u32) -> Result<Vec<JobRow>> {
        self.list(View::Running, limit, offset).await
    }

    /// Jobs that ran, failed, and are waiting for their backoff to pass.
    ///
    /// Separate from [`Queue::scheduled`] by `attempts`, and that is the
    /// only thing separating them: a job waiting for its first run has
    /// none, a job waiting after a failure has some.
    pub async fn retries(&self, limit: u32, offset: u32) -> Result<Vec<JobRow>> {
        self.list(View::Retrying, limit, offset).await
    }

    /// Jobs that have never run and are waiting for their time.
    pub async fn scheduled(&self, limit: u32, offset: u32) -> Result<Vec<JobRow>> {
        self.list(View::Scheduled, limit, offset).await
    }

    async fn list(&self, view: View, limit: u32, offset: u32) -> Result<Vec<JobRow>> {
        let page = Page {
            limit: limit.min(Page::MAX_LIMIT),
            offset: u64::from(offset),
        };
        Ok(self.inner.inspect.list(view, page).await?)
    }

    /// Makes a waiting job runnable now, whatever its next run said.
    ///
    /// The operator's override: a retry whose backoff is an hour away, or a
    /// job scheduled for tonight that should go out immediately. `false`
    /// means the job is not waiting: a worker holds it, or it is gone. The
    /// attempts it spent stay spent.
    pub async fn run_now(&self, id: &JobId) -> Result<bool> {
        Ok(self.inner.inspect.run_now(id).await?)
    }

    /// Throws a waiting job away without running it.
    ///
    /// No dead-letter row, on purpose. The dead letter records jobs that
    /// ran out of attempts, and a deliberate deletion is a different event;
    /// mixing them makes the record of failure useless. `false` means the
    /// job is not waiting.
    pub async fn discard(&self, id: &JobId) -> Result<bool> {
        Ok(self.inner.inspect.discard(id).await?)
    }

    // -- the dead letter ----------------------------------------------------

    /// The most recent dead jobs, newest first.
    pub async fn dead_jobs(&self, limit: u32, offset: u32) -> Result<Vec<DeadRow>> {
        let page = Page {
            limit: limit.min(Page::MAX_LIMIT),
            offset: u64::from(offset),
        };
        Ok(self.inner.inspect.dead(page).await?)
    }

    /// One dead job by its dead-letter id.
    pub async fn dead_job(&self, dead_id: &DeadId) -> Result<Option<DeadRow>> {
        Ok(self.inner.inspect.dead_one(dead_id).await?)
    }

    /// Puts a dead job back on its queue with its attempt count reset.
    ///
    /// `Ok(None)` means either that the dead row is gone, or that a live job
    /// already holds its unique key — in which case the dead row is left where
    /// it is rather than being deleted without being replayed.
    pub async fn requeue_dead(&self, dead_id: &DeadId) -> Result<Option<JobId>> {
        Ok(self.inner.inspect.requeue(dead_id).await?)
    }

    /// Deletes a dead job for good.
    pub async fn discard_dead(&self, dead_id: &DeadId) -> Result<bool> {
        Ok(self.inner.inspect.forget(dead_id).await?)
    }

    // -- recurring ----------------------------------------------------------

    /// Installs a recurring schedule, or updates one with the same name.
    ///
    /// The name is the identity: installing `"nightly-digest"` twice with
    /// different cron expressions replaces the expression and recomputes the
    /// next run; installing it twice with the *same* expression leaves the next
    /// run alone, so restarting the application does not push every schedule
    /// back by a period.
    ///
    /// ```no_run
    /// # use brocanth_jobs::{Job, JobContext, Misfire, Queue};
    /// # use serde::{Deserialize, Serialize};
    /// # #[derive(Serialize, Deserialize)] struct Digest;
    /// # impl Job for Digest {
    /// #     const NAME: &'static str = "digest";
    /// #     type Error = std::io::Error;
    /// #     async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
    /// # }
    /// # async fn example(queue: &Queue) -> Result<(), Box<dyn std::error::Error>> {
    /// queue
    ///     .recurring("nightly-digest", "0 3 * * *", Digest)
    ///     .misfire(Misfire::FireOnce)
    ///     .install()
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn recurring<J: Job>(&self, name: &str, cron: &str, job: J) -> Recurring<'_, J> {
        Recurring {
            queue: self,
            name: name.to_string(),
            cron: cron.to_string(),
            job,
            misfire: Misfire::Skip,
            grace: Duration::from_secs(60),
            override_queue: None,
            override_priority: None,
        }
    }

    /// Removes a schedule. Jobs it already enqueued are untouched.
    pub async fn remove_recurring(&self, name: &str) -> Result<bool> {
        Ok(self.inner.schedules.remove(name).await? > 0)
    }

    /// Enqueues whatever the recurring schedules owe, and advances them.
    ///
    /// A [`Worker`](crate::Worker) calls this on its maintenance tick. Running
    /// it from two workers at once is safe: every occurrence is enqueued with
    /// the unique key `"<schedule>@<occurrence epoch millis>"`, held for a
    /// day, and only one worker advances the schedule. See
    /// `schedule.rs` for the argument.
    pub async fn tick_schedules(&self) -> Result<u32> {
        let now = self.clock.now();
        let now_ms = to_millis(now);
        let due = self.inner.schedules.due(now_ms).await?;
        let mut fired = 0;

        for row in due {
            let cron = Cron::parse(&row.cron)?;
            let misfire = Misfire::from_str(&row.misfire);
            let grace_ms = row.grace_ms;

            // Walk forward from the stored next occurrence. Everything at or
            // before `now` has come due; the question is only which of them, if
            // any, still deserves to run.
            let mut occurrence_ms = row.next_run_at;
            let mut to_fire: Option<i64> = None;
            let mut missed: i64 = 0;
            let mut walked = 0u32;

            loop {
                if occurrence_ms > now_ms {
                    break;
                }
                let late_by = now_ms - occurrence_ms;
                if late_by <= grace_ms {
                    to_fire = Some(occurrence_ms);
                } else {
                    missed += 1;
                    if misfire == Misfire::FireOnce {
                        // Keep only the most recent: one job, not eight.
                        to_fire = Some(occurrence_ms);
                    }
                }
                match cron.next_after(from_millis(occurrence_ms)) {
                    Some(next) => occurrence_ms = to_millis(next),
                    // An expression that can no longer fire. Park it far enough
                    // ahead that it is not re-examined every tick; removing the
                    // row would lose the operator's intent.
                    None => {
                        occurrence_ms = now_ms + 86_400_000;
                        break;
                    }
                }
                walked += 1;
                if walked >= MAX_OCCURRENCES_PER_TICK {
                    // Too far behind to walk. Jump to the next future
                    // occurrence and record the rest as missed, which is what
                    // `Skip` would have concluded anyway.
                    if let Some(next) = cron.next_after(now) {
                        occurrence_ms = to_millis(next);
                    }
                    break;
                }
            }

            // Enqueue first, then advance. A process that dies between the
            // two leaves the schedule due, and the next tick enqueues the
            // same occurrence again, which its key folds into the first.
            let fired_at = match to_fire {
                Some(occurrence) => {
                    let new = NewJob {
                        kind: row.job_name.clone(),
                        queue: row.queue.clone(),
                        payload: row.payload.clone().into_bytes(),
                        run_at: None,
                        max_attempts: row.max_attempts.max(1) as u32,
                        unique_key: Some(format!("{}@{}", row.name, occurrence)),
                        uniqueness: UniquenessKind::Window(OCCURRENCE_KEY_HELD.as_millis() as u64),
                    };
                    if let Pushed::Enqueued(_) = self.push(new).await? {
                        fired += 1;
                        self.events.handle(JobEvent::ScheduleFired {
                            schedule: &row.name,
                            occurrence: from_millis(occurrence),
                            missed: missed.max(0) as u32,
                        });
                    }
                    Some(now_ms)
                }
                None => None,
            };

            self.inner
                .schedules
                .advance(&row.name, row.next_run_at, occurrence_ms, fired_at, missed)
                .await?;
        }

        Ok(fired)
    }

    // -- internals ----------------------------------------------------------

    /// One job into the backend.
    async fn push(&self, new: NewJob) -> Result<Pushed> {
        let mut out = self.inner.backend.push(vec![new]).await?;
        Ok(out.pop().unwrap_or(Pushed::Duplicate))
    }

    pub(crate) fn event_sink(&self) -> Arc<dyn JobEvents> {
        Arc::clone(&self.events)
    }

    /// The Postgres pool this queue runs on, to open the transaction that
    /// [`Enqueue::in_tx`] joins. `None` on the memory backend.
    #[cfg(feature = "postgres")]
    pub fn pg_pool(&self) -> Option<&sqlx::PgPool> {
        match &self.inner.store {
            Store::Postgres(pool) => Some(pool),
            #[cfg(feature = "memory")]
            Store::Memory => None,
        }
    }
}

impl std::fmt::Debug for Queue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Queue")
            .field("backend", &self.backend())
            .finish()
    }
}

/// So a [`Queue`] can go straight into the request context.
impl brocanth::plugin_context::ContextItem for Queue {}

/// One job's position, for an admin screen or a test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobState {
    /// `"pending"` or `"running"`.
    pub state: String,
    /// Attempts started so far.
    pub attempts: u32,
    /// When it next becomes runnable. For a running job, when its lease
    /// runs out.
    pub run_at: chrono::DateTime<chrono::Utc>,
}

/// How deep one queue is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueueDepth {
    /// The queue's name.
    pub queue: String,
    /// Jobs that could run now.
    pub ready: u64,
    /// Jobs waiting out a retry backoff.
    pub retrying: u64,
    /// Jobs waiting for their first run.
    pub scheduled: u64,
    /// Jobs a worker holds.
    pub running: u64,
    /// How long the oldest ready job has waited.
    pub oldest_ready_age: Option<Duration>,
}

/// The whole queue in numbers.
///
/// `oldest_ready_age` is the one that matters: a queue whose oldest runnable
/// job keeps getting older is a queue whose workers are not keeping up, and no
/// count of anything tells you that.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Jobs that could run now.
    pub ready: u64,
    /// Jobs waiting out a retry backoff.
    pub retrying: u64,
    /// Jobs waiting for their first run.
    pub scheduled: u64,
    /// Jobs a worker holds.
    pub running: u64,
    /// Jobs in the dead letter.
    pub dead: u64,
    /// How long the oldest ready job has waited.
    pub oldest_ready_age: Option<Duration>,
    /// The same counts, per queue.
    pub queues: Vec<QueueDepth>,
}

impl Stats {
    /// Every job that exists and is not dead.
    pub fn total(&self) -> u64 {
        self.ready + self.retrying + self.scheduled + self.running
    }
}

/// What an enqueue did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Enqueued {
    /// A new job, with its id.
    Inserted(JobId),
    /// A live job already held the unique key, so nothing was written.
    Duplicate,
}

impl Enqueued {
    /// The new job's id. `None` for a duplicate.
    pub fn id(&self) -> Option<&JobId> {
        match self {
            Enqueued::Inserted(id) => Some(id),
            Enqueued::Duplicate => None,
        }
    }

    /// Whether the enqueue folded into a job that was already there.
    pub fn is_duplicate(&self) -> bool {
        matches!(self, Enqueued::Duplicate)
    }
}

// ---------------------------------------------------------------------------
// The enqueue builder
// ---------------------------------------------------------------------------

/// A job on its way into the queue. Awaiting it is what sends it.
#[must_use = "an enqueue does nothing until it is awaited"]
pub struct Enqueue<'a, J: Job> {
    queue: &'a Queue,
    job: J,
    override_queue: Option<String>,
    override_unique: Option<Option<String>>,
    run_at: Option<SystemTime>,
}

impl<'a, J: Job> Enqueue<'a, J> {
    /// Runs this one on a different queue than [`Job::QUEUE`].
    pub fn queue(mut self, queue: impl Into<String>) -> Self {
        self.override_queue = Some(queue.into());
        self
    }

    /// Kept so code written for the first queue still compiles. **It has no
    /// effect**: see [`Job::PRIORITY`].
    pub fn priority(self, _priority: i32) -> Self {
        self
    }

    /// Replaces [`Job::unique_key`] for this one enqueue.
    pub fn unique(mut self, key: impl Into<String>) -> Self {
        self.override_unique = Some(Some(key.into()));
        self
    }

    /// Drops [`Job::unique_key`] for this one enqueue.
    pub fn not_unique(mut self) -> Self {
        self.override_unique = Some(None);
        self
    }

    /// Does not run before `at`.
    pub fn at(mut self, at: chrono::DateTime<chrono::Utc>) -> Self {
        self.run_at = Some(at.into());
        self
    }

    /// Does not run for another `delay`.
    pub fn in_(mut self, delay: Duration) -> Self {
        self.run_at = Some(SystemTime::now() + delay);
        self
    }

    fn build(&self) -> Result<NewJob> {
        let payload = serde_json::to_vec(&self.job).map_err(|source| Error::Encode {
            job: J::NAME,
            source,
        })?;
        let unique_key = match &self.override_unique {
            Some(key) => key.clone(),
            None => self.job.unique_key(),
        };
        let caps = self.queue.inner.backend.capabilities();
        if unique_key.is_some() && !caps.satisfies(brocanth_queue::Uniqueness::UntilSuccess) {
            return Err(Error::Enqueue(
                brocanth_queue::EnqueueError::UniquenessUnsupported {
                    kind: J::NAME,
                    want: brocanth_queue::Uniqueness::UntilSuccess,
                    have: caps.uniqueness,
                },
            ));
        }
        Ok(NewJob {
            kind: J::NAME.to_string(),
            queue: self
                .override_queue
                .clone()
                .unwrap_or_else(|| J::QUEUE.to_string()),
            payload,
            run_at: self.run_at,
            max_attempts: J::MAX_ATTEMPTS,
            uniqueness: if unique_key.is_some() {
                UniquenessKind::UntilSuccess
            } else {
                UniquenessKind::None
            },
            unique_key,
        })
    }

    async fn send(self) -> Result<Enqueued> {
        let new = self.build()?;
        let scheduled = new.run_at.is_some_and(|at| at > SystemTime::now());
        let queue_name = new.queue.clone();
        let unique_key = new.unique_key.clone();
        let events = &self.queue.events;
        match self.queue.push(new).await? {
            Pushed::Enqueued(id) => {
                events.handle(JobEvent::Enqueued {
                    id: id.clone(),
                    name: J::NAME,
                    queue: &queue_name,
                    scheduled,
                });
                Ok(Enqueued::Inserted(id))
            }
            Pushed::Duplicate => {
                events.handle(JobEvent::Deduplicated {
                    name: J::NAME,
                    unique_key: unique_key.as_deref().unwrap_or(""),
                });
                Ok(Enqueued::Duplicate)
            }
        }
    }

    /// Enqueues inside the caller's transaction, so the job commits with the
    /// data it refers to, or not at all.
    ///
    /// The job goes to the engine's outbox, `queue_outbox`, in the same
    /// transaction. After the commit, the relay that every
    /// [`Worker`](crate::Worker) runs moves it to the queue, within a few
    /// hundred milliseconds. A rollback takes the job with it.
    ///
    /// ```no_run
    /// # use brocanth_jobs::{Job, JobContext, Queue};
    /// # use serde::{Deserialize, Serialize};
    /// # #[derive(Serialize, Deserialize)] struct ChargeCard { order: i64 }
    /// # impl Job for ChargeCard {
    /// #     const NAME: &'static str = "charge_card";
    /// #     type Error = std::io::Error;
    /// #     async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> { Ok(()) }
    /// # }
    /// # async fn example(queue: &Queue, pool: &sqlx::PgPool) -> Result<(), Box<dyn std::error::Error>> {
    /// let mut tx = pool.begin().await?;
    /// // ... insert the order with `&mut *tx` ...
    /// queue.enqueue(ChargeCard { order: 7 }).in_tx(&mut tx).await?;
    /// tx.commit().await?; // the order and the job commit together
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// The outbox refuses a write once it holds 100,000 undelivered rows,
    /// so an outage of the relay fails the request that would grow it
    /// rather than filling the disk. A queue on the memory backend has no
    /// transaction to join, and refuses.
    #[cfg(feature = "postgres")]
    pub async fn in_tx(self, tx: &mut sqlx::Transaction<'_, sqlx::Postgres>) -> Result<()> {
        use brocanth_queue::TxEnqueue;
        #[cfg(feature = "memory")]
        if let Store::Memory = self.queue.inner.store {
            return Err(Error::Backend(
                "the memory backend has no transaction to enqueue in\n\
                 help: use `.await` without `.in_tx`, or a Postgres queue"
                    .to_string(),
            ));
        }
        let new = self.build()?;
        brocanth_queue_postgres::Tx::new(tx)
            .enqueue_in_tx(brocanth_queue::Lane::Bulk, new)
            .await?;
        Ok(())
    }
}

impl<'a, J: Job> std::future::IntoFuture for Enqueue<'a, J> {
    type Output = Result<Enqueued>;
    type IntoFuture =
        std::pin::Pin<Box<dyn std::future::Future<Output = Self::Output> + Send + 'a>>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(self.send())
    }
}

// ---------------------------------------------------------------------------
// The recurring builder
// ---------------------------------------------------------------------------

/// A recurring schedule on its way into the store.
#[must_use = "a schedule does nothing until `install` is awaited"]
pub struct Recurring<'a, J: Job> {
    queue: &'a Queue,
    name: String,
    cron: String,
    job: J,
    misfire: Misfire,
    grace: Duration,
    override_queue: Option<String>,
    override_priority: Option<i32>,
}

impl<J: Job> Recurring<'_, J> {
    /// What to do about occurrences the workers slept through. Default
    /// [`Misfire::Skip`].
    pub fn misfire(mut self, misfire: Misfire) -> Self {
        self.misfire = misfire;
        self
    }

    /// How late an occurrence may be and still count as on time.
    ///
    /// Default sixty seconds, which is a maintenance tick plus slack. An
    /// occurrence later than this is "missed", and what happens then is
    /// [`Misfire`]'s business.
    pub fn grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    /// Overrides [`Job::QUEUE`] for the jobs this schedule enqueues.
    pub fn queue(mut self, queue: impl Into<String>) -> Self {
        self.override_queue = Some(queue.into());
        self
    }

    /// Kept so code written for the first queue still compiles. Stored, and
    /// **without effect**: see [`Job::PRIORITY`].
    pub fn priority(mut self, priority: i32) -> Self {
        self.override_priority = Some(priority);
        self
    }

    /// Writes the schedule.
    pub async fn install(self) -> Result<()> {
        let cron = Cron::parse(&self.cron)?;
        let now = self.queue.clock.now();
        let next = cron.next_after(now).ok_or_else(|| Error::Cron {
            expression: self.cron.clone(),
            reason: "this expression has no next occurrence within four years".to_string(),
        })?;
        let payload = serde_json::to_string(&self.job).map_err(|source| Error::Encode {
            job: J::NAME,
            source,
        })?;
        self.queue
            .inner
            .schedules
            .upsert(ScheduleRow {
                name: self.name,
                cron: cron.source().to_string(),
                job_name: J::NAME.to_string(),
                queue: self.override_queue.unwrap_or_else(|| J::QUEUE.to_string()),
                payload,
                priority: i64::from(self.override_priority.unwrap_or(J::PRIORITY)),
                max_attempts: i64::from(J::MAX_ATTEMPTS),
                next_run_at: to_millis(next),
                misfire: self.misfire.as_str().to_string(),
                grace_ms: self.grace.as_millis() as i64,
            })
            .await
    }
}
