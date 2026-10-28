//! The screens, over the map.
//!
//! The same four derived states the Postgres adapter computes, spelled
//! against this backend's own columns. They are not the same predicates:
//! here a lease is `token` plus `lease_until`, and a leased row keeps its
//! `run_at` in the past, where Postgres pushes `available_at` forward and
//! lets one column carry both. That is the point of deriving state per
//! adapter instead of storing it: each one answers from what it already
//! has.
//!
//! What this backend cannot do is history. It deletes a row when the job
//! succeeds and keeps no archive, so
//! [`has_history`](brocanth_queue::Inspect::has_history) is false and
//! [`finished`](brocanth_queue::Inspect::finished) answers `Unsupported`.
//! A panel built over it draws no chart, which is the right thing to show
//! for a backend that keeps nothing.

use std::time::SystemTime;

use brocanth_queue::{
    BackendError, DeadId, DeadRow, Inspect, JobId, JobRow, Page, QueueDepth, Stats, View,
};

use crate::{Dead, Memory, Row};

impl Row {
    /// Which of the four screens this row belongs on.
    ///
    /// `None` for a row whose attempts are spent and which nothing has
    /// moved to the dead letter yet. It is in no state a person acts on.
    fn view(&self, now: SystemTime) -> Option<View> {
        if self.leased(now) {
            return Some(View::Running);
        }
        if self.run_at > now {
            return Some(if self.attempts > 0 {
                View::Retrying
            } else {
                View::Scheduled
            });
        }
        (self.attempts < self.max_attempts).then_some(View::Ready)
    }

    fn to_row(&self) -> JobRow {
        JobRow {
            id: JobId(self.id.to_string()),
            kind: self.kind.clone(),
            queue: self.queue.clone(),
            payload: self.payload.clone(),
            attempt: self.attempts,
            max_attempts: self.max_attempts,
            enqueued_at: self.enqueued_at,
            available_at: self.run_at,
            claimed_by: self.claimed_by.clone(),
            claimed_at: self.claimed_at,
            last_error: self.last_error.clone(),
        }
    }
}

impl Dead {
    fn to_row(&self) -> DeadRow {
        DeadRow {
            id: DeadId(self.id.to_string()),
            job_id: Some(JobId(self.job_id.to_string())),
            kind: self.kind.clone(),
            queue: self.queue.clone(),
            payload: self.payload.clone(),
            attempts: self.attempts,
            max_attempts: Some(self.max_attempts),
            unique_key: self.unique_key.clone(),
            failure: self.failure.to_string(),
            enqueued_at: self.enqueued_at,
            died_at: self.died_at,
        }
    }
}

/// Take `page`'s worth out of an already ordered list.
fn paginate<T>(rows: Vec<T>, page: Page) -> Vec<T> {
    let limit = page.limit.min(Page::MAX_LIMIT) as usize;
    rows.into_iter()
        .skip(page.offset as usize)
        .take(limit)
        .collect()
}

impl Inspect for Memory {
    async fn stats(&self) -> Result<Stats, BackendError> {
        let now = SystemTime::now();
        let state = self.state.lock().unwrap();
        let mut s = Stats {
            dead: state.dead.len() as u64,
            ..Stats::default()
        };
        for r in state.rows.values() {
            match r.view(now) {
                Some(View::Ready) => {
                    s.ready += 1;
                    let age = now.duration_since(r.run_at).unwrap_or_default();
                    s.oldest_ready = Some(s.oldest_ready.map_or(age, |o| o.max(age)));
                }
                Some(View::Running) => s.running += 1,
                Some(View::Retrying) => s.retrying += 1,
                Some(View::Scheduled) => s.scheduled += 1,
                None => {}
            }
        }
        Ok(s)
    }

    async fn depths(&self) -> Result<Vec<QueueDepth>, BackendError> {
        let now = SystemTime::now();
        let state = self.state.lock().unwrap();
        let mut by: std::collections::BTreeMap<String, QueueDepth> = Default::default();
        for r in state.rows.values() {
            let d = by.entry(r.queue.clone()).or_insert_with(|| QueueDepth {
                queue: r.queue.clone(),
                ready: 0,
                running: 0,
                retrying: 0,
                scheduled: 0,
                oldest_ready: None,
            });
            match r.view(now) {
                Some(View::Ready) => {
                    d.ready += 1;
                    let age = now.duration_since(r.run_at).unwrap_or_default();
                    d.oldest_ready = Some(d.oldest_ready.map_or(age, |o| o.max(age)));
                }
                Some(View::Running) => d.running += 1,
                Some(View::Retrying) => d.retrying += 1,
                Some(View::Scheduled) => d.scheduled += 1,
                None => {}
            }
        }
        Ok(by.into_values().collect())
    }

    async fn list(&self, view: View, page: Page) -> Result<Vec<JobRow>, BackendError> {
        let now = SystemTime::now();
        let state = self.state.lock().unwrap();
        let mut rows: Vec<JobRow> = state
            .rows
            .values()
            .filter(|r| r.view(now) == Some(view))
            .map(Row::to_row)
            .collect();
        // The same order the Postgres adapter uses: the row worth reading
        // first is the one that is late, or the one that has been held
        // longest.
        match view {
            View::Running => rows.sort_by_key(|r| (r.claimed_at, r.id.0.clone())),
            _ => rows.sort_by_key(|r| (r.available_at, r.id.0.clone())),
        }
        Ok(paginate(rows, page))
    }

    async fn job(&self, id: &JobId) -> Result<Option<JobRow>, BackendError> {
        let Ok(n) = id.as_str().parse::<u64>() else {
            return Ok(None);
        };
        let state = self.state.lock().unwrap();
        Ok(state.rows.get(&n).map(Row::to_row))
    }

    async fn dead(&self, page: Page) -> Result<Vec<DeadRow>, BackendError> {
        let state = self.state.lock().unwrap();
        let mut rows: Vec<DeadRow> = state.dead.iter().map(Dead::to_row).collect();
        rows.sort_by(|a, b| b.died_at.cmp(&a.died_at).then(b.id.cmp(&a.id)));
        Ok(paginate(rows, page))
    }

    async fn dead_one(&self, id: &DeadId) -> Result<Option<DeadRow>, BackendError> {
        let Ok(n) = id.as_str().parse::<u64>() else {
            return Ok(None);
        };
        let state = self.state.lock().unwrap();
        Ok(state.dead.iter().find(|d| d.id == n).map(Dead::to_row))
    }

    async fn run_now(&self, id: &JobId) -> Result<bool, BackendError> {
        let Ok(n) = id.as_str().parse::<u64>() else {
            return Ok(false);
        };
        let now = SystemTime::now();
        let mut state = self.state.lock().unwrap();
        // A held lease is refused. Clearing one is how two workers come to
        // run the same job, which is the thing the lease exists to stop.
        match state.rows.get_mut(&n) {
            Some(r) if !r.leased(now) => {
                r.run_at = now;
                r.token = None;
                r.claimed_by = None;
                r.claimed_at = None;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn discard(&self, id: &JobId) -> Result<bool, BackendError> {
        let Ok(n) = id.as_str().parse::<u64>() else {
            return Ok(false);
        };
        let now = SystemTime::now();
        let mut state = self.state.lock().unwrap();
        match state.rows.get(&n) {
            Some(r) if !r.leased(now) => {
                state.rows.remove(&n);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn requeue(&self, id: &DeadId) -> Result<Option<JobId>, BackendError> {
        let Ok(n) = id.as_str().parse::<u64>() else {
            return Ok(None);
        };
        let now = SystemTime::now();
        let mut state = self.state.lock().unwrap();
        let Some(at) = state.dead.iter().position(|d| d.id == n) else {
            return Ok(None);
        };
        // A key a live job still holds means the copy the operator asked
        // for already exists. Nothing is written and the corpse stays, so
        // the record of why this one died is not lost to a button press.
        if let Some(key) = state.dead[at].unique_key.clone() {
            if state
                .rows
                .values()
                .any(|r| r.unique_key.as_deref() == Some(key.as_str()))
            {
                return Ok(None);
            }
        }
        let d = state.dead.remove(at);
        state.seq += 1;
        let new_id = state.seq;
        state.rows.insert(
            new_id,
            Row {
                id: new_id,
                kind: d.kind,
                queue: d.queue,
                payload: d.payload,
                run_at: now,
                attempts: 0,
                max_attempts: d.max_attempts,
                token: None,
                lease_until: now,
                unique_key: d.unique_key,
                enqueued_at: now,
                claimed_by: None,
                claimed_at: None,
                last_error: None,
            },
        );
        Ok(Some(JobId(new_id.to_string())))
    }

    async fn forget(&self, id: &DeadId) -> Result<bool, BackendError> {
        let Ok(n) = id.as_str().parse::<u64>() else {
            return Ok(false);
        };
        let mut state = self.state.lock().unwrap();
        let before = state.dead.len();
        state.dead.retain(|d| d.id != n);
        Ok(state.dead.len() < before)
    }
}
