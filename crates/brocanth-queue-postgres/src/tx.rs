//! The write side of the transaction: the one atomic enqueue.

use brocanth_queue::{EnqueueError, JobId, Lane, NewJob, Pushed, TxEnqueue, DEFAULT_MAX_PENDING};
use sqlx::{Postgres, Row, Transaction};

use crate::uniqueness_text;

/// A delay in seconds, from the caller's own clock.
///
/// The server turns it back into an instant with its own `now()`. See
/// `delay_secs` in the crate root: nothing here ever writes a timestamp the
/// client computed, because the two clocks are not the same clock.
fn delay_secs(at: Option<std::time::SystemTime>) -> f64 {
    at.and_then(|t| t.duration_since(std::time::SystemTime::now()).ok())
        .unwrap_or(std::time::Duration::ZERO)
        .as_secs_f64()
}

/// A transaction that can hold a job.
///
/// It borrows the caller's transaction rather than owning one. The framework
/// does not get to decide what an application's unit of work is; it only
/// gets to add a row to it.
///
/// ```ignore
/// let mut tx = pool.begin().await?;
/// orders::insert(&mut tx, &order).await?;
/// client.enqueue(ChargeCard { order: order.id })
///       .in_tx(&mut Tx::new(&mut tx))
///       .await?;
/// tx.commit().await?;   // the order and the job commit together
/// ```
pub struct Tx<'a, 'c> {
    tx: &'a mut Transaction<'c, Postgres>,
    max_pending: Option<u64>,
}

impl<'a, 'c> Tx<'a, 'c> {
    /// Wrap a transaction.
    ///
    /// The outbox limit starts at [`DEFAULT_MAX_PENDING`] rather than at
    /// nothing. An unbounded outbox is not a safer default: when the bulk
    /// backend is unreachable the table grows at the rate the application
    /// writes, silently, and the outage is found when the disk fills. The
    /// limit turns that into a failure at the write that caused it. Use
    /// [`Tx::unbounded`] for an application that would rather keep serving.
    pub fn new(tx: &'a mut Transaction<'c, Postgres>) -> Self {
        Tx {
            tx,
            max_pending: Some(DEFAULT_MAX_PENDING),
        }
    }

    /// Accept a bulk enqueue however far behind the relay is.
    ///
    /// This is [`OnOutboxFull::Accept`](brocanth_queue::OnOutboxFull), spelled
    /// at the call site.
    pub fn unbounded(tx: &'a mut Transaction<'c, Postgres>) -> Self {
        Tx {
            tx,
            max_pending: None,
        }
    }

    /// Refuse a bulk enqueue once the outbox holds this many undelivered
    /// rows.
    ///
    /// The count is bounded, not a full `count(*)`: the query stops at the
    /// limit, so the check costs the same whether the outbox holds a
    /// thousand rows or ten million. A check that gets slower as the
    /// backlog grows is a check that fails when it matters.
    pub fn max_pending(mut self, rows: u64) -> Self {
        self.max_pending = Some(rows);
        self
    }

    async fn outbox_is_full(&mut self, limit: u64) -> Result<Option<u64>, EnqueueError> {
        let row = sqlx::query(
            "SELECT count(*) AS n FROM (SELECT 1 FROM queue_outbox LIMIT $1) AS capped",
        )
        .bind(limit as i64)
        .fetch_one(&mut **self.tx)
        .await
        .map_err(|e| EnqueueError::Store(e.to_string()))?;
        let n = row.get::<i64, _>("n") as u64;
        Ok((n >= limit).then_some(n))
    }
}

impl TxEnqueue for Tx<'_, '_> {
    async fn enqueue_in_tx(&mut self, lane: Lane, job: NewJob) -> Result<Pushed, EnqueueError> {
        match lane {
            Lane::Transactional => {
                let row = sqlx::query(
                    "INSERT INTO queue_tx \
                     (kind, queue, payload, available_at, max_attempts, unique_key, uniqueness) \
                     VALUES ($1, $2, $3, now() + make_interval(secs => $4), $5, $6, $7) \
                     ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING \
                     RETURNING id",
                )
                .bind(&job.kind)
                .bind(&job.queue)
                .bind(&job.payload)
                .bind(delay_secs(job.run_at))
                .bind(job.max_attempts as i32)
                .bind(job.unique_key.as_deref())
                .bind(uniqueness_text(job.uniqueness))
                .fetch_optional(&mut **self.tx)
                .await
                .map_err(|e| EnqueueError::Store(e.to_string()))?;
                Ok(match row {
                    Some(r) => Pushed::Enqueued(JobId(r.get::<i64, _>("id").to_string())),
                    None => Pushed::Duplicate,
                })
            }
            Lane::Bulk => {
                // A bulk job enqueued inside a transaction is the outbox
                // case, and it is the proof that the lane and the call site
                // are different axes.
                if let Some(limit) = self.max_pending {
                    if let Some(rows) = self.outbox_is_full(limit).await? {
                        return Err(EnqueueError::OutboxFull { rows, limit });
                    }
                }
                let row = sqlx::query(
                    "INSERT INTO queue_outbox \
                     (kind, queue, payload, run_at, max_attempts, unique_key, uniqueness) \
                     VALUES ($1, $2, $3, \
                             CASE WHEN $4 = 0 THEN NULL \
                                  ELSE now() + make_interval(secs => $4) END, \
                             $5, $6, $7) RETURNING id",
                )
                .bind(&job.kind)
                .bind(&job.queue)
                .bind(&job.payload)
                .bind(delay_secs(job.run_at))
                .bind(job.max_attempts as i32)
                .bind(job.unique_key.as_deref())
                .bind(uniqueness_text(job.uniqueness))
                .fetch_one(&mut **self.tx)
                .await
                .map_err(|e| EnqueueError::Store(e.to_string()))?;
                Ok(Pushed::Enqueued(JobId(row.get::<i64, _>("id").to_string())))
            }
        }
    }
}
