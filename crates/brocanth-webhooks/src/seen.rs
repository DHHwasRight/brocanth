//! Idempotency keys: a delivery that arrives twice is handled once.
//!
//! Every sender retries. Stripe retries for three days, GitHub on demand,
//! and any sender retries when our 200 was lost on the way back. So a
//! handler sees one event more than once unless something remembers.
//!
//! The key is claimed **before** the handler runs, for a short [`LEASE`].
//! Two copies that arrive together cannot both run: the second finds the
//! key [`Claim::Running`] and is answered 409, so its sender retries it
//! later. When the handler returns `Ok`, the claim is confirmed for the
//! whole window and a later copy finds it [`Claim::Done`]. A failed
//! handler gives the key back, so the sender's retry runs it again. A
//! handler that never returns -- a panic, a dropped request -- holds the
//! key for the lease at most.
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

/// The longest a key is kept: a year. A longer `keep` is clamped to it, so
/// a deadline never overflows.
pub const MAX_KEEP: Duration = Duration::from_secs(365 * 24 * 3600);

/// How long a claim holds before its handler returns: five minutes.
pub const LEASE: Duration = Duration::from_secs(5 * 60);

/// What a claim found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    /// The key was free, and is now held for the lease.
    New,
    /// A run is in progress under this key: the copy is retried later.
    Running,
    /// A run finished under this key: the copy is a duplicate.
    Done,
}

/// Where the keys are kept.
#[async_trait]
pub trait Seen: Send + Sync + 'static {
    /// Claims `(source, id)` for `lease`. [`Claim::New`] the first time;
    /// [`Claim::Running`] or [`Claim::Done`] while an earlier claim holds.
    ///
    /// # Errors
    /// The store could not answer. The caller answers 503, so the sender
    /// retries, rather than run a handler it cannot deduplicate.
    async fn claim(&self, source: &str, id: &str, lease: Duration) -> Result<Claim, SeenError>;

    /// Marks a claim done, and keeps it for `keep`, after a handler
    /// returned `Ok`.
    ///
    /// # Errors
    /// The store could not answer. The claim keeps its lease.
    async fn confirm(&self, source: &str, id: &str, keep: Duration) -> Result<(), SeenError>;

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
    /// Each key's deadline, and whether its run finished.
    keys: Mutex<HashMap<(String, String), (Instant, bool)>>,
    cap: usize,
}

/// `now + keep`, clamped to [`MAX_KEEP`] so it cannot overflow.
fn deadline(now: Instant, keep: Duration) -> Instant {
    now.checked_add(keep.min(MAX_KEEP)).unwrap_or(now)
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
    async fn claim(&self, source: &str, id: &str, lease: Duration) -> Result<Claim, SeenError> {
        let now = Instant::now();
        let mut keys = self.keys.lock().unwrap_or_else(|e| e.into_inner());
        let key = (source.to_string(), id.to_string());
        if let Some((until, done)) = keys.get(&key) {
            if *until > now {
                return Ok(if *done { Claim::Done } else { Claim::Running });
            }
        }
        if keys.len() >= self.cap {
            keys.retain(|_, (until, _)| *until > now);
            while keys.len() >= self.cap {
                let Some(oldest) = keys
                    .iter()
                    .min_by_key(|(_, (until, _))| *until)
                    .map(|(k, _)| k.clone())
                else {
                    break;
                };
                keys.remove(&oldest);
            }
        }
        keys.insert(key, (deadline(now, lease), false));
        Ok(Claim::New)
    }

    async fn confirm(&self, source: &str, id: &str, keep: Duration) -> Result<(), SeenError> {
        let now = Instant::now();
        self.keys.lock().unwrap_or_else(|e| e.into_inner()).insert(
            (source.to_string(), id.to_string()),
            (deadline(now, keep), true),
        );
        Ok(())
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
            assert_eq!(seen.claim("stripe", "evt_1", LEASE).await, Ok(Claim::New));
            assert_eq!(
                seen.claim("stripe", "evt_1", LEASE).await,
                Ok(Claim::Running)
            );
            assert_eq!(seen.claim("github", "evt_1", LEASE).await, Ok(Claim::New));
            seen.release("stripe", "evt_1").await.unwrap();
            assert_eq!(seen.claim("stripe", "evt_1", LEASE).await, Ok(Claim::New));
            seen.confirm("stripe", "evt_1", DEFAULT_KEEP).await.unwrap();
            assert_eq!(seen.claim("stripe", "evt_1", LEASE).await, Ok(Claim::Done));
        });
    }

    #[test]
    fn an_expired_key_is_claimed_again() {
        let seen = MemorySeen::new();
        run(async {
            assert_eq!(seen.claim("s", "1", Duration::ZERO).await, Ok(Claim::New));
            assert_eq!(seen.claim("s", "1", LEASE).await, Ok(Claim::New));
        });
    }

    #[test]
    fn a_huge_keep_does_not_overflow() {
        let seen = MemorySeen::new();
        run(async {
            assert_eq!(seen.claim("s", "1", Duration::MAX).await, Ok(Claim::New));
            seen.confirm("s", "1", Duration::MAX).await.unwrap();
            assert_eq!(seen.claim("s", "1", LEASE).await, Ok(Claim::Done));
        });
    }

    #[test]
    fn the_store_never_holds_more_than_its_cap() {
        let seen = MemorySeen::with_cap(3);
        run(async {
            for id in 0..10 {
                seen.claim("s", &id.to_string(), LEASE).await.unwrap();
            }
        });
        assert_eq!(seen.len(), 3);
    }
}
