//! This queue, through the contract the admin screens read.
//!
//! [`brocanth_queue::Inspect`] is the read side every queue in the
//! workspace answers, so one panel serves all of them. The panel never
//! learns which queue it is looking at, which is the whole point: an
//! application that moves its jobs to `brocanth-queue` keeps its screens.
//!
//! # Where the two queues disagree
//!
//! This store has a `state` column, `'pending'` or `'running'`, and
//! `brocanth-queue` deliberately has none — there a state is a predicate
//! over `available_at` and `lease_token`. Both end up at the same four
//! views, which is why the trait is written in views and not in columns.
//!
//! Two things are therefore shaped a little differently here:
//!
//! * **Ids.** A job id and a dead-letter id are both `i64` in this store,
//!   and the trait keeps them in separate types so one cannot be passed
//!   where the other belongs. They are printed and parsed at this boundary.
//! * **Clocks.** Everything here is epoch milliseconds from the queue's own
//!   [`Clock`](crate::Clock), which a test can move by hand. The trait
//!   speaks `SystemTime`, so the conversion happens here and the screens
//!   never see a millisecond.
//!
//! # No history
//!
//! This store deletes a job's row when it succeeds, so
//! [`has_history`](brocanth_queue::Inspect::has_history) is false and the
//! panel draws no chart. That is not an oversight to fix here: drawing one
//! means keeping a record of every success, which is a decision about write
//! volume and not about a screen.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brocanth_queue::{
    BackendError, DeadId, DeadRow, Inspect, JobId as QueueJobId, JobRow, Page, QueueDepth, Stats,
    View,
};

use crate::job::JobId;
use crate::queue::Queue;
use crate::store::{DeadJob, RunningJob, WaitingJob};

/// Epoch milliseconds as an instant.
///
/// Before the epoch is not a time this store can hold, and clamping is
/// what the rest of the crate already does with one.
fn at(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64)
}

/// The queue's own error, as the one the trait speaks.
///
/// Every arm is [`Other`](BackendError::Other) rather than
/// [`Unavailable`](BackendError::Unavailable), because the distinction the
/// trait draws is "would retrying the same call help", and nothing here
/// retries: a screen that cannot read says so to the person reading it.
fn failed(e: crate::Error) -> BackendError {
    BackendError::Other(e.to_string())
}

fn waiting_row(j: &WaitingJob) -> JobRow {
    JobRow {
        id: QueueJobId(j.id.to_string()),
        kind: j.job_name.clone(),
        queue: j.queue.clone(),
        payload: j.payload.clone().into_bytes(),
        attempt: j.attempts.max(0) as u32,
        max_attempts: j.max_attempts.max(0) as u32,
        enqueued_at: at(j.created_at),
        available_at: at(j.run_at),
        claimed_by: None,
        claimed_at: None,
        last_error: j.last_error.clone(),
    }
}

fn running_row(j: &RunningJob) -> JobRow {
    JobRow {
        id: QueueJobId(j.id.to_string()),
        kind: j.job_name.clone(),
        queue: j.queue.clone(),
        payload: j.payload.clone().into_bytes(),
        attempt: j.attempts.max(0) as u32,
        max_attempts: j.max_attempts.max(0) as u32,
        // This store has no `created_at` on the running query, and the
        // claim time is the one a screen shows. Both come from the row, so
        // neither is invented here.
        enqueued_at: j.locked_at.map_or(UNIX_EPOCH, at),
        available_at: j.locked_at.map_or(UNIX_EPOCH, at),
        claimed_by: j.locked_by.clone(),
        claimed_at: j.locked_at.map(at),
        last_error: None,
    }
}

fn dead_row(d: &DeadJob) -> DeadRow {
    DeadRow {
        id: DeadId(d.id.to_string()),
        job_id: Some(QueueJobId(d.job_id.to_string())),
        kind: d.job_name.clone(),
        queue: d.queue.clone(),
        payload: d.payload.clone().into_bytes(),
        attempts: d.attempts.max(0) as u32,
        max_attempts: Some(d.max_attempts.max(0) as u32),
        unique_key: d.unique_key.clone(),
        // The failure kind and the backtrace are this store's own columns
        // and the trait has no field for either. They are folded into the
        // text rather than dropped: a dead letter that does not say it
        // panicked is a dead letter somebody misreads.
        failure: match &d.backtrace {
            Some(bt) => format!("{}: {}\n{bt}", d.failure_kind, d.error),
            None => format!("{}: {}", d.failure_kind, d.error),
        },
        enqueued_at: at(d.enqueued_at),
        died_at: at(d.failed_at),
    }
}

fn parse(id: &QueueJobId) -> Option<i64> {
    id.as_str().parse().ok()
}

impl Inspect for Queue {
    async fn stats(&self) -> Result<Stats, BackendError> {
        let s = Queue::stats(self).await.map_err(failed)?;
        Ok(Stats {
            ready: s.ready,
            running: s.running,
            retrying: s.retrying,
            scheduled: s.scheduled,
            dead: s.dead,
            oldest_ready: s.oldest_ready_age,
        })
    }

    async fn depths(&self) -> Result<Vec<QueueDepth>, BackendError> {
        let s = Queue::stats(self).await.map_err(failed)?;
        Ok(s.queues
            .iter()
            .map(|q| QueueDepth {
                queue: q.queue.clone(),
                ready: q.ready,
                running: q.running,
                retrying: q.retrying,
                scheduled: q.scheduled,
                oldest_ready: q.oldest_ready_age,
            })
            .collect())
    }

    async fn list(&self, view: View, page: Page) -> Result<Vec<JobRow>, BackendError> {
        let limit = page.limit.min(Page::MAX_LIMIT);
        let offset = page.offset as u32;
        Ok(match view {
            View::Ready => Queue::ready(self, limit, offset)
                .await
                .map_err(failed)?
                .iter()
                .map(waiting_row)
                .collect(),
            View::Running => Queue::busy(self, limit, offset)
                .await
                .map_err(failed)?
                .iter()
                .map(running_row)
                .collect(),
            View::Retrying => Queue::retries(self, limit, offset)
                .await
                .map_err(failed)?
                .iter()
                .map(waiting_row)
                .collect(),
            View::Scheduled => Queue::scheduled(self, limit, offset)
                .await
                .map_err(failed)?
                .iter()
                .map(waiting_row)
                .collect(),
        })
    }

    async fn job(&self, id: &QueueJobId) -> Result<Option<JobRow>, BackendError> {
        let Some(n) = parse(id) else {
            return Ok(None);
        };
        if let Some(w) = Queue::waiting_job(self, JobId(n)).await.map_err(failed)? {
            return Ok(Some(waiting_row(&w)));
        }
        Ok(Queue::running_job(self, JobId(n))
            .await
            .map_err(failed)?
            .as_ref()
            .map(running_row))
    }

    async fn dead(&self, page: Page) -> Result<Vec<DeadRow>, BackendError> {
        Ok(
            Queue::dead_jobs(self, page.limit.min(Page::MAX_LIMIT), page.offset as u32)
                .await
                .map_err(failed)?
                .iter()
                .map(dead_row)
                .collect(),
        )
    }

    async fn dead_one(&self, id: &DeadId) -> Result<Option<DeadRow>, BackendError> {
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(None);
        };
        Ok(Queue::dead_job(self, n)
            .await
            .map_err(failed)?
            .as_ref()
            .map(dead_row))
    }

    async fn run_now(&self, id: &QueueJobId) -> Result<bool, BackendError> {
        let Some(n) = parse(id) else {
            return Ok(false);
        };
        // The store scopes this to `state = 'pending'`, so a job a worker
        // holds is refused here exactly as it is on the other queue.
        Queue::run_now(self, JobId(n)).await.map_err(failed)
    }

    async fn discard(&self, id: &QueueJobId) -> Result<bool, BackendError> {
        let Some(n) = parse(id) else {
            return Ok(false);
        };
        Queue::discard(self, JobId(n)).await.map_err(failed)
    }

    async fn requeue(&self, id: &DeadId) -> Result<Option<QueueJobId>, BackendError> {
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(None);
        };
        Ok(Queue::requeue_dead(self, n)
            .await
            .map_err(failed)?
            .map(|j| QueueJobId(j.0.to_string())))
    }

    async fn forget(&self, id: &DeadId) -> Result<bool, BackendError> {
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(false);
        };
        Queue::discard_dead(self, n).await.map_err(failed)
    }
}
