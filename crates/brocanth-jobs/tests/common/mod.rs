//! Shared fixtures for the integration tests.
//!
//! Every test gets its own SQLite file in its own temporary directory, so the
//! tests are parallel by default and cannot see each other's rows. Jobs report
//! what they did through [`Recorder`], keyed by a token carried in the payload,
//! which is how one job type serves several tests running at once.
//!
//! SQLite rather than Postgres because `cargo test` must pass on a laptop with
//! no server running, and it stays that way now that `postgres` is the default
//! feature: the crate carries a dev-dependency on itself with `sqlite` on, so
//! this harness is always compiled for tests whatever the library build picks.
//!
//! That means the backend this suite runs on is **not** the default one, which
//! is a real cost rather than a detail. `postgres.rs` is what pays it back,
//! and only when `BROCANTH_JOBS_TEST_POSTGRES` or `BROCANTH_TEST_DATABASE_URL`
//! names a server. Every test there that does not run says so on stderr.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use brocanth_jobs::{Job, JobContext, Queue};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// A database per test
// ---------------------------------------------------------------------------

/// A queue on a fresh SQLite file, with its tables created.
///
/// Hold the returned [`tempfile::TempDir`] for as long as the queue is used;
/// dropping it deletes the database.
pub async fn sqlite_queue() -> (Queue, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("jobs.db");
    let queue = Queue::connect(&format!("sqlite://{}", path.display()))
        .await
        .expect("opening the queue");
    queue.migrate().await.expect("creating the tables");
    (queue, dir)
}

/// The Postgres test server's URL, or `None` when no variable names one.
pub fn postgres_url() -> Option<String> {
    ["BROCANTH_JOBS_TEST_POSTGRES", "BROCANTH_TEST_DATABASE_URL"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
        .filter(|url| !url.is_empty())
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
