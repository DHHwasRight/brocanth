//! [`Trail`]: the queue of new entries, the store behind it, and the reads
//! the history page makes.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use brocanth_events::{ChangeSink, RowChange};

use crate::entry::Entry;
use crate::store::{AuditStore, Query, StoreError};

/// The most entries held while the store does not answer. Past this, a
/// new entry is dropped and counted, so a database that is down cannot
/// fill the memory of the process.
pub const MAX_PENDING: usize = 10_000;

/// What tells the time: seconds since 1970. A test gives its own.
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// The change history of this process: a queue in front of a store.
///
/// A write must not wait on the history, so [`Trail::record`] only pushes
/// onto a queue. [`Trail::flush`] writes the queue in one batch; the plugin
/// calls it every `flush_secs` and at shutdown. A read flushes first, so
/// the history page shows a change made a moment ago.
///
/// It is also a [`ChangeSink`]: hand it, or [`Trail::by`], to a write of
/// the model layer and the write lands here.
///
/// Cheap to clone: an `Arc`.
#[derive(Clone)]
pub struct Trail {
    inner: Arc<Inner>,
}

struct Inner {
    store: OnceLock<Arc<dyn AuditStore>>,
    pending: Mutex<Vec<Entry>>,
    dropped: Mutex<u64>,
    last_error: Mutex<Option<String>>,
    retention_days: u32,
    clock: Clock,
}

impl std::fmt::Debug for Trail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Trail")
            .field("pending", &self.pending())
            .field("store", &self.inner.store.get().is_some())
            .field("retention_days", &self.inner.retention_days)
            .finish()
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

impl Trail {
    /// A trail with no store that keeps entries `retention_days` days
    /// (0: forever). Entries wait in the queue until a store is attached.
    pub fn new(retention_days: u32) -> Trail {
        Trail::with_clock(retention_days, Arc::new(now))
    }

    /// [`Trail::new`] with a clock of your own, for a test.
    pub fn with_clock(retention_days: u32, clock: Clock) -> Trail {
        Trail {
            inner: Arc::new(Inner {
                store: OnceLock::new(),
                pending: Mutex::new(Vec::new()),
                dropped: Mutex::new(0),
                last_error: Mutex::new(None),
                retention_days,
                clock,
            }),
        }
    }

    /// A trail over `store`.
    pub fn with_store(store: Arc<dyn AuditStore>, retention_days: u32) -> Trail {
        let trail = Trail::new(retention_days);
        trail.attach(store);
        trail
    }

    /// Sets the store, once. A second call keeps the first store and
    /// returns `false`.
    pub fn attach(&self, store: Arc<dyn AuditStore>) -> bool {
        self.inner.store.set(store).is_ok()
    }

    /// Whether a store is attached.
    pub fn has_store(&self) -> bool {
        self.inner.store.get().is_some()
    }

    /// Seconds since 1970, by this trail's clock.
    pub fn now(&self) -> i64 {
        (self.inner.clock)()
    }

    /// Queues `entry`. Its `at` is set to now when it is 0. Never waits.
    pub fn record(&self, mut entry: Entry) {
        if entry.at == 0 {
            entry.at = self.now();
        }
        let mut pending = lock(&self.inner.pending);
        if pending.len() >= MAX_PENDING {
            *lock(&self.inner.dropped) += 1;
            return;
        }
        pending.push(entry);
    }

    /// Queues a change the model layer published, made by `actor`.
    pub fn change(&self, change: &RowChange<'_>, actor: Option<String>) {
        self.record(Entry::from_change(change, actor, self.now()));
    }

    /// A [`ChangeSink`] that records each change as made by `actor`.
    ///
    /// ```
    /// # use brocanth_audit::Trail;
    /// # use brocanth_events::{ChangeSink, RowChange};
    /// let trail = Trail::new(0);
    /// let sink = trail.by("ada");
    /// sink.publish(&RowChange::created("post", 7_i64));
    /// assert_eq!(trail.pending(), 1);
    /// ```
    pub fn by(&self, actor: impl Into<String>) -> By {
        self.sink(Some(actor.into()))
    }

    /// A [`ChangeSink`] that records each change as made by `actor`, or by
    /// nobody signed in with `None`.
    pub fn sink(&self, actor: Option<String>) -> By {
        By {
            trail: Some(self.clone()),
            actor,
        }
    }

    /// How many entries wait for the next flush.
    pub fn pending(&self) -> usize {
        lock(&self.inner.pending).len()
    }

    /// How many entries were dropped because the queue was full.
    pub fn dropped(&self) -> u64 {
        *lock(&self.inner.dropped)
    }

    /// What the last flush or purge failed with, while it still fails.
    pub fn last_error(&self) -> Option<String> {
        lock(&self.inner.last_error).clone()
    }

    /// Days an entry is kept. 0 keeps every entry.
    pub fn retention_days(&self) -> u32 {
        self.inner.retention_days
    }

    /// Writes the queue to the store and says how many entries it wrote.
    ///
    /// # Errors
    ///
    /// The store failed. The entries go back to the front of the queue, so
    /// the next flush tries them again.
    pub async fn flush(&self) -> Result<usize, StoreError> {
        let Some(store) = self.inner.store.get() else {
            return Ok(0);
        };
        let batch = std::mem::take(&mut *lock(&self.inner.pending));
        if batch.is_empty() {
            return Ok(0);
        }
        match store.append(&batch).await {
            Ok(()) => {
                self.note(None);
                Ok(batch.len())
            }
            Err(e) => {
                self.note(Some(e.to_string()));
                let mut pending = lock(&self.inner.pending);
                let newer = std::mem::replace(&mut *pending, batch);
                let room = MAX_PENDING.saturating_sub(pending.len());
                if newer.len() > room {
                    *lock(&self.inner.dropped) += (newer.len() - room) as u64;
                }
                pending.extend(newer.into_iter().take(room));
                Err(e)
            }
        }
    }

    /// The entries `query` matches, newest first. Flushes first.
    ///
    /// # Errors
    ///
    /// No store is attached, or the store failed.
    pub async fn find(&self, query: &Query) -> Result<Vec<Entry>, StoreError> {
        let store = self.store()?;
        // A failed flush keeps its entries queued; the read still shows
        // what is stored.
        let _ = self.flush().await;
        store.find(query).await
    }

    /// The history of one row, newest first.
    ///
    /// # Errors
    ///
    /// As [`Trail::find`].
    pub async fn history(&self, table: &str, row: &str) -> Result<Vec<Entry>, StoreError> {
        self.find(&Query::row(table, row)).await
    }

    /// Removes the entries older than `retention_days`, and says how many.
    /// Does nothing with a retention of 0.
    ///
    /// # Errors
    ///
    /// No store is attached, or the store failed.
    pub async fn purge(&self) -> Result<u64, StoreError> {
        let days = self.inner.retention_days;
        if days == 0 {
            return Ok(0);
        }
        let before = self.now() - i64::from(days) * 86_400;
        let result = self.store()?.purge(before).await;
        if let Err(e) = &result {
            self.note(Some(e.to_string()));
        }
        result
    }

    fn store(&self) -> Result<&Arc<dyn AuditStore>, StoreError> {
        self.inner.store.get().ok_or_else(|| {
            StoreError::new(
                "the audit trail has no store\n\
                 help: register `brocanth_audit::AuditPlugin` after the `db` plugin, \
                 or build `Trail::with_store`",
            )
        })
    }

    fn note(&self, error: Option<String>) {
        *lock(&self.inner.last_error) = error;
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Records every change with no actor: a job, a migration, a page with no
/// user. [`Trail::by`] names one.
impl ChangeSink for Trail {
    fn publish(&self, change: &RowChange<'_>) {
        self.change(change, None);
    }
}

/// The [`ChangeSink`] of [`Trail::by`] and [`Trail::sink`]: each change,
/// made by one actor.
#[derive(Clone, Debug)]
pub struct By {
    trail: Option<Trail>,
    actor: Option<String>,
}

impl By {
    /// A sink that records nothing: for code that runs with no audit
    /// plugin, such as a test with a `Ctx` built by hand.
    pub fn nothing() -> By {
        By {
            trail: None,
            actor: None,
        }
    }
}

impl ChangeSink for By {
    fn publish(&self, change: &RowChange<'_>) {
        if let Some(trail) = &self.trail {
            trail.change(change, self.actor.clone());
        }
    }

    /// `false` with no trail, so the writer builds no change.
    fn wants(&self, _collection: &str) -> bool {
        self.trail.is_some()
    }
}

impl brocanth::plugin_context::ContextItem for Trail {}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::entry::Kind;
    use crate::store::MemoryStore;

    fn clock(at: &Arc<AtomicI64>) -> Clock {
        let at = Arc::clone(at);
        Arc::new(move || at.load(Ordering::SeqCst))
    }

    #[tokio::test]
    async fn a_change_waits_in_the_queue_and_a_read_finds_it() {
        let at = Arc::new(AtomicI64::new(1_000));
        let trail = Trail::with_clock(0, clock(&at));
        trail.attach(Arc::new(MemoryStore::new()));
        trail.by("ada").publish(&RowChange::created("post", 7_i64));
        let fields = ["title"];
        trail.publish(&RowChange::updated("post", 7_i64, fields.as_slice()));
        assert_eq!(trail.pending(), 2);

        let history = trail.history("post", "7").await.unwrap();
        assert_eq!(trail.pending(), 0, "a read flushes first");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].kind, Kind::Updated, "newest first");
        assert_eq!(history[0].actor, None);
        assert_eq!(history[1].actor.as_deref(), Some("ada"));
        assert_eq!(history[1].at, 1_000);
        assert!(trail.history("post", "8").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn the_purge_keeps_the_retention_and_zero_keeps_everything() {
        let at = Arc::new(AtomicI64::new(0));
        let trail = Trail::with_clock(30, clock(&at));
        trail.attach(Arc::new(MemoryStore::new()));
        trail.publish(&RowChange::created("post", 1_i64));
        at.store(40 * 86_400, Ordering::SeqCst);
        trail.publish(&RowChange::created("post", 2_i64));
        trail.flush().await.unwrap();
        assert_eq!(trail.purge().await.unwrap(), 1);
        let left = trail.find(&Query::recent()).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].row, "2");

        let forever = Trail::with_clock(0, clock(&at));
        forever.attach(Arc::new(MemoryStore::new()));
        assert_eq!(forever.purge().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn a_failed_flush_keeps_the_entries_for_the_next_one() {
        struct Down;
        #[async_trait]
        impl AuditStore for Down {
            async fn append(&self, _: &[Entry]) -> Result<(), StoreError> {
                Err(StoreError::new("down"))
            }
            async fn find(&self, _: &Query) -> Result<Vec<Entry>, StoreError> {
                Ok(Vec::new())
            }
            async fn purge(&self, _: i64) -> Result<u64, StoreError> {
                Ok(0)
            }
        }
        let trail = Trail::with_store(Arc::new(Down), 0);
        trail.publish(&RowChange::created("post", 1_i64));
        assert!(trail.flush().await.is_err());
        assert_eq!(trail.pending(), 1);
        assert_eq!(trail.last_error().as_deref(), Some("down"));
    }

    #[tokio::test]
    async fn a_read_without_a_store_says_what_to_do() {
        let e = Trail::new(0).find(&Query::recent()).await.unwrap_err();
        assert!(e.to_string().contains("help:"), "{e}");
    }

    #[test]
    fn a_full_queue_drops_and_counts() {
        let trail = Trail::new(0);
        for n in 0..(MAX_PENDING as i64 + 3) {
            trail.publish(&RowChange::created("post", n));
        }
        assert_eq!(trail.pending(), MAX_PENDING);
        assert_eq!(trail.dropped(), 3);
    }
}
