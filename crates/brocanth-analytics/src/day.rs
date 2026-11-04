//! [`Day`]: a UTC calendar day, the finest time this crate keeps.

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// A UTC day, counted from 1970-01-01.
///
/// The only time on a stored row. No hour and no minute: a time of day
/// narrows a visit down, and a daily report has no use for it. The salt
/// rotates on the same boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day(pub i32);

impl Day {
    /// The UTC day `at` falls on. A time before 1970 is day 0.
    pub fn of(at: SystemTime) -> Day {
        let secs = at
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Day(i32::try_from(secs / 86_400).unwrap_or(i32::MAX))
    }

    /// `n` days earlier.
    pub fn minus(self, n: u32) -> Day {
        Day(self.0.saturating_sub(i32::try_from(n).unwrap_or(i32::MAX)))
    }

    /// `n` days later.
    pub fn plus(self, n: u32) -> Day {
        Day(self.0.saturating_add(i32::try_from(n).unwrap_or(i32::MAX)))
    }

    /// The day as `(year, month, day)`.
    ///
    /// Howard Hinnant's `civil_from_days`, which is exact for every day an
    /// `i32` holds and needs no calendar library.
    pub fn ymd(self) -> (i32, u32, u32) {
        let z = i64::from(self.0) + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        (
            i32::try_from(y).unwrap_or(i32::MAX),
            u32::try_from(m).unwrap_or(1),
            u32::try_from(d).unwrap_or(1),
        )
    }
}

/// `2026-11-04`.
impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (y, m, d) = self.ymd();
        write!(f, "{y:04}-{m:02}-{d:02}")
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_day_prints_as_its_calendar_date() {
        assert_eq!(Day(0).to_string(), "1970-01-01");
        assert_eq!(Day(20_396).to_string(), "2025-11-04");
        assert_eq!(Day(19_782).to_string(), "2024-02-29");
    }

    #[test]
    fn the_day_turns_at_midnight_utc() {
        let before = UNIX_EPOCH + Duration::from_secs(86_400 * 10 - 1);
        assert_eq!(Day::of(before), Day(9));
        assert_eq!(Day::of(before + Duration::from_secs(1)), Day(10));
    }
}
