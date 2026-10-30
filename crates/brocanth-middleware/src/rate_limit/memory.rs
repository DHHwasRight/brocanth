//! The counters in this process.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brocanth::plugin_routes::BoxFuture;

use super::{Hit, Store, StoreError};

/// How many counts between two sweeps of the expired windows.
const SWEEP_EVERY: u64 = 1024;

type Clock = Arc<dyn Fn() -> Duration + Send + Sync>;

/// A map from key to `(window end, count)`, behind one mutex.
///
/// The lock is held for one map lookup and one add, so it is not the
/// thing that limits a server. Every 1024th count also drops the windows
/// that have ended, so a scraper that rotates addresses does not grow
/// the map without end.
pub struct MemoryStore {
    windows: Mutex<HashMap<String, (Duration, u64)>>,
    counted: AtomicU64,
    clock: Clock,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    /// A store on the system clock.
    pub fn new() -> Self {
        Self::with_clock(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
        })
    }

    /// A store on this clock: time since the Unix epoch. For a test that
    /// moves time.
    pub fn with_clock(clock: impl Fn() -> Duration + Send + Sync + 'static) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            counted: AtomicU64::new(0),
            clock: Arc::new(clock),
        }
    }

    /// Count one request, now. [`Store::hit`] without the future.
    pub fn count(&self, key: &str, window: Duration) -> Hit {
        let now = (self.clock)();
        let size = window.as_secs().max(1);
        let end = Duration::from_secs((now.as_secs() / size + 1) * size);
        let sweep = self
            .counted
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(SWEEP_EVERY);

        // A poisoned lock means a panic while one add ran. The map is
        // still a map, so counting goes on.
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if sweep {
            windows.retain(|_, (ends, _)| *ends > now);
        }
        let slot = windows.entry(key.to_string()).or_insert((end, 0));
        if slot.0 != end {
            *slot = (end, 0);
        }
        slot.1 += 1;
        Hit {
            count: slot.1,
            resets_in: end.saturating_sub(now),
        }
    }

    /// How many keys hold a window, ended or not.
    pub fn len(&self) -> usize {
        self.windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    /// Whether no key holds a window.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Store for MemoryStore {
    fn hit<'a>(&'a self, key: &'a str, window: Duration) -> BoxFuture<'a, Result<Hit, StoreError>> {
        let hit = self.count(key, window);
        Box::pin(async move { Ok(hit) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clock_at(secs: &Arc<AtomicU64>) -> MemoryStore {
        let secs = secs.clone();
        MemoryStore::with_clock(move || Duration::from_secs(secs.load(Ordering::SeqCst)))
    }

    #[test]
    fn a_window_counts_up_and_starts_again_at_its_end() {
        let now = Arc::new(AtomicU64::new(1_000));
        let store = clock_at(&now);
        let minute = Duration::from_secs(60);
        // 1000 is 40 seconds into the window that ends at 1020.
        assert_eq!(
            store.count("k", minute),
            Hit {
                count: 1,
                resets_in: Duration::from_secs(20)
            }
        );
        assert_eq!(store.count("k", minute).count, 2);
        assert_eq!(store.count("other", minute).count, 1);

        now.store(1_020, Ordering::SeqCst);
        assert_eq!(
            store.count("k", minute),
            Hit {
                count: 1,
                resets_in: minute
            }
        );
    }

    #[test]
    fn ended_windows_are_swept() {
        let now = Arc::new(AtomicU64::new(0));
        let store = clock_at(&now);
        for i in 0..SWEEP_EVERY {
            store.count(&format!("k{i}"), Duration::from_secs(1));
        }
        assert_eq!(store.len() as u64, SWEEP_EVERY);
        now.store(10, Ordering::SeqCst);
        store.count("late", Duration::from_secs(1));
        assert_eq!(store.len(), 1, "the sweep kept ended windows");
    }
}
