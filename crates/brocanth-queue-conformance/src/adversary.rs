//! A backend wrapper that makes every legal choice a Postgres backend never
//! makes.
//!
//! Each hostility is permitted by the contract:
//!
//! * `claim` may return fewer jobs than asked for, including none.
//! * Delivery order is not promised, so a batch may come back reversed.
//! * A job is not promised to be visible to the next claim, so the first
//!   claim after a push may be empty.
//! * `approx_depth` is approximate unless the backend says otherwise.
//!
//! A test that passes against the bare backend and fails through this
//! wrapper is a test that encodes a Postgres assumption. The suite runs
//! twice and requires the same result, so the assumption is found here and
//! not when the second backend arrives.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use brocanth_queue::{
    BackendError, BoxFuture, Capabilities, ClaimRequest, Claimed, DynBackend, JobId, LeaseRef,
    LeaseState, NewJob, Pushed, Settle,
};

/// Wraps a backend and uses every freedom the contract gives it.
pub struct Adversary {
    inner: Arc<dyn DynBackend>,
    hold_next_claim: AtomicBool,
    claims: AtomicU64,
}

impl std::fmt::Debug for Adversary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Adversary")
            .field("inner", &self.inner.name())
            .finish()
    }
}

impl Adversary {
    /// Wrap a backend. Returns the wrapper already boxed, because it is
    /// only ever used where a `DynBackend` is wanted.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(inner: Arc<dyn DynBackend>) -> Arc<dyn DynBackend> {
        Arc::new(Adversary {
            inner,
            hold_next_claim: AtomicBool::new(false),
            claims: AtomicU64::new(0),
        })
    }
}

impl DynBackend for Adversary {
    fn name(&self) -> &'static str {
        "adversary"
    }

    fn capabilities(&self) -> Capabilities {
        self.inner.capabilities()
    }

    fn push(&self, batch: Vec<NewJob>) -> BoxFuture<'_, Result<Vec<Pushed>, BackendError>> {
        self.hold_next_claim.store(true, Ordering::SeqCst);
        self.inner.push(batch)
    }

    fn claim<'a>(
        &'a self,
        req: &'a ClaimRequest,
    ) -> BoxFuture<'a, Result<Vec<Claimed>, BackendError>> {
        Box::pin(async move {
            // A push does not make a job visible to the very next claim.
            if self.hold_next_claim.swap(false, Ordering::SeqCst) {
                return Ok(Vec::new());
            }
            // Half the batch size the caller asked for, and never more than
            // two, so a caller that claims once and expects everything
            // fails here.
            let trimmed = ClaimRequest {
                max: req.max.min(2),
                ..req.clone()
            };
            let mut out = self.inner.claim(&trimmed).await?;
            // Nothing promises order.
            if self.claims.fetch_add(1, Ordering::SeqCst).is_multiple_of(2) {
                out.reverse();
            }
            Ok(out)
        })
    }

    fn heartbeat<'a>(
        &'a self,
        lease: &'a LeaseRef,
        extend: Duration,
    ) -> BoxFuture<'a, Result<LeaseState, BackendError>> {
        self.inner.heartbeat(lease, extend)
    }

    fn settle<'a>(
        &'a self,
        lease: &'a LeaseRef,
        outcome: Settle,
    ) -> BoxFuture<'a, Result<LeaseState, BackendError>> {
        self.inner.settle(lease, outcome)
    }

    fn approx_depth<'a>(&'a self, queue: &'a str) -> BoxFuture<'a, Result<u64, BackendError>> {
        Box::pin(async move {
            let n = self.inner.approx_depth(queue).await?;
            // Approximate means approximate. A test that asserts an exact
            // count without checking `exact_depth` fails here.
            if self.inner.capabilities().exact_depth {
                Ok(n)
            } else {
                Ok(n.saturating_add(1))
            }
        })
    }

    fn cancel<'a>(&'a self, id: &'a JobId) -> BoxFuture<'a, Result<bool, BackendError>> {
        self.inner.cancel(id)
    }

    fn purge<'a>(&'a self, queue: &'a str) -> BoxFuture<'a, Result<u64, BackendError>> {
        self.inner.purge(queue)
    }

    fn sweep(&self) -> BoxFuture<'_, Result<(), BackendError>> {
        self.inner.sweep()
    }
}
