//! Shared fixtures for the integration tests.
//!
//! Every behaviour test runs on every backend: [`on_every_backend!`] turns
//! one `async fn name(queue: Queue)` into a `memory::name` test, which always
//! runs, and a `postgres::name` test, which runs when
//! `BROCANTH_JOBS_TEST_POSTGRES` or `BROCANTH_TEST_DATABASE_URL` names a
//! server and says loudly on stderr when it did not. One suite, two
//! backends: the suite that proves the memory backend is the suite that
//! proves Postgres.
//!
//! Each Postgres test gets its own schema, so the tests run in parallel and
//! never see each other's rows, and nothing is truncated. Jobs report what
//! they did through [`record`], keyed by a token carried in the payload,
//! which is how one job type serves several tests running at once.

#![allow(dead_code, unused_macros)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use brocanth_jobs::{Job, JobContext, Queue};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// A store per test
// ---------------------------------------------------------------------------

/// Runs each named `async fn(queue: Queue)` once per backend.
macro_rules! on_every_backend {
    ($($name:ident),* $(,)?) => {
        mod memory {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $name() {
                    super::$name(super::common::memory_queue()).await;
                }
            )*
        }
        mod postgres {
            $(
                #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
                async fn $name() {
                    match super::common::postgres_queue().await {
                        Some(queue) => super::$name(queue).await,
                        None => super::common::skipped(super::common::NO_POSTGRES),
                    }
                }
            )*
        }
    };
}

/// What a Postgres test says when it did not run.
pub const NO_POSTGRES: &str =
    "no Postgres server; set BROCANTH_JOBS_TEST_POSTGRES or BROCANTH_TEST_DATABASE_URL to run this";

/// A queue in this process's memory.
pub fn memory_queue() -> Queue {
    Queue::memory()
}

/// A queue on the Postgres test server, in a schema of its own, with its
/// tables created. `None` when no variable names a server.
pub async fn postgres_queue() -> Option<Queue> {
    let pool = postgres_pool().await?;
    let queue = Queue::from_pool(pool);
    queue.migrate().await.expect("creating the tables");
    Some(queue)
}

/// A pool on the Postgres test server whose `search_path` is a new, empty
/// schema. `None` when no variable names a server.
pub async fn postgres_pool() -> Option<sqlx::PgPool> {
    use std::str::FromStr;

    let url = postgres_url()?;
    let schema = format!("jobs_{}", token("t").replace('-', "_"));
    let admin = sqlx::PgPool::connect(&url)
        .await
        .expect("connecting to Postgres");
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE SCHEMA {schema}")))
        .execute(&admin)
        .await
        .expect("creating the test schema");
    admin.close().await;
    let options = sqlx::postgres::PgConnectOptions::from_str(&url)
        .expect("a Postgres URL")
        .options([("search_path", schema.as_str())]);
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(6)
            .connect_with(options)
            .await
            .expect("connecting to Postgres"),
    )
}

/// The Postgres test server's URL, or `None` when no variable names one.
/// An empty variable counts as unset.
pub fn postgres_url() -> Option<String> {
    ["BROCANTH_JOBS_TEST_POSTGRES", "BROCANTH_TEST_DATABASE_URL"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .filter(|url| !url.trim().is_empty())
        })
}

/// Announce that a test did not run, where the announcement cannot be
/// swallowed.
///
/// `cargo test` captures `println!` and `eprintln!` and shows them only for a
/// test that fails, so a skip announced that way is invisible — and a test
/// that silently passes by running nothing is worse than one that is honestly
/// skipped. This writes to file descriptor 2 directly, which the capture does
/// not intercept, and names the test through its thread name.
pub fn skipped(reason: &str) {
    use std::io::Write;

    let thread = std::thread::current();
    let name = thread.name().unwrap_or("a test");
    let line = format!("!! SKIPPED {name}: {reason}\n");

    if let Ok(mut fd2) = std::fs::OpenOptions::new().write(true).open("/dev/stderr") {
        if fd2.write_all(line.as_bytes()).is_ok() {
            let _ = fd2.flush();
            return;
        }
    }
    eprint!("{line}");
}

// ---------------------------------------------------------------------------
// What the jobs did
// ---------------------------------------------------------------------------

type Log = Vec<(String, Instant)>;

fn recorder() -> &'static Mutex<HashMap<String, Log>> {
    static RECORDER: OnceLock<Mutex<HashMap<String, Log>>> = OnceLock::new();
    RECORDER.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Notes that `what` happened under `token`.
pub fn record(token: &str, what: impl Into<String>) {
    recorder()
        .lock()
        .expect("recorder")
        .entry(token.to_string())
        .or_default()
        .push((what.into(), Instant::now()));
}

/// Everything recorded under `token`, in order.
pub fn log(token: &str) -> Vec<String> {
    recorder()
        .lock()
        .expect("recorder")
        .get(token)
        .map(|entries| entries.iter().map(|(what, _)| what.clone()).collect())
        .unwrap_or_default()
}

/// When each entry under `token` was recorded.
pub fn timings(token: &str) -> Vec<Instant> {
    recorder()
        .lock()
        .expect("recorder")
        .get(token)
        .map(|entries| entries.iter().map(|(_, at)| *at).collect())
        .unwrap_or_default()
}

/// A token nothing else will use.
///
/// The process id is in it, not only a counter, because the Postgres tests
/// share one server: two `cargo test` processes against the same database
/// would otherwise both start at `name-0` and collide on a unique key.
pub fn token(name: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{name}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

/// Waits for `condition`, polling, up to `limit`. Panics with `what` on
/// timeout, so a hung test says what it was waiting for.
pub async fn wait_for<F, Fut>(what: &str, limit: Duration, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if condition().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out after {limit:?} waiting for {what}");
}

// ---------------------------------------------------------------------------
// The job types
// ---------------------------------------------------------------------------

/// Succeeds, and says it ran.
#[derive(Serialize, Deserialize)]
pub struct Echo {
    pub token: String,
    pub n: u32,
}

impl Job for Echo {
    const NAME: &'static str = "echo";
    type Error = std::convert::Infallible;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, format!("ran {}", self.n));
        Ok(())
    }
}

/// Never succeeds. Three attempts, a backoff small enough to watch and with no
/// jitter so the growth is exact rather than probable.
#[derive(Serialize, Deserialize)]
pub struct AlwaysFails {
    pub token: String,
}

impl Job for AlwaysFails {
    const NAME: &'static str = "always_fails";
    const MAX_ATTEMPTS: u32 = 3;
    const BACKOFF: brocanth_jobs::Backoff =
        brocanth_jobs::Backoff::new(Duration::from_millis(120), Duration::from_secs(60))
            .jitter(0.0);
    type Error = String;

    async fn perform(&self, cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, format!("attempt {}", cx.attempt()));
        Err(format!("nope, attempt {}", cx.attempt()))
    }
}

/// Takes a while and then succeeds.
#[derive(Serialize, Deserialize)]
pub struct Slow {
    pub token: String,
    pub millis: u64,
}

impl Job for Slow {
    const NAME: &'static str = "slow";
    type Error = std::convert::Infallible;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, "slow started");
        tokio::time::sleep(Duration::from_millis(self.millis)).await;
        record(&self.token, "slow finished");
        Ok(())
    }
}

/// Never returns. For shutdown deadlines and for orphaning a worker.
#[derive(Serialize, Deserialize)]
pub struct Hangs {
    pub token: String,
}

impl Job for Hangs {
    const NAME: &'static str = "hangs";
    type Error = std::convert::Infallible;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, "hang started");
        loop {
            tokio::time::sleep(Duration::from_secs(3_600)).await;
        }
    }
}

/// Hangs on its first attempt and succeeds on any later one, so a job can be
/// orphaned and then actually finish when somebody reclaims it.
#[derive(Serialize, Deserialize)]
pub struct HangsOnce {
    pub token: String,
}

impl Job for HangsOnce {
    const NAME: &'static str = "hangs_once";
    type Error = std::convert::Infallible;

    async fn perform(&self, cx: &JobContext) -> Result<(), Self::Error> {
        if cx.attempt() == 1 {
            record(&self.token, "first attempt, hanging");
            loop {
                tokio::time::sleep(Duration::from_secs(3_600)).await;
            }
        }
        record(&self.token, format!("finished on attempt {}", cx.attempt()));
        Ok(())
    }
}

/// Fires once a day, for the recurring tests.
#[derive(Serialize, Deserialize)]
pub struct Nightly {
    pub token: String,
}

impl Job for Nightly {
    const NAME: &'static str = "nightly";
    type Error = std::convert::Infallible;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, "nightly ran");
        Ok(())
    }
}

/// Fails its first attempt and succeeds on the next. A small backoff, no
/// jitter.
#[derive(Serialize, Deserialize)]
pub struct FailsFirst {
    pub token: String,
}

impl Job for FailsFirst {
    const NAME: &'static str = "fails_first";
    const MAX_ATTEMPTS: u32 = 3;
    const BACKOFF: brocanth_jobs::Backoff =
        brocanth_jobs::Backoff::new(Duration::from_millis(50), Duration::from_secs(60)).jitter(0.0);
    type Error = String;

    async fn perform(&self, cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, format!("attempt {}", cx.attempt()));
        if cx.attempt() == 1 {
            return Err("not yet".to_string());
        }
        Ok(())
    }
}

/// Fails, and has one attempt only: the first failure is the last.
#[derive(Serialize, Deserialize)]
pub struct DiesAtOnce {
    pub token: String,
}

impl Job for DiesAtOnce {
    const NAME: &'static str = "dies_at_once";
    const MAX_ATTEMPTS: u32 = 1;
    type Error = String;

    async fn perform(&self, cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, format!("attempt {}", cx.attempt()));
        Err(format!("gave up on {}", self.token))
    }
}

/// Never returns, and has one attempt: a worker's job timeout kills it.
#[derive(Serialize, Deserialize)]
pub struct TimesOut {
    pub token: String,
}

impl Job for TimesOut {
    const NAME: &'static str = "times_out";
    const MAX_ATTEMPTS: u32 = 1;
    type Error = std::convert::Infallible;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, "started");
        loop {
            tokio::time::sleep(Duration::from_secs(3_600)).await;
        }
    }
}

/// Panics, and has one attempt.
#[derive(Serialize, Deserialize)]
pub struct Panics {
    pub token: String,
}

impl Job for Panics {
    const NAME: &'static str = "panics";
    const MAX_ATTEMPTS: u32 = 1;
    type Error = std::convert::Infallible;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, "started");
        panic!("boom in {}", self.token);
    }
}

/// Fails, and waits an hour before its retry, so the retry stays on the
/// retries screen for the whole test, however slow the machine. `AlwaysFails` comes due again in
/// 120 ms, and a due retry is a ready job, not a row on that screen.
#[derive(Serialize, Deserialize)]
pub struct RetriesInAnHour {
    pub token: String,
}

impl Job for RetriesInAnHour {
    const NAME: &'static str = "retries_in_an_hour";
    const MAX_ATTEMPTS: u32 = 3;
    const BACKOFF: brocanth_jobs::Backoff =
        brocanth_jobs::Backoff::new(Duration::from_secs(3600), Duration::from_secs(7200))
            .jitter(0.0);
    type Error = String;

    async fn perform(&self, _cx: &JobContext) -> Result<(), Self::Error> {
        record(&self.token, "failed");
        Err("try again later".to_string())
    }
}
