//! Time, as a seam.
//!
//! Every expiry in this crate — session idle, session absolute, token TTL,
//! flow lifetime — reads the clock through this trait. The reason is not
//! purity: it is that "an expired token is rejected" and "a session goes stale
//! after the idle timeout" are the tests that matter most here, and a test that
//! has to sleep for twenty minutes does not get written.

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch.
pub type UnixTime = i64;

/// The source of now.
pub trait Clock: Send + Sync + 'static {
    /// Seconds since the Unix epoch.
    fn now(&self) -> UnixTime;
}

/// The real clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> UnixTime {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

/// A clock a test moves by hand.
#[derive(Debug)]
pub struct TestClock(AtomicI64);

impl TestClock {
    /// A clock stopped at `at`.
    pub fn at(at: UnixTime) -> Self {
        Self(AtomicI64::new(at))
    }

    /// Moves the clock forward by `seconds`.
    pub fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }

    /// Moves the clock to `at`.
    pub fn set(&self, at: UnixTime) {
        self.0.store(at, Ordering::SeqCst);
    }
}

impl Default for TestClock {
    fn default() -> Self {
        // An arbitrary fixed point, so a failing assertion prints a number a
        // human recognises as "the test clock" rather than today's date.
        Self::at(1_700_000_000)
    }
}

impl Clock for TestClock {
    fn now(&self) -> UnixTime {
        self.0.load(Ordering::SeqCst)
    }
}
