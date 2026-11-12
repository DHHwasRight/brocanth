//! Time, as a value the queue holds rather than a function it calls.
//!
//! **The clock drives the recurring schedules, and nothing else.** A
//! schedule's next run is **epoch milliseconds in UTC, read from this
//! clock**, and both decisions are deliberate.
//!
//! **Milliseconds, not a database timestamp type.** Postgres would rather have
//! `TIMESTAMPTZ`, but the schedules also have an in-memory store, and the two
//! must compare and sort the same way. So it is `BIGINT`, compared with `<=`
//! and sorted as an integer in both.
//!
//! **This clock, not the database's.** The clock is a parameter, so a test
//! can advance it by an hour without sleeping for an hour, and the schedule
//! tests in this crate are exact rather than timing-dependent. Rails takes
//! the same position with `Time.current`.
//!
//! **Jobs do not use it.** A job's delay (`Enqueue::in_`, `Enqueue::at`), its
//! retry backoff and [`Queue::inspect`](crate::Queue::inspect) read the system
//! clock, and the engine turns a delay into an instant on the store's clock:
//! Postgres with the server's `now()`. One source for all of them means a
//! `ManualClock` cannot make a job's delay disagree with the store's, and it
//! cannot make a retry test exact either: those wait real time.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, TimeZone, Utc};

/// Where the queue reads the time from.
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
/// use brocanth_jobs::{Clock, ManualClock};
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
        self.advance(chrono::Duration::from_std(by).unwrap_or(chrono::Duration::zero()));
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
pub(crate) fn to_millis(at: DateTime<Utc>) -> i64 {
    at.timestamp_millis()
}

/// The inverse of [`to_millis`]. Out-of-range values clamp to the epoch, which
/// cannot happen for a value this crate wrote and is not worth a `Result` for
/// one that someone else wrote by hand.
pub(crate) fn from_millis(ms: i64) -> DateTime<Utc> {
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
