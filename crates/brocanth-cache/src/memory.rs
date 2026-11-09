//! A cache in the process's memory, for tests and development.
//!
//! [`MemoryCache`] keeps every entry in a `HashMap` behind one lock. It
//! passes the same conformance suite as [`PgCache`]
//! (`brocanth-cache-conformance`), so code written against it behaves the
//! same against the cache you deploy.
//!
//! # Not for production
//!
//! It is not shared between processes: two servers behind one load balancer
//! have two caches, and an invalidation in one does not reach the other. It
//! does not survive a restart, and it has no bound but the TTLs, so it grows
//! with what you write. `brocanth check --deploy` reports the `cache-memory`
//! module as an error for that reason.
//!
//! It is right for a test that needs a cache and no database, for
//! `brocanth dev` on a laptop without Postgres, and for a tool that runs
//! once.
//!
//! # Why it exists after all
//!
//! This crate refused an in-memory backend: a second implementation can
//! behave differently from the one you deploy. The conformance suite
//! answers that objection. A backend that passes it keeps every promise the
//! trait makes, and the suite runs on this one and on Postgres alike.
//!
//! [`PgCache`]: crate::PgCache

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;

use crate::cache::{expires_at, Cache, DEFAULT_MAX_VALUE_BYTES};
use crate::clock::{to_millis, Clock, SystemClock};
use crate::error::{CacheError, Result};
use crate::key::{Key, Namespace};
use crate::tag::Tag;

/// A namespace and a key, owned.
type Slot = (String, String);

/// One entry. `value` is `None` while somebody holds a lease on the key.
struct Entry {
    value: Option<Vec<u8>>,
    expires_at: i64,
}

#[derive(Default)]
struct State {
    entries: HashMap<Slot, Entry>,
    /// tag -> the entries stored with it.
    tagged: HashMap<String, HashSet<Slot>>,
    /// entry -> its tags, so a tagged write can drop the old ones.
    tags_of: HashMap<Slot, HashSet<String>>,
}

impl State {
    fn live(&self, slot: &Slot, now: i64) -> Option<&Entry> {
        self.entries.get(slot).filter(|e| e.expires_at > now)
    }

    fn untag(&mut self, slot: &Slot) {
        for tag in self.tags_of.remove(slot).unwrap_or_default() {
            if let Some(slots) = self.tagged.get_mut(&tag) {
                slots.remove(slot);
                if slots.is_empty() {
                    self.tagged.remove(&tag);
                }
            }
        }
    }
}

/// A cache in this process's memory. For tests and development: see the
/// [module documentation](self) for why not production.
///
/// ```
/// # use std::time::Duration;
/// # use brocanth_cache::{Cache, MemoryCache, Namespace};
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let cache = MemoryCache::new();
/// let key = Namespace::new("views")?.key("home")?;
/// cache.set(&key, b"<h1>Home</h1>", Duration::from_secs(60)).await?;
/// assert_eq!(cache.get(&key).await?, Some(b"<h1>Home</h1>".to_vec()));
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct MemoryCache {
    state: Arc<Mutex<State>>,
    clock: Arc<dyn Clock>,
    max_value_bytes: usize,
}

impl Default for MemoryCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MemoryCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryCache")
            .field("entries", &self.state().entries.len())
            .field("max_value_bytes", &self.max_value_bytes)
            .finish_non_exhaustive()
    }
}

impl MemoryCache {
    /// An empty cache, on the system clock, with the default cap.
    pub fn new() -> Self {
        Self {
            state: Arc::default(),
            clock: Arc::new(SystemClock),
            max_value_bytes: DEFAULT_MAX_VALUE_BYTES,
        }
    }

    /// Replaces the clock, so a test moves time by hand. See
    /// [`crate::clock`].
    #[must_use]
    pub fn with_clock(mut self, clock: impl Clock) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Changes the cap on one value. See [`DEFAULT_MAX_VALUE_BYTES`].
    #[must_use]
    pub fn with_max_value_bytes(mut self, bytes: usize) -> Self {
        self.max_value_bytes = bytes;
        self
    }

    /// How many entries the map holds, expired ones and leases included.
    pub fn len(&self) -> usize {
        self.state().entries.len()
    }

    /// Whether the map holds no entry at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // No method panics while it holds the lock, and a poisoned map is
        // still a map: every write leaves it consistent.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> i64 {
        to_millis(self.clock.now())
    }

    fn check_size(&self, key: &Key, value: &[u8]) -> Result<()> {
        if value.len() > self.max_value_bytes {
            return Err(CacheError::TooLarge {
                namespace: key.namespace().as_str().to_owned(),
                key: key.as_str().to_owned(),
                bytes: value.len(),
                limit: self.max_value_bytes,
            });
        }
        Ok(())
    }
}

fn slot(key: &Key) -> Slot {
    (key.namespace().as_str().to_owned(), key.as_str().to_owned())
}

#[async_trait]
impl Cache for MemoryCache {
    fn max_value_bytes(&self) -> usize {
        self.max_value_bytes
    }

    async fn migrate(&self) -> Result<()> {
        Ok(())
    }

    async fn get(&self, key: &Key) -> Result<Option<Vec<u8>>> {
        let now = self.now();
        Ok(self
            .state()
            .live(&slot(key), now)
            .and_then(|e| e.value.clone()))
    }

    async fn get_many(&self, keys: &[Key]) -> Result<Vec<Option<Vec<u8>>>> {
        let now = self.now();
        let state = self.state();
        Ok(keys
            .iter()
            .map(|key| state.live(&slot(key), now).and_then(|e| e.value.clone()))
            .collect())
    }

    async fn set(&self, key: &Key, value: &[u8], ttl: Duration) -> Result<()> {
        self.check_size(key, value)?;
        let now = self.now();
        let until = expires_at(now, ttl)?;
        self.state().entries.insert(
            slot(key),
            Entry {
                value: Some(value.to_vec()),
                expires_at: until,
            },
        );
        Ok(())
    }

    async fn set_tagged(&self, key: &Key, value: &[u8], ttl: Duration, tags: &[Tag]) -> Result<()> {
        self.check_size(key, value)?;
        let now = self.now();
        let until = expires_at(now, ttl)?;
        let at = slot(key);
        let mut state = self.state();
        state.untag(&at);
        for tag in tags {
            state
                .tagged
                .entry(tag.as_str().to_owned())
                .or_default()
                .insert(at.clone());
        }
        if !tags.is_empty() {
            state.tags_of.insert(
                at.clone(),
                tags.iter().map(|t| t.as_str().to_owned()).collect(),
            );
        }
        state.entries.insert(
            at,
            Entry {
                value: Some(value.to_vec()),
                expires_at: until,
            },
        );
        Ok(())
    }

    async fn invalidate_tags(&self, tags: &[Tag]) -> Result<u64> {
        let mut state = self.state();
        let mut slots = HashSet::new();
        for tag in tags {
            slots.extend(state.tagged.remove(tag.as_str()).unwrap_or_default());
        }
        let mut removed = 0;
        for at in slots {
            state.untag(&at);
            if state.entries.remove(&at).is_some() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    async fn add(&self, key: &Key, value: &[u8], ttl: Duration) -> Result<bool> {
        self.check_size(key, value)?;
        let now = self.now();
        let until = expires_at(now, ttl)?;
        let at = slot(key);
        let mut state = self.state();
        if state.live(&at, now).is_some() {
            return Ok(false);
        }
        state.entries.insert(
            at,
            Entry {
                value: Some(value.to_vec()),
                expires_at: until,
            },
        );
        Ok(true)
    }

    async fn delete(&self, key: &Key) -> Result<bool> {
        Ok(self.state().entries.remove(&slot(key)).is_some())
    }

    async fn clear(&self, namespace: &Namespace) -> Result<u64> {
        let mut state = self.state();
        let before = state.entries.len();
        state.entries.retain(|(ns, _), _| ns != namespace.as_str());
        Ok((before - state.entries.len()) as u64)
    }

    async fn lease(&self, key: &Key, ttl: Duration) -> Result<bool> {
        let now = self.now();
        let until = expires_at(now, ttl)?;
        let at = slot(key);
        let mut state = self.state();
        if state.live(&at, now).is_some() {
            return Ok(false);
        }
        state.entries.insert(
            at,
            Entry {
                value: None,
                expires_at: until,
            },
        );
        Ok(true)
    }

    async fn release_lease(&self, key: &Key) -> Result<()> {
        let at = slot(key);
        let mut state = self.state();
        if state.entries.get(&at).is_some_and(|e| e.value.is_none()) {
            state.entries.remove(&at);
        }
        Ok(())
    }

    async fn purge_expired(&self, limit: u64) -> Result<u64> {
        let now = self.now();
        let mut state = self.state();
        let limit = usize::try_from(limit).unwrap_or(usize::MAX);
        let expired: Vec<Slot> = state
            .entries
            .iter()
            .filter(|(_, e)| e.expires_at <= now)
            .map(|(at, _)| at.clone())
            .take(limit)
            .collect();
        for at in &expired {
            state.entries.remove(at);
        }
        // The tags of entries that are gone, up to `limit` of them too.
        let orphans: Vec<Slot> = state
            .tags_of
            .keys()
            .filter(|at| state.live(at, now).is_none())
            .take(limit)
            .cloned()
            .collect();
        for at in &orphans {
            state.untag(at);
        }
        Ok(expired.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ManualClock;
    use chrono::TimeZone;

    fn key(name: &str) -> Key {
        Namespace::new("t").unwrap().key(name).unwrap()
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_manual_clock_expires_an_entry_exactly() {
        let clock = ManualClock::new(chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap());
        let cache = MemoryCache::new().with_clock(clock.clone());
        cache
            .set(&key("k"), b"v", Duration::from_secs(30))
            .await
            .unwrap();
        clock.advance(chrono::Duration::seconds(29));
        assert_eq!(cache.get(&key("k")).await.unwrap(), Some(b"v".to_vec()));
        clock.advance(chrono::Duration::seconds(1));
        assert_eq!(cache.get(&key("k")).await.unwrap(), None);
        assert_eq!(cache.len(), 1, "expiry is lazy, as in Postgres");
        assert_eq!(cache.purge_expired(10).await.unwrap(), 1);
        assert!(cache.is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn the_sweep_drops_the_tags_of_entries_that_are_gone() {
        let clock = ManualClock::new(chrono::Utc.timestamp_opt(1_700_000_000, 0).unwrap());
        let cache = MemoryCache::new().with_clock(clock.clone());
        let t = Tag::new("t").unwrap();
        cache
            .set_tagged(
                &key("a"),
                b"x",
                Duration::from_secs(1),
                std::slice::from_ref(&t),
            )
            .await
            .unwrap();
        clock.advance(chrono::Duration::seconds(2));
        crate::sweep(&cache, 100, 10).await.unwrap();
        assert!(cache.state().tagged.is_empty());
        assert!(cache.state().tags_of.is_empty());
    }
}
