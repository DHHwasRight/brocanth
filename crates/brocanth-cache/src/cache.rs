//! The cache contract, which is the whole public surface a caller needs.
//!
//! §8 of the plan says a capability is a crate, the core depends on none of
//! them, and a plugin that needs another depends on it in the open. So there is
//! one trait here, [`PgCache`] implements it, and everything above — a handler,
//! a view renderer, a policy cache — talks to the trait.
//!
//! # Values are bytes
//!
//! `set` takes `&[u8]` and `get` answers `Vec<u8>`. This crate does not depend
//! on `serde`, does not know what JSON is, and will not grow a `set_json`.
//!
//! That is not minimalism for its own sake. A cache that owns the
//! serialisation owns the *format*, and the format is where a cache goes
//! wrong: a struct gains a field, the old entries deserialise into the new
//! type — or do not — and the failure lands at read time in production, on
//! bytes nobody can see. When the caller serialises, the caller already has
//! the mechanism for that problem, because it is the same mechanism it uses
//! for its database rows and its HTTP bodies. And the caller is the only one
//! who can put the format's version in the key, which is the actual fix
//! ([`Namespace::versioned`]).
//!
//! It is also what makes the cache useful for things that are already bytes: a
//! rendered HTML fragment, a compressed response body, an image thumbnail, an
//! upstream API's response verbatim. None of those want to be round-tripped
//! through a serialiser.
//!
//! # Expiry: lazy on read, and a sweep that is only about space
//!
//! **A TTL is enforced on read, by the read.** Every statement that returns a
//! value carries `AND expires_at > $now`, so an expired entry is a miss from
//! the instant it expires, whatever else is or is not running. This is the
//! correctness mechanism and it has no moving parts.
//!
//! **A sweep is about disk, not about correctness.** Expired rows that nobody
//! reads still occupy pages. [`Cache::purge_expired`] removes them, and
//! [`crate::sweep`] is a helper that calls it in batches until the table is
//! clean.
//!
//! **This crate ships no loop, no thread and no timer to drive that sweep**,
//! and that is the deliberate part. See [`crate::sweep`] for the argument and
//! for the three ways to drive it — the first of which is a recurring job in
//! `brocanth-jobs`, which already has a cron, a worker and a maintenance loop,
//! and does not need a second one living here.
//!
//! # Object safety
//!
//! [`Cache`] is object-safe: no generic method, no `Self` in a return
//! position, and `async_trait` boxes the futures. `Arc<dyn Cache>` is the
//! expected way to hold one — it is what [`CachePlugin`] puts in the context,
//! and what [`crate::sweep::sweep`] takes.
//!
//! The one operation that *is* generic, [`CacheExt::fetch`], lives on an
//! extension trait with a blanket implementation for `C: Cache + ?Sized`, so
//! it is still available on `dyn Cache`. That is the standard shape —
//! `AsyncReadExt` over `AsyncRead` — and it is the reason the object-safe
//! trait carries [`Cache::lease`] and [`Cache::release_lease`]: `fetch` is
//! written once, against the trait, and every backend gets the same
//! single-flight behaviour without reimplementing it.
//!
//! [`PgCache`]: crate::PgCache
//! [`CachePlugin`]: crate::CachePlugin
//! [`Namespace::versioned`]: crate::Namespace::versioned

use std::time::Duration;

use async_trait::async_trait;

use crate::error::{CacheError, Result};
use crate::key::Key;
use crate::key::Namespace;

/// The default cap on one value, in bytes: 1 MiB.
///
/// # Why there is a cap at all
///
/// Postgres will take far more than this. `bytea` runs to a gigabyte, and
/// TOAST compresses and splits anything over about two kilobytes
/// transparently, so a ten-megabyte entry *works*. It is still a bad idea,
/// for three reasons that have nothing to do with whether it fits:
///
/// 1. **It evicts the database's own working set.** A cached value read
///    through Postgres passes through `shared_buffers`, which is the memory
///    your real queries are using for their indexes. A few large entries read
///    often will push out the pages that make the rest of the application
///    fast. This is the cost people do not predict, and it shows up as "the
///    database got slower" rather than as "the cache got slower".
/// 2. **It is a round trip, not a pointer.** Every read copies the whole value
///    over the connection and decompresses it. At a megabyte that is already
///    comparable to recomputing many things.
/// 3. **It multiplies the garbage.** An entry rewritten often leaves a dead
///    tuple each time, and a TOASTed value leaves dead rows in the TOAST table
///    too. See the crate documentation on vacuum pressure.
///
/// A megabyte is enough for a rendered page, a large JSON document or a
/// serialised result set, and small enough that a thousand hot entries are a
/// gigabyte rather than a surprise. Raise it with
/// [`PgCache::with_max_value_bytes`] when you know why; if the answer is "a
/// video", the answer is `brocanth-storage`, not a cache.
///
/// [`PgCache::with_max_value_bytes`]: crate::PgCache::with_max_value_bytes
pub const DEFAULT_MAX_VALUE_BYTES: usize = 1024 * 1024;

/// Everything the rest of the system needs from a cache.
///
/// Implementations must be safe to call concurrently; every method takes
/// `&self` for the same reason a connection pool does.
///
/// A **miss is `Ok(None)`, never an error**, and an expired entry is a miss.
#[async_trait]
pub trait Cache: Send + Sync + 'static {
    /// The largest value this cache will store, in bytes.
    ///
    /// A caller that cannot bound what it is about to cache should ask, and
    /// skip the cache rather than take a [`CacheError::TooLarge`]. See
    /// [`DEFAULT_MAX_VALUE_BYTES`] for what the number is and why.
    fn max_value_bytes(&self) -> usize;

    /// Creates this backend's storage if it is not there.
    ///
    /// Idempotent, and deliberately not a migration framework: §4.3 of the
    /// plan says migrations are files a developer reads, edits and commits,
    /// and this crate is not going to invent a second mechanism for its own
    /// one table. [`PgCache::schema_sql`] hands the same statements back as
    /// text, so an application that keeps every change in `migrations/` can
    /// paste them into one.
    ///
    /// A backend with nothing to create answers `Ok(())`.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when the statements would not run.
    ///
    /// [`PgCache::schema_sql`]: crate::PgCache::schema_sql
    async fn migrate(&self) -> Result<()>;

    /// The value stored at `key`, or `None`.
    ///
    /// `None` covers all three of: nothing was ever stored, it was stored and
    /// has expired, and somebody is computing it right now
    /// ([`Cache::lease`]). A caller cannot tell them apart and does not need
    /// to: all three mean "I do not have it for you".
    ///
    /// # Errors
    /// [`CacheError::Backend`] when the backend could not answer.
    async fn get(&self, key: &Key) -> Result<Option<Vec<u8>>>;

    /// The values for several keys, in one round trip, positionally aligned
    /// with `keys`.
    ///
    /// The returned vector always has `keys.len()` entries. This exists
    /// because a round trip is the whole cost of a database-backed cache: one
    /// query for forty keys is the difference between a cache that pays for
    /// itself and one that does not. A page that renders forty cached
    /// fragments should call this once, not `get` forty times.
    ///
    /// The default implementation calls [`Cache::get`] in a loop, which is
    /// correct and slow; a backend that can do better should.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when the backend could not answer.
    async fn get_many(&self, keys: &[Key]) -> Result<Vec<Option<Vec<u8>>>> {
        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            out.push(self.get(key).await?);
        }
        Ok(out)
    }

    /// Stores `value` at `key` for `ttl`, replacing whatever was there.
    ///
    /// # Errors
    /// [`CacheError::TooLarge`] when the value is over
    /// [`Cache::max_value_bytes`]; [`CacheError::InvalidTtl`] for a zero or
    /// unrepresentable `ttl`; [`CacheError::Backend`] when the write failed.
    async fn set(&self, key: &Key, value: &[u8], ttl: Duration) -> Result<()>;

    /// Stores `value` at `key` only if nothing live is there, and answers
    /// whether it did.
    ///
    /// "Nothing live" means no entry, or an entry that has already expired —
    /// an expired row is a free slot, not an obstacle. The check and the write
    /// are one statement, so two callers racing produce exactly one winner.
    ///
    /// This is the primitive underneath [`CacheExt::fetch`], and it is
    /// directly useful on its own for "only the first of these should do the
    /// work" — a once-a-day email, a warm-up, a one-shot migration trigger.
    ///
    /// # Errors
    /// As [`Cache::set`].
    async fn add(&self, key: &Key, value: &[u8], ttl: Duration) -> Result<bool>;

    /// Removes `key`, and answers whether anything was there.
    ///
    /// Idempotent: deleting what is not there succeeds.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when the delete failed.
    async fn delete(&self, key: &Key) -> Result<bool>;

    /// Removes every entry in `namespace`, and answers how many went.
    ///
    /// **Think before reaching for this.** It is correct and immediate, and it
    /// makes one dead tuple per row on the write-hottest table in the
    /// database. [`Namespace::versioned`] invalidates a whole class of entries
    /// by making nothing read them, at a cost of zero rows touched, and is the
    /// right answer whenever the invalidation is a deploy rather than an
    /// event.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when the delete failed.
    async fn clear(&self, namespace: &Namespace) -> Result<u64>;

    /// Takes an exclusive, expiring claim on `key` without storing a value,
    /// and answers whether it got it.
    ///
    /// While a lease is held, [`Cache::get`] on that key answers `None` — the
    /// lease is not a value — and another `lease` answers `false` until it
    /// expires. A later [`Cache::set`] replaces it with the real value.
    ///
    /// This is half of the stampede answer and it is on the object-safe trait
    /// so that [`CacheExt::fetch`] can be written once for every backend. It
    /// is rarely the thing you call directly. See [`crate::fetch`] for the
    /// argument about why it is a lease rather than an advisory lock.
    ///
    /// # Errors
    /// [`CacheError::InvalidTtl`] for a zero `ttl`; [`CacheError::Backend`]
    /// when the write failed.
    async fn lease(&self, key: &Key, ttl: Duration) -> Result<bool>;

    /// Gives up a lease on `key` without storing anything.
    ///
    /// Removes the entry **only if it is still a lease**. A value that
    /// somebody else stored in the meantime — because this lease expired and
    /// another caller took over — is left alone, which is the difference
    /// between this and [`Cache::delete`] and the reason both exist.
    ///
    /// Not strictly required: a lease nobody releases expires on its own. It
    /// turns "the next caller waits out the lease" into "the next caller
    /// starts now", which matters when the computation failed in fifty
    /// milliseconds and the lease was for thirty seconds.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when the delete failed.
    async fn release_lease(&self, key: &Key) -> Result<()>;

    /// Deletes up to `limit` expired entries and answers how many went.
    ///
    /// Purely about reclaiming space. Nothing in this crate's correctness
    /// depends on it ever being called — an entry is dead the moment it
    /// expires, whether or not its row is still there. See [`crate::sweep`].
    ///
    /// `limit` bounds the work so one call is a short transaction rather than
    /// a lock on the whole table. A return equal to `limit` means there is
    /// probably more to do.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when the delete failed.
    async fn purge_expired(&self, limit: u64) -> Result<u64>;
}

/// Checks a TTL and turns it into an absolute expiry in epoch milliseconds.
///
/// Shared by every backend, so "a zero TTL is an error" is one decision in one
/// place rather than a thing each backend remembers.
///
/// # Errors
/// [`CacheError::InvalidTtl`] when `ttl` is zero or so long it does not fit.
pub fn expires_at(now_ms: i64, ttl: Duration) -> Result<i64> {
    if ttl.is_zero() {
        return Err(CacheError::InvalidTtl("a time to live of zero".into()));
    }
    let ms = i64::try_from(ttl.as_millis()).map_err(|_| {
        CacheError::InvalidTtl(format!("{ttl:?}, which does not fit in an instant"))
    })?;
    now_ms
        .checked_add(ms)
        .ok_or_else(|| CacheError::InvalidTtl(format!("{ttl:?}, which runs past the end of time")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_ttl_is_refused_rather_than_silently_doing_nothing() {
        assert!(matches!(
            expires_at(0, Duration::ZERO),
            Err(CacheError::InvalidTtl(_))
        ));
    }

    #[test]
    fn a_ttl_that_would_overflow_is_refused() {
        assert!(expires_at(i64::MAX - 1, Duration::from_secs(60)).is_err());
        assert!(expires_at(0, Duration::from_secs(u64::MAX / 1000)).is_err());
    }

    #[test]
    fn an_ordinary_ttl_becomes_an_absolute_instant() {
        assert_eq!(expires_at(1_000, Duration::from_secs(60)).unwrap(), 61_000);
    }
}
