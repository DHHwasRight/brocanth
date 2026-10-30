//! A job runner whose storage is a choice, not an assumption.
//!
//! An application starts with its jobs in the database it already has, and
//! moves the high-volume ones to a dedicated backend when the write churn
//! says so. Nothing in the application changes. That only works if the
//! contract is written against the weakest backend it intends to support,
//! so this crate is specified against SQS and not against Postgres.
//!
//! # Two lanes
//!
//! [`Lane::Transactional`] is for work whose loss is a correctness bug. It
//! commits with the data it refers to, it lives in the application's own
//! database, and it never moves. It is not served by [`Backend`] at all,
//! because "join the caller's transaction" cannot be expressed in a `dyn`
//! signature that an SQS adapter could implement.
//!
//! [`Lane::Bulk`] is for fire-and-forget work that is idempotent by
//! contract. It is the only lane a [`Backend`] serves and the only one that
//! migrates.
//!
//! There is no third lane for scheduled work. A delay is
//! [`Enqueue::after`], available on both lanes, because "cancel this order
//! in thirty minutes unless it is paid" is both atomic and delayed, and a
//! lane split would make that unspellable. Storage still separates them: a
//! backend keeps a delayed row wherever it wants, which is what the
//! Postgres adapter's second table is for.
//!
//! # The write side
//!
//! ```ignore
//! client.enqueue(SendWelcome { user }).await?;                 // bulk, now
//! client.enqueue(Digest).after(Duration::from_secs(3600)).await?;
//! client.enqueue(ChargeCard { order }).in_tx(&mut tx).await?;  // atomic
//! client.enqueue(SendReceipt { order }).in_tx(&mut tx).await?; // via outbox
//! ```
//!
//! The lane picks the destination and the call picks the atomicity. They are
//! different axes: the fourth line is a `Bulk` job enqueued atomically, and
//! the [`outbox`] is exactly that case.
//!
//! # The read side
//!
//! A [`Worker`] claims a lease, runs the job, extends the lease while it
//! works and settles it. Every call that changes a job carries a fence
//! token, so a worker that stalls past its lease cannot delete the row
//! another worker now owns.
//!
//! # The screens
//!
//! [`Backend`] has no method that reads, because SQS cannot list its own
//! messages. The screens an operator needs — what is waiting, what is
//! running and on which worker, what failed, what gave up — are
//! [`Inspect`], a separate trait that an adapter implements only if it can.
//! An application whose backend cannot serve it has no queue panel, rather
//! than a panel that lies.

#![forbid(unsafe_code)]

pub mod backend;
mod enqueue;
pub mod inspect;
pub mod job;
pub mod outbox;
pub mod registry;
mod worker;

pub use backend::{
    Backend, BackendError, BoxFuture, Capabilities, ClaimRequest, Claimed, DynBackend, LeaseRef,
    LeaseState, NewJob, Pushed, ReleaseReason, Settle, UniquenessKind, UniquenessSupport,
};
pub use enqueue::{Client, Enqueue, EnqueueError, TxEnqueue, TxJobId};
pub use inspect::{
    Bucket, DeadId, DeadRow, DynInspect, Finished, Inspect, JobRow, Page, QueueDepth, Stats, View,
};
pub use job::{Backoff, FailureKind, Job, JobContext, JobExt, JobFailure, JobId, Lane, Uniqueness};
pub use outbox::{
    DynOutboxInspect, OnOutboxFull, Outbox, OutboxError, OutboxInspect, OutboxRow, Relay,
    RelayConfig, Tick, DEFAULT_MAX_PENDING,
};
pub use registry::{Demands, DuplicateKind, Registry, Unmet};
pub use worker::{
    Budget, ConfigError, Observer, PoolTooSmall, Silent, StartError, Worker, WorkerConfig,
};
