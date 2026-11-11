//! This queue as `brocanth_queue::Inspect`, so the admin screens can read it
//! without knowing which queue they have.
//!
//! The read side is the engine's: every call goes to the backend's own
//! `Inspect`. The states are the engine's four, derived from the lease and
//! not stored (`brocanth-queue/docs/DECISIONS.md` §3.2), and the history
//! screen has data on Postgres, whose archive keeps seven days of finished
//! jobs.

use std::time::SystemTime;

use brocanth_queue::{
    BackendError, Bucket, DeadId, DeadRow, Finished, Inspect, JobId, JobRow, Page, QueueDepth,
    Stats, View,
};

use crate::queue::Queue;

impl Inspect for Queue {
    fn has_history(&self) -> bool {
        self.reader().has_history()
    }

    async fn stats(&self) -> Result<Stats, BackendError> {
        self.reader().stats().await
    }

    async fn depths(&self) -> Result<Vec<QueueDepth>, BackendError> {
        self.reader().depths().await
    }

    async fn list(&self, view: View, page: Page) -> Result<Vec<JobRow>, BackendError> {
        self.reader().list(view, page).await
    }

    async fn job(&self, id: &JobId) -> Result<Option<JobRow>, BackendError> {
        self.reader().job(id).await
    }

    async fn dead(&self, page: Page) -> Result<Vec<DeadRow>, BackendError> {
        self.reader().dead(page).await
    }

    async fn dead_one(&self, id: &DeadId) -> Result<Option<DeadRow>, BackendError> {
        self.reader().dead_one(id).await
    }

    async fn run_now(&self, id: &JobId) -> Result<bool, BackendError> {
        self.reader().run_now(id).await
    }

    async fn discard(&self, id: &JobId) -> Result<bool, BackendError> {
        self.reader().discard(id).await
    }

    async fn requeue(&self, id: &DeadId) -> Result<Option<JobId>, BackendError> {
        self.reader().requeue(id).await
    }

    async fn forget(&self, id: &DeadId) -> Result<bool, BackendError> {
        self.reader().forget(id).await
    }

    async fn finished(
        &self,
        since: SystemTime,
        bucket: Bucket,
    ) -> Result<Vec<Finished>, BackendError> {
        self.reader().finished(since, bucket).await
    }
}
