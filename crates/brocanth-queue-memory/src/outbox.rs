//! An in-process outbox, with the same dead letter as the Postgres one.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use brocanth_queue::{
    DeadId, DeadRow, NewJob, Outbox, OutboxError, OutboxInspect, OutboxRow, Page,
};

#[derive(Debug, Clone)]
struct Row {
    job: NewJob,
    attempts: u32,
    available_at: SystemTime,
    enqueued_at: SystemTime,
    last_error: Option<String>,
}

#[derive(Debug, Clone)]
struct Dead {
    id: u64,
    job: NewJob,
    attempts: u32,
    failure: String,
    enqueued_at: SystemTime,
    died_at: SystemTime,
}

impl Dead {
    fn row(&self) -> DeadRow {
        DeadRow {
            id: DeadId(self.id.to_string()),
            job_id: None,
            kind: self.job.kind.clone(),
            queue: self.job.queue.clone(),
            payload: self.job.payload.clone(),
            attempts: self.attempts,
            max_attempts: Some(self.job.max_attempts),
            unique_key: self.job.unique_key.clone(),
            failure: self.failure.clone(),
            enqueued_at: self.enqueued_at,
            died_at: self.died_at,
        }
    }
}

#[derive(Default)]
struct State {
    seq: u64,
    dead_seq: u64,
    rows: BTreeMap<u64, Row>,
    dead: Vec<Dead>,
}

/// An outbox held in memory.
///
/// For a test, and for the conformance suite, which checks it against the
/// same cases as the Postgres outbox. There is no transaction to join, so
/// [`write`](MemoryOutbox::write) stands in for the caller's commit.
#[derive(Default)]
pub struct MemoryOutbox {
    state: Mutex<State>,
}

impl std::fmt::Debug for MemoryOutbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryOutbox").finish_non_exhaustive()
    }
}

impl MemoryOutbox {
    /// An empty outbox.
    pub fn new() -> Self {
        MemoryOutbox::default()
    }

    /// Add a row, as a committed transaction would. Returns its id.
    pub fn write(&self, job: NewJob) -> String {
        let now = SystemTime::now();
        let mut state = self.state.lock().unwrap();
        state.seq += 1;
        let id = state.seq;
        state.rows.insert(
            id,
            Row {
                job,
                attempts: 0,
                available_at: now,
                enqueued_at: now,
                last_error: None,
            },
        );
        id.to_string()
    }

    /// The last delivery error recorded on a live row.
    pub fn last_error(&self, id: &str) -> Option<String> {
        let n = id.parse::<u64>().ok()?;
        self.state
            .lock()
            .unwrap()
            .rows
            .get(&n)
            .and_then(|r| r.last_error.clone())
    }

    fn put_back(&self, ids: &[String], not_before: Duration, error: &str, refund: bool) {
        let at = SystemTime::now() + not_before;
        let mut state = self.state.lock().unwrap();
        for id in ids.iter().filter_map(|s| s.parse::<u64>().ok()) {
            if let Some(r) = state.rows.get_mut(&id) {
                r.available_at = at;
                r.last_error = Some(error.to_string());
                if refund {
                    r.attempts = r.attempts.saturating_sub(1);
                }
            }
        }
    }
}

impl Outbox for MemoryOutbox {
    async fn fetch(&self, limit: usize, lease: Duration) -> Result<Vec<OutboxRow>, OutboxError> {
        let now = SystemTime::now();
        let mut state = self.state.lock().unwrap();
        let mut out = Vec::new();
        for (id, r) in state.rows.iter_mut() {
            if out.len() >= limit {
                break;
            }
            if r.available_at > now {
                continue;
            }
            // The lease is the same column as the backoff, as in Postgres:
            // a relay that dies leaves a row that comes back on its own.
            r.available_at = now + lease;
            r.attempts += 1;
            out.push(OutboxRow {
                id: id.to_string(),
                job: r.job.clone(),
                attempts: r.attempts,
            });
        }
        Ok(out)
    }

    async fn delete(&self, ids: &[String]) -> Result<(), OutboxError> {
        let mut state = self.state.lock().unwrap();
        for id in ids.iter().filter_map(|s| s.parse::<u64>().ok()) {
            state.rows.remove(&id);
        }
        Ok(())
    }

    async fn defer(
        &self,
        ids: &[String],
        not_before: Duration,
        error: &str,
    ) -> Result<(), OutboxError> {
        self.put_back(ids, not_before, error, false);
        Ok(())
    }

    async fn release(
        &self,
        ids: &[String],
        not_before: Duration,
        error: &str,
    ) -> Result<(), OutboxError> {
        self.put_back(ids, not_before, error, true);
        Ok(())
    }

    async fn bury(&self, id: &str, failure: &str) -> Result<(), OutboxError> {
        let Ok(n) = id.parse::<u64>() else {
            return Ok(());
        };
        let mut state = self.state.lock().unwrap();
        if let Some(r) = state.rows.remove(&n) {
            state.dead_seq += 1;
            let dead_id = state.dead_seq;
            state.dead.push(Dead {
                id: dead_id,
                job: r.job,
                attempts: r.attempts,
                failure: failure.to_string(),
                enqueued_at: r.enqueued_at,
                died_at: SystemTime::now(),
            });
        }
        Ok(())
    }

    async fn pending(&self) -> Result<u64, OutboxError> {
        Ok(self.state.lock().unwrap().rows.len() as u64)
    }
}

impl OutboxInspect for MemoryOutbox {
    async fn dead(&self, page: Page) -> Result<Vec<DeadRow>, OutboxError> {
        let state = self.state.lock().unwrap();
        Ok(state
            .dead
            .iter()
            .rev()
            .skip(page.offset as usize)
            .take(page.limit.min(Page::MAX_LIMIT) as usize)
            .map(Dead::row)
            .collect())
    }

    async fn dead_one(&self, id: &DeadId) -> Result<Option<DeadRow>, OutboxError> {
        let Ok(n) = id.as_str().parse::<u64>() else {
            return Ok(None);
        };
        let state = self.state.lock().unwrap();
        Ok(state.dead.iter().find(|d| d.id == n).map(Dead::row))
    }

    async fn requeue(&self, id: &DeadId) -> Result<Option<String>, OutboxError> {
        let Ok(n) = id.as_str().parse::<u64>() else {
            return Ok(None);
        };
        let job = {
            let mut state = self.state.lock().unwrap();
            let Some(at) = state.dead.iter().position(|d| d.id == n) else {
                return Ok(None);
            };
            state.dead.remove(at).job
        };
        Ok(Some(self.write(job)))
    }

    async fn forget(&self, id: &DeadId) -> Result<bool, OutboxError> {
        let Ok(n) = id.as_str().parse::<u64>() else {
            return Ok(false);
        };
        let mut state = self.state.lock().unwrap();
        let before = state.dead.len();
        state.dead.retain(|d| d.id != n);
        Ok(state.dead.len() < before)
    }
}
