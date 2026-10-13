//! Time, as a value the cache holds rather than a function it calls.
//!
//! Every instant in this crate is **epoch milliseconds in UTC, read from the
//! caller's clock**, the same decision `brocanth-jobs` made and for the same
//! two reasons.
//!
//! **Milliseconds in a `BIGINT`, not `TIMESTAMPTZ`.** One column type, one
//! comparison semantics and one sort order, whatever database is underneath —
//! which is what lets a SQLite backend in a later crate behave identically
//! rather than nearly identically. An application that wants a readable column
//! can add a generated one; the cache will not read it.
//!
//! **The caller's clock, not the database's.** `now()` inside the SQL would
//! remove clock skew between application instances, which is a real cost and is
//! paid here — run NTP. What it buys is worth more: the clock is a parameter,
//! so a test can prove that an expired entry is not returned by moving time
//! forward rather than by sleeping, and the expiry tests in this crate are
//! exact rather than timing-dependent.
//!
//! The skew that matters is bounded and small. A TTL is a hint about freshness,
//! not a lock; an entry that lives 200 ms longer than it should on one instance
//! is not a correctness problem. The one place it is load-bearing is the
//! single-flight lease in [`crate::fetch`], where a clock that is fast ends a
//! lease early and lets a second caller compute — which costs a duplicated
//! computation, never a wrong answer.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};

/// Where the cache reads the time from.
pub trait Clock: Send + Sync + 'static {
    /// The current instant, in UTC.
    fn now(&self) -> DateTime<Utc>;
}

/// The system clock. The default everywhere.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock a test moves by hand.
///
/// Opt-in, like the rest of the test apparatus: nothing selects it for you, and
/// an application that never names it cannot accidentally run on it.
///
/// ```
/// use brocanth_cache::{Clock, ManualClock};
/// use chrono::{TimeZone, Utc};
///
/// let clock = ManualClock::new(Utc.timestamp_opt(0, 0).unwrap());
/// clock.advance(chrono::Duration::hours(2));
/// assert_eq!(clock.now().timestamp(), 7_200);
/// ```
#[derive(Clone, Debug)]
pub struct ManualClock(Arc<Mutex<DateTime<Utc>>>);

impl ManualClock {
    /// A clock stopped at `at`.
    pub fn new(at: DateTime<Utc>) -> Self {
        Self(Arc::new(Mutex::new(at)))
    }

    /// A clock stopped at the current instant.
    pub fn now_frozen() -> Self {
        Self::new(Utc::now())
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: chrono::Duration) {
        let mut guard = self.0.lock().expect("manual clock poisoned");
        *guard += by;
    }

    /// Moves the clock forward by a [`std::time::Duration`].
    pub fn advance_std(&self, by: std::time::Duration) {
        self.advance(chrono::Duration::from_std(by).unwrap_or_else(|_| chrono::Duration::zero()));
    }

    /// Jumps the clock to `at`.
    pub fn set(&self, at: DateTime<Utc>) {
        *self.0.lock().expect("manual clock poisoned") = at;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().expect("manual clock poisoned")
    }
}

/// The on-disk spelling of an instant: milliseconds since the Unix epoch.
///
/// Public because a backend in another crate — the SQLite one — has to store
/// instants the same way to behave the same way, and a conversion each backend
/// writes for itself is a conversion they can disagree about.
pub fn to_millis(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

/// The inverse of [`to_millis`]. Out-of-range values clamp to the epoch, which
/// cannot happen for a value this crate wrote and is not worth a `Result` for
/// one that somebody else wrote by hand.
pub fn from_millis(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).single().expect("the epoch"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn millis_round_trip() {
        let at = Utc.timestamp_millis_opt(1_700_000_123_456).unwrap();
        assert_eq!(from_millis(to_millis(at)), at);
    }

    #[test]
    fn a_manual_clock_only_moves_when_told_to() {
        let clock = ManualClock::new(Utc.timestamp_opt(1_000, 0).unwrap());
        let first = clock.now();
        assert_eq!(clock.now(), first);
        clock.advance(chrono::Duration::seconds(30));
        assert_eq!(clock.now() - first, chrono::Duration::seconds(30));
    }
}
