//! Shared fixtures for the integration tests.
//!
//! Every test gets its own SQLite file in its own temporary directory, so the
//! tests are parallel by default and cannot see each other's rows. Jobs report
//! what they did through [`Recorder`], keyed by a token carried in the payload,
//! which is how one job type serves several tests running at once.
//!
//! SQLite rather than Postgres because `cargo test` must pass on a laptop with
//! no server running. The Postgres path is exercised by the same tests when
//! `BROCANTH_JOBS_TEST_POSTGRES` names a database; see `postgres.rs`.

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

/// A token nothing else in the process will use.
pub fn token(name: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{name}-{}", N.fetch_add(1, Ordering::Relaxed))
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
