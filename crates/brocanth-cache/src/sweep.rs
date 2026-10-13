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
//! # This crate ships no loop, and that is on purpose
//!
//! There is a [`sweep`] function here and nothing that calls it. No thread, no
//! timer, no `tokio::spawn`. Three reasons, in order of weight:
//!
//! 1. **`brocanth-jobs` already has the loop.** It ships a cron, a worker and
//!    a maintenance loop with a schedule table and misfire handling, and an
//!    application that runs the queue already pays for all of it. Starting a
//!    second periodic task in the same process, with its own interval and its
//!    own failure mode, is how a system ends up with two schedulers that
//!    disagree about whether something ran.
//! 2. **PLAN §8.2: a feature that is off must mean the code is absent.** An
//!    always-on sweeper means a timer and a spawned task in every application
//!    that caches anything — including one whose entries all live sixty
//!    seconds and whose table never grows past a few thousand rows, which does
//!    not need a sweeper at all.
//! 3. **The arrow would point the wrong way.** Depending on `brocanth-jobs`
//!    from here would make every caching application link a queue. A cache is
//!    the more primitive of the two, and an application that caches and does
//!    not have background work is ordinary.
//!
//! So: this crate owns the *operation*, and whatever you already run owns the
//! *schedule*. The operation is on the trait
//! ([`Cache::purge_expired`](crate::Cache::purge_expired)) rather than on the
//! Postgres type, so a later SQLite backend gets the same arrangement for
//! free.
//!
//! # Driving it
//!
//! **With `brocanth-jobs`, which is the recommended way.** A recurring job,
//! registered once in `main`:
//!
//! ```ignore
//! use std::sync::Arc;
//! use brocanth_cache::{sweep, Cache};
//! use brocanth_jobs::{Job, JobContext};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Serialize, Deserialize)]
//! struct SweepCache;
//!
//! impl Job for SweepCache {
//!     const NAME: &'static str = "sweep_cache";
//!     const QUEUE: &'static str = "maintenance";
//!     type Error = brocanth_cache::CacheError;
//!
//!     async fn perform(&self, cx: &JobContext) -> Result<(), Self::Error> {
//!         let cache: Arc<dyn Cache> = cx.get();  // however the app holds it
//!         sweep(&*cache, 10_000, 50).await?;
//!         Ok(())
//!     }
//! }
//!
//! // in main, beside the other schedules:
//! queue.recurring("sweep_cache", "*/5 * * * *", SweepCache).await?;
//! ```
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
//! **With nothing at all**, if the cache is small and the TTLs are short. The
//! table reaches a steady size because `set` reuses the row it overwrites, and
//! autovacuum reclaims what the overwrites leave. Watch
//! [`PgCache::stats`](crate::PgCache::stats) and start sweeping when `expired`
//! stops being a small fraction of `entries`.
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

use crate::cache::Cache;
use crate::error::Result;

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
