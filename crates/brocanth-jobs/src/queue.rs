//! The queue: the handle an application enqueues through, and everything you
//! can do to work that is already in the table.
//!
//! A [`Queue`] is cheap to clone — it is an `Arc` around a pool, a clock and an
//! event sink — so it goes in the context, in a handler, in a hook, anywhere.

use std::sync::Arc;
use std::time::Duration;

use crate::clock::{from_millis, to_millis, Clock, SystemClock};
use crate::cron::Cron;
use crate::error::{Error, Result};
use crate::events::{JobEvent, JobEvents, Silent};
use crate::job::{Job, JobId};
use crate::store::{schema, Backend, DeadJob, Enqueued, NewJob, Stats, Store};

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

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

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
/// let queue = Queue::connect("sqlite://jobs.db").await?;
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
    store: Arc<Store>,
    clock: Arc<dyn Clock>,
    events: Arc<dyn JobEvents>,
}

impl Queue {
    /// Opens a queue from a database URL.
    ///
    /// `postgres://…` or `postgresql://…` needs the `postgres` feature;
    /// `sqlite://…` needs `sqlite`. A URL whose backend was not compiled in is
    /// an error that says which Cargo feature to turn on, because that is where
    /// the fix is.
    ///
    /// For SQLite this sets WAL journalling and a five-second `busy_timeout`,
    /// which the claim path depends on. Handing in a pool built elsewhere with
    /// [`Queue::from_pool`] means setting them yourself.
    pub async fn connect(url: &str) -> Result<Self> {
        let backend = connect_backend(url).await?;
        Ok(Self::from_pool(backend))
    }

    /// Builds a queue on a pool the application already has.
    ///
    /// This is the path for an application that also uses `brocanth-model`:
    /// SeaORM 2.0 hands out its underlying `sqlx` pool, so the queue and the
    /// models share one set of connections rather than opening a second pool
    /// against the same database.
    pub fn from_pool(pool: impl Into<Backend>) -> Self {
        Self {
            store: Arc::new(Store::new(pool.into())),
            clock: Arc::new(SystemClock),
            events: Arc::new(Silent),
        }
    }

    /// Replaces the clock. See [`crate::clock`] for why there is one.
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

    /// Which backend this queue is on: `"postgres"` or `"sqlite"`.
    pub fn backend(&self) -> &'static str {
        self.store.backend().name()
    }

    /// The current time, from this queue's clock.
    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.now()
    }

    /// Creates the three tables if they are not there.
    ///
    /// Safe to call on every boot. See [`Queue::schema_sql`] for the version an
    /// application keeps in `migrations/` instead.
    pub async fn migrate(&self) -> Result<()> {
        self.store.migrate().await
    }

    /// The DDL this queue would run, as statements.
    ///
    /// For an application whose rule is that the schema lives in files a human
    /// reviewed — which is §4.3 of the plan, and the right rule — paste these
    /// into a migration and never call [`Queue::migrate`].
    pub fn schema_sql(&self) -> &'static [&'static str] {
        schema(self.store.backend().dialect())
    }

    // -- enqueue ------------------------------------------------------------

    /// Puts a job on its queue.
    ///
    /// Returns a builder that is also a future, so the simple case is
    /// `queue.enqueue(job).await?` and the elaborate one chains `.at()`,
    /// `.in_()`, `.queue()`, `.priority()` or `.unique()` first.
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
            override_priority: None,
            override_unique: None,
            run_at: None,
        }
    }

    // -- inspection ---------------------------------------------------------

    /// Counts by state, per queue and in total, plus the age of the oldest
    /// runnable job.
    pub async fn stats(&self) -> Result<Stats> {
        let now = to_millis(self.clock.now());
        self.store.stats(now).await
    }

    /// One job's state, attempt count and `run_at`. `None` once it has
    /// finished and its row is gone.
    pub async fn inspect(&self, id: JobId) -> Result<Option<JobState>> {
        Ok(self
            .store
            .job_state(id)
            .await?
            .map(|(state, attempts, run_at)| JobState {
                state,
                attempts: attempts.max(0) as u32,
                run_at: from_millis(run_at),
            }))
    }

    // -- the dead letter ----------------------------------------------------

    /// The most recent dead jobs, newest first.
    pub async fn dead_jobs(&self, limit: u32, offset: u32) -> Result<Vec<DeadJob>> {
        self.store
            .dead_jobs(i64::from(limit), i64::from(offset))
            .await
    }

    /// One dead job by its dead-letter id.
    pub async fn dead_job(&self, dead_id: i64) -> Result<Option<DeadJob>> {
        self.store.dead_job(dead_id).await
    }

    /// Puts a dead job back on its queue with its attempt count reset.
    ///
    /// `Ok(None)` means either that the dead row is gone, or that a live job
    /// already holds its unique key — in which case the dead row is left where
    /// it is rather than being deleted without being replayed.
    pub async fn requeue_dead(&self, dead_id: i64) -> Result<Option<JobId>> {
        let now = to_millis(self.clock.now());
        self.store.requeue_dead(dead_id, now).await
    }

    /// Deletes a dead job for good.
    pub async fn discard_dead(&self, dead_id: i64) -> Result<bool> {
        Ok(self.store.discard_dead(dead_id).await? > 0)
    }

    // -- maintenance --------------------------------------------------------

    /// Puts back every running job whose worker stopped heartbeating longer
    /// ago than `stale_after`.
    ///
    /// A [`Worker`](crate::Worker) does this on a timer. It is public because a
    /// CLI wants it, a test wants it, and an operator who has just lost a
    /// machine wants it now rather than on the next tick.
    pub async fn reclaim_orphans(&self, stale_after: Duration) -> Result<Reclaimed> {
        let now = self.clock.now();
        let stale_before = to_millis(now) - stale_after.as_millis() as i64;
        let (requeued, dead_lettered) = self
            .store
            .reclaim_orphans(to_millis(now), stale_before)
            .await?;
        if requeued > 0 || dead_lettered > 0 {
            self.events.handle(JobEvent::Reclaimed {
                requeued,
                dead_lettered,
            });
        }
        Ok(Reclaimed {
            requeued,
            dead_lettered,
        })
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
        Ok(self.store.remove_schedule(name).await? > 0)
    }

    /// Enqueues whatever the recurring schedules owe, and advances them.
    ///
    /// A [`Worker`](crate::Worker) calls this on its maintenance tick. Running
    /// it from two workers at once is safe: every occurrence is enqueued with
    /// the unique key `"<schedule>@<occurrence epoch millis>"`, so the second
    /// worker's insert folds into the first one's row instead of producing a
    /// second job.
    pub async fn tick_schedules(&self) -> Result<u32> {
        let now = self.clock.now();
        let now_ms = to_millis(now);
        let due = self.store.due_schedules(now_ms).await?;
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

            let fired_at = match to_fire {
                Some(occurrence) => {
                    let new = NewJob {
                        queue: row.queue.clone(),
                        // The schedule row owns the name; the registry
                        // resolves it on the worker that runs it.
                        job_name: row.job_name.clone(),
                        payload: row.payload.clone(),
                        priority: row.priority,
                        max_attempts: row.max_attempts,
                        run_at: now_ms,
                        unique_key: Some(format!("{}@{}", row.name, occurrence)),
                    };
                    let result = self.store.enqueue(&new, now_ms).await?;
                    if !result.is_duplicate() {
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

            self.store
                .advance_schedule(&row.name, occurrence_ms, fired_at, missed)
                .await?;
        }

        Ok(fired)
    }

    // -- internals the worker uses ------------------------------------------

    /// Reports a failure in the queue's own machinery through the event sink.
    pub(crate) fn events_for_error(&self, during: &'static str, error: &Error) {
        self.events
            .handle(JobEvent::InternalError { during, error });
    }

    pub(crate) fn event_sink(&self) -> Arc<dyn JobEvents> {
        Arc::clone(&self.events)
    }

    pub(crate) fn clock_handle(&self) -> Arc<dyn Clock> {
        Arc::clone(&self.clock)
    }

    pub(crate) fn store_handle(&self) -> Arc<Store> {
        Arc::clone(&self.store)
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

/// What [`Queue::reclaim_orphans`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reclaimed {
    /// Jobs put back on their queue.
    pub requeued: u64,
    /// Jobs that had no attempts left and went to the dead letter instead.
    pub dead_lettered: u64,
}

/// One job's position, for an admin screen or a test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JobState {
    /// `"pending"` or `"running"`.
    pub state: String,
    /// Attempts started so far.
    pub attempts: u32,
    /// When it next becomes runnable.
    pub run_at: chrono::DateTime<chrono::Utc>,
}

// ---------------------------------------------------------------------------
// The enqueue builder
// ---------------------------------------------------------------------------

/// A job on its way into the table. Awaiting it is what sends it.
#[must_use = "an enqueue does nothing until it is awaited"]
pub struct Enqueue<'a, J: Job> {
    queue: &'a Queue,
    job: J,
    override_queue: Option<String>,
    override_priority: Option<i32>,
    override_unique: Option<Option<String>>,
    run_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl<'a, J: Job> Enqueue<'a, J> {
    /// Runs this one on a different queue than [`Job::QUEUE`].
    pub fn queue(mut self, queue: impl Into<String>) -> Self {
        self.override_queue = Some(queue.into());
        self
    }

    /// Runs this one at a different priority than [`Job::PRIORITY`]. Lower
    /// runs first.
    pub fn priority(mut self, priority: i32) -> Self {
        self.override_priority = Some(priority);
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
        self.run_at = Some(at);
        self
    }

    /// Does not run for another `delay`.
    pub fn in_(mut self, delay: Duration) -> Self {
        let now = self.queue.clock.now();
        self.run_at = Some(
            now + chrono::Duration::from_std(delay)
                .unwrap_or_else(|_| chrono::Duration::days(3_650)),
        );
        self
    }

    async fn send(self) -> Result<Enqueued> {
        let now = self.queue.clock.now();
        let payload = serde_json::to_string(&self.job).map_err(|source| Error::Encode {
            job: J::NAME,
            source,
        })?;
        let run_at = self.run_at.unwrap_or(now);
        let unique = match self.override_unique {
            Some(key) => key,
            None => self.job.unique_key(),
        };
        let queue_name = self.override_queue.unwrap_or_else(|| J::QUEUE.to_string());

        let new = NewJob {
            queue: queue_name,
            job_name: J::NAME.to_string(),
            payload,
            priority: i64::from(self.override_priority.unwrap_or(J::PRIORITY)),
            max_attempts: i64::from(J::MAX_ATTEMPTS),
            run_at: to_millis(run_at),
            unique_key: unique,
        };

        let result = self.queue.store.enqueue(&new, to_millis(now)).await?;
        match result {
            Enqueued::Inserted(id) => self.queue.events.handle(JobEvent::Enqueued {
                id,
                name: J::NAME,
                queue: &new.queue,
                scheduled: run_at > now,
            }),
            Enqueued::Duplicate(id) => self.queue.events.handle(JobEvent::Deduplicated {
                id,
                name: J::NAME,
                unique_key: new.unique_key.as_deref().unwrap_or(""),
            }),
        }
        Ok(result)
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

/// A recurring schedule on its way into the table.
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

    /// Overrides [`Job::PRIORITY`] for the jobs this schedule enqueues.
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
            .store
            .upsert_schedule(
                &self.name,
                cron.source(),
                J::NAME,
                self.override_queue.as_deref().unwrap_or(J::QUEUE),
                &payload,
                i64::from(self.override_priority.unwrap_or(J::PRIORITY)),
                i64::from(J::MAX_ATTEMPTS),
                to_millis(next),
                self.misfire.as_str(),
                self.grace.as_millis() as i64,
            )
            .await
    }
}

// ---------------------------------------------------------------------------
// Connecting
// ---------------------------------------------------------------------------

async fn connect_backend(url: &str) -> Result<Backend> {
    let scheme = url.split(':').next().unwrap_or("");
    match scheme {
        #[cfg(feature = "postgres")]
        "postgres" | "postgresql" => Ok(Backend::Postgres(
            sqlx::postgres::PgPoolOptions::new().connect(url).await?,
        )),
        #[cfg(not(feature = "postgres"))]
        "postgres" | "postgresql" => Err(Error::Backend(
            "this build of brocanth-jobs has no Postgres driver; add \
             `features = [\"postgres\"]` to its dependency in Cargo.toml"
                .to_string(),
        )),
        #[cfg(feature = "sqlite")]
        "sqlite" => {
            use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
            use std::str::FromStr;
            let options = SqliteConnectOptions::from_str(url)?
                .create_if_missing(true)
                // WAL lets readers run while the one writer writes, which is
                // the difference between a usable SQLite queue and a stalled
                // one. `busy_timeout` is what makes `BEGIN IMMEDIATE` wait for
                // the write lock rather than failing instantly.
                .journal_mode(SqliteJournalMode::Wal)
                .busy_timeout(Duration::from_secs(5))
                .foreign_keys(true);
            Ok(Backend::Sqlite(
                SqlitePoolOptions::new().connect_with(options).await?,
            ))
        }
        #[cfg(not(feature = "sqlite"))]
        "sqlite" => Err(Error::Backend(
            "this build of brocanth-jobs has no SQLite driver; add \
             `features = [\"sqlite\"]` to its dependency in Cargo.toml"
                .to_string(),
        )),
        other => Err(Error::Backend(format!(
            "`{other}` is not a database this queue knows; the URL must start \
             with `postgres://` or `sqlite://`"
        ))),
    }
}
