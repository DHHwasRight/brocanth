//! An in-process backend.
//!
//! It exists for two reasons. A test needs a queue with no server, and the
//! conformance suite needs something to check itself against: a suite that
//! has never passed proves nothing.
//!
//! It also takes its [`Capabilities`] as a constructor argument. Running the
//! same storage twice, once unbounded and once with the limits SQS declares,
//! is how a Postgres assumption in a test is caught without an SQS account.

#![forbid(unsafe_code)]

mod inspect;
mod outbox;

pub use outbox::MemoryOutbox;

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use brocanth_queue::{
    Backend, BackendError, Capabilities, ClaimRequest, Claimed, JobFailure, JobId, LeaseRef,
    LeaseState, NewJob, Pushed, Settle, UniquenessKind,
};

#[derive(Debug, Clone)]
struct Row {
    id: u64,
    kind: String,
    queue: String,
    payload: Vec<u8>,
    run_at: SystemTime,
    attempts: u32,
    max_attempts: u32,
    token: Option<String>,
    lease_until: SystemTime,
    unique_key: Option<String>,
    enqueued_at: SystemTime,
    // Written by the claim and read by nothing but the screens. See
    // `Inspect`: a worker roster that is a column on the job is a roster
    // that cannot disagree with the jobs.
    claimed_by: Option<String>,
    claimed_at: Option<SystemTime>,
    last_error: Option<String>,
}

impl Row {
    fn claimable(&self, now: SystemTime) -> bool {
        self.run_at <= now && (self.token.is_none() || self.lease_until <= now)
    }

    fn leased(&self, now: SystemTime) -> bool {
        self.token.is_some() && self.lease_until > now
    }
}

/// A job that ran out of attempts, kept for inspection.
///
/// It records what a requeue needs, which is more than a log line can hold:
/// a dead letter you cannot put back is a log line with extra steps.
#[derive(Debug, Clone)]
pub struct Dead {
    /// The dead letter's own id, which is what a requeue or a discard
    /// names. Not the id the job had: those number independently.
    pub id: u64,
    /// The id the job had while it was live.
    pub job_id: u64,
    /// Its kind.
    pub kind: String,
    /// Its queue.
    pub queue: String,
    /// Its payload.
    pub payload: Vec<u8>,
    /// How many times it ran.
    pub attempts: u32,
    /// How many times it was allowed to run.
    pub max_attempts: u32,
    /// Its deduplication key, if it had one.
    pub unique_key: Option<String>,
    /// Why it died.
    pub failure: JobFailure,
    /// When it was first enqueued.
    pub enqueued_at: SystemTime,
    /// When it was given up on.
    pub died_at: SystemTime,
}

#[derive(Default)]
struct State {
    seq: u64,
    dead_seq: u64,
    tokens: u64,
    rows: BTreeMap<u64, Row>,
    dead: Vec<Dead>,
    windows: HashMap<String, SystemTime>,
}

/// An in-process queue.
pub struct Memory {
    state: Mutex<State>,
    caps: Capabilities,
    sweeps: std::sync::atomic::AtomicU64,
}

impl Default for Memory {
    fn default() -> Self {
        Memory::new()
    }
}

impl std::fmt::Debug for Memory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Memory").finish_non_exhaustive()
    }
}

impl Memory {
    /// An unbounded in-process queue.
    pub fn new() -> Self {
        Memory::with_capabilities(Capabilities::unbounded())
    }

    /// An in-process queue that declares, and enforces, someone else's
    /// limits.
    pub fn with_capabilities(caps: Capabilities) -> Self {
        Memory {
            state: Mutex::new(State::default()),
            caps,
            sweeps: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// The limits a managed queue declares: a small payload, a 15 minute
    /// delay ceiling, no server-side filter, a 5 minute dedup window.
    pub fn sqs_shaped() -> Self {
        Memory::with_capabilities(Capabilities {
            max_payload_bytes: Some(256 * 1024),
            max_delay: Some(Duration::from_secs(900)),
            max_backoff: Some(Duration::from_secs(12 * 60 * 60)),
            filter_by_kind: false,
            uniqueness: brocanth_queue::UniquenessSupport::Window(Duration::from_secs(300)),
            cancel_pending: false,
            exact_depth: false,
            max_in_flight: Some(120_000),
            max_claim_batch: 10,
            ordered: false,
        })
    }

    /// How many times [`Backend::sweep`] has been called.
    ///
    /// A test reads it to prove the sweep runs while the queue is busy. It
    /// used to run only after a claim came back empty, which meant a queue
    /// with a standing backlog never swept at all.
    pub fn sweeps(&self) -> u64 {
        self.sweeps.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Everything in the dead letter.
    pub fn dead_letter(&self) -> Vec<Dead> {
        self.state.lock().unwrap().dead.clone()
    }

    /// How many jobs exist, claimable or leased.
    pub fn len(&self) -> usize {
        self.state.lock().unwrap().rows.len()
    }

    /// True when no job exists.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn held(state: &State, key: &str, kind: UniquenessKind, now: SystemTime) -> bool {
        match kind {
            UniquenessKind::None => false,
            UniquenessKind::Window(_) => state.windows.get(key).is_some_and(|until| *until > now),
            UniquenessKind::UntilSuccess => state
                .rows
                .values()
                .any(|r| r.unique_key.as_deref() == Some(key)),
        }
    }
}

impl Backend for Memory {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn capabilities(&self) -> Capabilities {
        self.caps.clone()
    }

    async fn push(&self, batch: Vec<NewJob>) -> Result<Vec<Pushed>, BackendError> {
        let now = SystemTime::now();
        let mut state = self.state.lock().unwrap();
        let mut out = Vec::with_capacity(batch.len());
        for job in batch {
            if let Some(limit) = self.caps.max_payload_bytes {
                if job.payload.len() > limit {
                    return Err(BackendError::Rejected(format!(
                        "payload is {} bytes, the limit is {limit}",
                        job.payload.len()
                    )));
                }
            }
            let run_at = job.run_at.unwrap_or(now);
            if let Some(limit) = self.caps.max_delay {
                let want = run_at.duration_since(now).unwrap_or(Duration::ZERO);
                if want > limit {
                    return Err(BackendError::Rejected(format!(
                        "a delay of {want:?} is past the {limit:?} ceiling"
                    )));
                }
            }
            if let Some(key) = job.unique_key.as_deref() {
                if Self::held(&state, key, job.uniqueness, now) {
                    out.push(Pushed::Duplicate);
                    continue;
                }
                if let UniquenessKind::Window(ms) = job.uniqueness {
                    state
                        .windows
                        .insert(key.to_string(), now + Duration::from_millis(ms));
                }
            }
            state.seq += 1;
            let id = state.seq;
            state.rows.insert(
                id,
                Row {
                    id,
                    kind: job.kind,
                    queue: job.queue,
                    payload: job.payload,
                    run_at,
                    attempts: 0,
                    max_attempts: job.max_attempts,
                    token: None,
                    lease_until: now,
                    unique_key: job.unique_key,
                    enqueued_at: now,
                    claimed_by: None,
                    claimed_at: None,
                    last_error: None,
                },
            );
            out.push(Pushed::Enqueued(JobId(id.to_string())));
        }
        Ok(out)
    }

    async fn claim(&self, req: &ClaimRequest) -> Result<Vec<Claimed>, BackendError> {
        let now = SystemTime::now();
        let max = req.max.min(self.caps.max_claim_batch);
        let mut state = self.state.lock().unwrap();
        let ids: Vec<u64> = state
            .rows
            .values()
            .filter(|r| r.queue == req.queue && r.claimable(now))
            .filter(|r| match (&req.kinds, self.caps.filter_by_kind) {
                (Some(ks), true) => ks.iter().any(|k| k == &r.kind),
                _ => true,
            })
            .take(max)
            .map(|r| r.id)
            .collect();
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            state.tokens += 1;
            let token = format!("t{}", state.tokens);
            let row = state.rows.get_mut(&id).expect("just selected");
            row.token = Some(token.clone());
            row.lease_until = now + req.lease;
            row.attempts += 1;
            row.claimed_by = Some(req.worker.clone());
            row.claimed_at = Some(now);
            out.push(Claimed {
                lease: LeaseRef {
                    id: JobId(id.to_string()),
                    token,
                    expires_at: row.lease_until,
                },
                kind: row.kind.clone(),
                queue: row.queue.clone(),
                payload: row.payload.clone(),
                attempt: row.attempts,
                max_attempts: row.max_attempts,
            });
        }
        Ok(out)
    }

    async fn heartbeat(
        &self,
        lease: &LeaseRef,
        extend: Duration,
    ) -> Result<LeaseState, BackendError> {
        let now = SystemTime::now();
        let mut state = self.state.lock().unwrap();
        let Ok(id) = lease.id.as_str().parse::<u64>() else {
            return Ok(LeaseState::Lost);
        };
        match state.rows.get_mut(&id) {
            Some(row) if row.token.as_deref() == Some(lease.token.as_str()) => {
                row.lease_until = now + extend;
                Ok(LeaseState::Held)
            }
            _ => Ok(LeaseState::Lost),
        }
    }

    async fn settle(&self, lease: &LeaseRef, outcome: Settle) -> Result<LeaseState, BackendError> {
        let mut state = self.state.lock().unwrap();
        let Ok(id) = lease.id.as_str().parse::<u64>() else {
            return Ok(LeaseState::Lost);
        };
        let matches = state
            .rows
            .get(&id)
            .is_some_and(|r| r.token.as_deref() == Some(lease.token.as_str()));
        if !matches {
            return Ok(LeaseState::Lost);
        }
        match outcome {
            Settle::Done => {
                state.rows.remove(&id);
            }
            Settle::Dead { failure } => {
                if let Some(row) = state.rows.remove(&id) {
                    state.dead_seq += 1;
                    let dead_id = state.dead_seq;
                    state.dead.push(Dead {
                        id: dead_id,
                        job_id: row.id,
                        kind: row.kind,
                        queue: row.queue,
                        payload: row.payload,
                        attempts: row.attempts,
                        max_attempts: row.max_attempts,
                        unique_key: row.unique_key,
                        failure,
                        enqueued_at: row.enqueued_at,
                        died_at: SystemTime::now(),
                    });
                }
            }
            Settle::Retry { at, failure } => {
                if let Some(row) = state.rows.get_mut(&id) {
                    row.token = None;
                    row.run_at = at;
                    row.claimed_by = None;
                    row.claimed_at = None;
                    row.last_error = Some(failure.to_string());
                }
            }
            Settle::Release { not_before, .. } => {
                if let Some(row) = state.rows.get_mut(&id) {
                    row.token = None;
                    row.run_at = not_before;
                    row.claimed_by = None;
                    row.claimed_at = None;
                    // A release does not spend an attempt. This is the one
                    // line the rolling-deploy case depends on.
                    row.attempts = row.attempts.saturating_sub(1);
                }
            }
        }
        Ok(LeaseState::Held)
    }

    async fn approx_depth(&self, queue: &str) -> Result<u64, BackendError> {
        let now = SystemTime::now();
        let state = self.state.lock().unwrap();
        Ok(state
            .rows
            .values()
            .filter(|r| r.queue == queue && r.claimable(now))
            .count() as u64)
    }

    async fn cancel(&self, id: &JobId) -> Result<bool, BackendError> {
        if !self.caps.cancel_pending {
            return Err(BackendError::Unsupported("cancel"));
        }
        let now = SystemTime::now();
        let mut state = self.state.lock().unwrap();
        let Ok(id) = id.as_str().parse::<u64>() else {
            return Ok(false);
        };
        match state.rows.get(&id) {
            Some(r) if !r.leased(now) => {
                state.rows.remove(&id);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn purge(&self, queue: &str) -> Result<u64, BackendError> {
        let mut state = self.state.lock().unwrap();
        let before = state.rows.len();
        state.rows.retain(|_, r| r.queue != queue);
        Ok((before - state.rows.len()) as u64)
    }

    async fn sweep(&self) -> Result<(), BackendError> {
        self.sweeps
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let now = SystemTime::now();
        let mut state = self.state.lock().unwrap();
        state.windows.retain(|_, until| *until > now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(kind: &str) -> NewJob {
        NewJob {
            kind: kind.into(),
            queue: "default".into(),
            payload: b"{}".to_vec(),
            run_at: None,
            max_attempts: 3,
            unique_key: None,
            uniqueness: UniquenessKind::None,
        }
    }

    #[tokio::test]
    async fn a_stale_worker_cannot_settle_a_job_it_lost() {
        let m = Memory::new();
        m.push(vec![job("a")]).await.unwrap();
        let req = ClaimRequest {
            queue: "default".into(),
            max: 1,
            lease: Duration::from_millis(30),
            kinds: None,
            worker: "one".into(),
        };
        let first = m.claim(&req).await.unwrap().pop().unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        let second = m.claim(&req).await.unwrap().pop().unwrap();
        assert_ne!(first.lease.token, second.lease.token);

        // The first worker wakes up and tries to finish.
        let state = m.settle(&first.lease, Settle::Done).await.unwrap();
        assert_eq!(state, LeaseState::Lost);
        assert_eq!(m.len(), 1, "the second worker still owns the job");

        assert_eq!(
            m.settle(&second.lease, Settle::Done).await.unwrap(),
            LeaseState::Held
        );
        assert!(m.is_empty());
    }

    #[tokio::test]
    async fn a_release_does_not_spend_an_attempt() {
        let m = Memory::new();
        m.push(vec![job("a")]).await.unwrap();
        let req = ClaimRequest {
            queue: "default".into(),
            max: 1,
            lease: Duration::from_secs(10),
            kinds: None,
            worker: "old".into(),
        };
        let c = m.claim(&req).await.unwrap().pop().unwrap();
        assert_eq!(c.attempt, 1);
        m.settle(
            &c.lease,
            Settle::Release {
                not_before: SystemTime::now(),
                reason: brocanth_queue::ReleaseReason::UnknownKind,
            },
        )
        .await
        .unwrap();
        let again = m.claim(&req).await.unwrap().pop().unwrap();
        assert_eq!(again.attempt, 1, "the budget must survive a rolling deploy");
    }
}
