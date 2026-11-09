//! Idempotency keys: a delivery that arrives twice is handled once.
//!
//! Every sender retries. Stripe retries for three days, GitHub on demand,
//! and any sender retries when our 200 was lost on the way back. So a
//! handler sees one event more than once unless something remembers.
//!
//! The key is claimed **before** the handler runs and released when the
//! handler fails. Two copies that arrive together cannot both run: the
//! second finds the key claimed and is answered 200 with nothing done. A
//! failed handler gives the key back, so the sender's retry runs it again.
//!
//! A key is kept for a window ([`DEFAULT_KEEP`], a week), then forgotten.
//! A sender that retries after the window is handled again; a week is
//! longer than any sender in the table in `crate::sign` retries.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;

/// How long a key is kept: a week.
pub const DEFAULT_KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

/// Where the keys are kept.
#[async_trait]
pub trait Seen: Send + Sync + 'static {
    /// Claims `(source, id)` for `keep`. `true` the first time, `false` while
    /// a claim inside the window holds it.
    ///
    /// # Errors
    /// The store could not answer. The caller answers 503, so the sender
    /// retries, rather than run a handler it cannot deduplicate.
    async fn claim(&self, source: &str, id: &str, keep: Duration) -> Result<bool, SeenError>;

    /// Gives a claim back, after a handler failed.
    ///
    /// # Errors
    /// The store could not answer.
    async fn release(&self, source: &str, id: &str) -> Result<(), SeenError>;
}

/// The store could not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeenError(pub String);

impl std::fmt::Display for SeenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "idempotency store: {}", self.0)
    }
}

impl std::error::Error for SeenError {}

/// Keys in this process's memory.
///
/// Lost at restart, and not shared between two instances. Right for one
/// process in development; with the `db` module the generated project uses
/// [`PgSeen`](crate::PgSeen) instead.
///
/// Bounded: past `cap` keys, the expired ones are dropped, then the oldest.
pub struct MemorySeen {
    keys: Mutex<HashMap<(String, String), Instant>>,
    cap: usize,
}

impl MemorySeen {
    /// A store of at most 100 000 keys.
    pub fn new() -> Self {
        Self::with_cap(100_000)
    }

    /// A store of at most `cap` keys.
    pub fn with_cap(cap: usize) -> Self {
        Self {
            keys: Mutex::new(HashMap::new()),
            cap: cap.max(1),
        }
    }

    /// How many keys are held.
    pub fn len(&self) -> usize {
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Whether no key is held.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for MemorySeen {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Seen for MemorySeen {
    async fn claim(&self, source: &str, id: &str, keep: Duration) -> Result<bool, SeenError> {
        let now = Instant::now();
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        let key = (source.to_string(), id.to_string());
        if let Some(until) = keys.get(&key) {
            if *until > now {
                return Ok(false);
            }
        }
        if keys.len() >= self.cap {
            keys.retain(|_, until| *until > now);
            while keys.len() >= self.cap {
                let Some(oldest) = keys
                    .iter()
                    .min_by_key(|(_, until)| **until)
                    .map(|(k, _)| k.clone())
                else {
                    break;
                };
                keys.remove(&oldest);
            }
        }
        keys.insert(key, now + keep);
        Ok(true)
    }

    async fn release(&self, source: &str, id: &str) -> Result<(), SeenError> {
        self.keys
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(source.to_string(), id.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn a_key_is_claimed_once_and_again_after_release() {
        let seen = MemorySeen::new();
        run(async {
            assert!(seen.claim("stripe", "evt_1", DEFAULT_KEEP).await.unwrap());
            assert!(!seen.claim("stripe", "evt_1", DEFAULT_KEEP).await.unwrap());
            assert!(seen.claim("github", "evt_1", DEFAULT_KEEP).await.unwrap());
            seen.release("stripe", "evt_1").await.unwrap();
            assert!(seen.claim("stripe", "evt_1", DEFAULT_KEEP).await.unwrap());
        });
    }

    #[test]
    fn an_expired_key_is_claimed_again() {
        let seen = MemorySeen::new();
        run(async {
            assert!(seen.claim("s", "1", Duration::ZERO).await.unwrap());
            assert!(seen.claim("s", "1", DEFAULT_KEEP).await.unwrap());
        });
    }

    #[test]
    fn the_store_never_holds_more_than_its_cap() {
        let seen = MemorySeen::with_cap(3);
        run(async {
            for id in 0..10 {
                seen.claim("s", &id.to_string(), DEFAULT_KEEP)
                    .await
                    .unwrap();
            }
        });
        assert_eq!(seen.len(), 3);
    }
}
