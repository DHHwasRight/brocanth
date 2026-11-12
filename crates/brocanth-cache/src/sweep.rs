//! Reclaiming the space expired entries take, and the argument about where
//! the thing that drives it belongs.
//!
//! # The sweep is about disk, never about correctness
//!
//! Say this first, because it is the part people get wrong. An entry is dead
//! the moment it expires: every read carries `AND expires_at > $now`, so an
//! expired row is a miss whether or not anything ever deletes it. **A cache
//! that is never swept is still correct.** It is only large.
//!
//! That ordering is deliberate. A TTL that is enforced by a sweeper is a TTL
//! that lies whenever the sweeper is wedged, misconfigured or not deployed —
//! and the lie is "here is a stale value", served confidently, which is the
//! worst failure a cache has. Lazy expiry on read has no moving parts and
//! cannot be switched off.
//!
//! So the sweep exists for one reason: expired rows occupy pages, pages are
//! `shared_buffers`, and `shared_buffers` is what the rest of your database is
//! using. Run it, but do not think of it as the thing that makes the TTL true.
//!
//! # Who drives it: the plugin, by default
//!
//! An earlier version of this crate shipped [`sweep`] and nothing that
//! called it, and left the schedule to `brocanth-jobs`. Nothing wired it, so
//! every application with a cache kept every expired row for ever. The
//! schedule is now [`Sweeper`]: one task per process, started by
//! [`CachePlugin`](crate::CachePlugin) at `start` and stopped at `shutdown`.
//! The generated `cache` plugin starts the same task. The reasons, in order:
//!
//! 1. **It must work in every application that caches.** A cache does not
//!    need a queue, and most applications that cache have none. A schedule
//!    that exists only with `brocanth-jobs` is a schedule that most
//!    applications do not have.
//! 2. **The arrow stays the right way.** The task is a Tokio timer, which the
//!    crate already links for `fetch`. It depends on nothing else.
//! 3. **It is cheap enough to run everywhere.** A sweep is a sequential scan
//!    (`expires_at` has no index, on purpose: see
//!    [`PgCache::schema_sql`](crate::PgCache::schema_sql)), every few minutes,
//!    off the request path. Its cost follows the size of a cache that the
//!    sweep keeps bounded.
//!
//! Every instance of an application runs its own task, and the deletes do
//! not mind: two sweeps at once remove each row once. Each wait is the
//! interval with a random quarter either way, so instances that started
//! together do not sweep together.
//!
//! An application that schedules the sweep another way — a recurring job, or
//! `pg_cron` below — turns the task off with
//! [`CacheConfig::sweep_every`](crate::CacheConfig::sweep_every)`(Duration::ZERO)`.
//!
//! The operation is on the trait
//! ([`Cache::purge_expired`](crate::Cache::purge_expired)) rather than on the
//! Postgres type, so any other backend gets the same arrangement for free.
//!
//! # Other ways to drive it
//!
//! **With `pg_cron`, if the deployment already has it.** One line of SQL and
//! no application involvement at all:
//!
//! ```sql
//! SELECT cron.schedule('sweep-brocanth-cache', '*/5 * * * *',
//!   $$DELETE FROM brocanth_cache
//!     WHERE ctid IN (SELECT ctid FROM brocanth_cache
//!                    WHERE expires_at <= (extract(epoch from now()) * 1000)::bigint
//!                    LIMIT 10000)$$);
//! ```
//!
//! **By hand**, from a job or a command: call [`sweep`].
//!
//! # What this does not do: `VACUUM`
//!
//! Deleting an expired row does not return its space to the operating system;
//! it makes a dead tuple that autovacuum will reclaim for reuse. This crate
//! does not run `VACUUM` and should not: a framework that issues maintenance
//! commands against somebody's database is a framework making operational
//! decisions it cannot see the consequences of. The DDL in
//! [`PgCache::schema_sql`](crate::PgCache::schema_sql) instead sets per-table
//! autovacuum thresholds low, so the database's own autovacuum keeps up with a
//! table that churns far faster than a normal one.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::time::Duration;

use crate::cache::Cache;
use crate::error::{CacheError, Result};

/// How often [`Sweeper`] sweeps when nothing says otherwise: five minutes.
pub const DEFAULT_SWEEP_EVERY: Duration = Duration::from_secs(5 * 60);

/// Rows one `DELETE` of a scheduled sweep removes at most.
pub const SWEEP_BATCH: u64 = 10_000;

/// Batches one scheduled sweep runs at most, so one sweep cannot run until
/// the next is due.
pub const SWEEP_MAX_BATCHES: u32 = 50;

/// Deletes expired entries in batches until there are none left, or until
/// `max_batches` have run. Answers how many entries went.
///
/// Batched rather than one statement, because a single `DELETE` over a large
/// cache is a long transaction holding row locks on the hottest table in the
/// database. `batch` of a few thousand keeps each statement short;
/// `max_batches` bounds the whole call so a scheduled sweep cannot run until
/// the next one is due.
///
/// A return equal to `batch * max_batches` means it stopped early and there is
/// more to do.
///
/// # Errors
/// [`CacheError::Backend`](crate::CacheError::Backend) when a delete failed.
/// Whatever earlier batches removed stays removed.
pub async fn sweep(cache: &dyn Cache, batch: u64, max_batches: u32) -> Result<u64> {
    let mut total = 0;
    for _ in 0..max_batches {
        let removed = cache.purge_expired(batch).await?;
        total += removed;
        if removed < batch {
            break;
        }
    }
    Ok(total)
}

/// What [`Sweeper`] calls when a sweep fails.
pub type SweepErrorFn = Arc<dyn Fn(&CacheError) + Send + Sync>;

/// The periodic sweep: a task that calls [`sweep`] about every `every`.
///
/// Dropping it stops the task, and so does [`Sweeper::stop`]. A sweep that
/// is in flight when it stops is cancelled; the rows it deleted stay deleted.
#[derive(Debug)]
pub struct Sweeper {
    task: tokio::task::JoinHandle<()>,
}

impl Sweeper {
    /// Starts the task on `runtime`.
    ///
    /// Each wait is `every` with up to a quarter added or taken away, at
    /// random, so several instances do not sweep at the same moment. The
    /// first sweep comes after the first wait, not at once: the table can
    /// still be missing while the readiness check creates it. A failed sweep
    /// goes to `on_error` and the next one runs on time.
    ///
    /// A zero `every` counts as one millisecond. To have no sweep, do not
    /// call this.
    pub fn spawn(
        runtime: &tokio::runtime::Handle,
        cache: Arc<dyn Cache>,
        every: Duration,
        on_error: SweepErrorFn,
    ) -> Self {
        let every = every.max(Duration::from_millis(1));
        let task = runtime.spawn(async move {
            loop {
                tokio::time::sleep(jittered(every)).await;
                if let Err(e) = sweep(&*cache, SWEEP_BATCH, SWEEP_MAX_BATCHES).await {
                    on_error(&e);
                }
            }
        });
        Self { task }
    }

    /// Stops the task.
    pub fn stop(self) {
        drop(self);
    }

    /// Whether the task still runs. False after [`Sweeper::stop`] only once
    /// the runtime has seen the cancellation.
    pub fn is_running(&self) -> bool {
        !self.task.is_finished()
    }
}

impl Drop for Sweeper {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// `every`, plus or minus up to a quarter of it, at random.
///
/// The randomness is the standard library's hash seed, so the crate needs no
/// random-number dependency for one number per sweep.
fn jittered(every: Duration) -> Duration {
    let quarter = every / 4;
    let span = u64::try_from(quarter.as_nanos().saturating_mul(2)).unwrap_or(u64::MAX);
    if span == 0 {
        return every;
    }
    let draw = RandomState::new().build_hasher().finish() % span;
    every - quarter + Duration::from_nanos(draw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_stays_within_a_quarter_of_the_interval() {
        let every = Duration::from_secs(300);
        let waits: Vec<Duration> = (0..200).map(|_| jittered(every)).collect();
        for wait in &waits {
            assert!(*wait >= Duration::from_secs(225) && *wait <= Duration::from_secs(375));
        }
        let first = waits[0];
        assert!(
            waits.iter().any(|w| *w != first),
            "two hundred waits were all the same, so there is no jitter"
        );
    }

    #[test]
    fn a_tiny_interval_has_no_room_to_move_and_is_kept() {
        assert_eq!(jittered(Duration::from_nanos(1)), Duration::from_nanos(1));
    }
}
