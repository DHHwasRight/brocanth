//! Where the time comes from.
//!
//! A trait, so a test moves the time forward and sees a token expire or a
//! started flow run out, without a sleep.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since 1970.
pub trait Clock: Send + Sync + 'static {
    /// The time now, in seconds since 1970.
    fn now(&self) -> i64;
}

/// The system clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
    }
}
