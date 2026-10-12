//! Dates and times, over text.
//!
//! # Why text
//!
//! A date arrives on the wire as `"2026-10-12"`. Something has to turn that
//! into a value, and the only question is whether this crate does it or
//! borrows a calendar library to do it. It does it, for one reason: §8 of the
//! plan says an application that does not use a capability pays nothing for
//! it, and a form with no date field should not link `chrono`.
//!
//! So the input is a string, the output is [`Date`], [`Time`] or [`DateTime`],
//! and an application that wants `chrono` types writes one
//! [`transform`](crate::Schema::transform):
//!
//! ```
//! use brocanth_schema::{date, Date, Schema};
//!
//! let born = date().max(Date::new(2026, 10, 12).unwrap());
//! assert_eq!(born.parse("1815-12-10").unwrap(), Date::new(1815, 12, 10).unwrap());
//! assert!(born.parse("2030-01-01").is_err());
//! assert!(born.parse("1815-02-30").is_err());
//! ```
//!
//! The accepted forms are ISO 8601, and only the ones a browser's `<input>`
//! and a JSON client actually send: `YYYY-MM-DD`, `HH:MM[:SS[.fff]]`, and the
//! two joined by `T` or a space with an optional `Z` or `±HH:MM` offset.

use core::fmt;
use core::str::FromStr;

use crate::issue::{quote_received, Issues, Path};
use crate::node::{RangeNode, SchemaNode};
use crate::schema::Schema;
use crate::IssueCode;

// ---------------------------------------------------------------------------
// The values
// ---------------------------------------------------------------------------

/// What is wrong with a date or time literal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateError {
    /// Not in the accepted layout at all.
    Shape,
    /// A component is outside its range, or the day does not exist in that
    /// month.
    OutOfRange,
}

impl fmt::Display for DateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DateError::Shape => f.write_str("not in the expected layout"),
            DateError::OutOfRange => f.write_str("out of range"),
        }
    }
}

impl std::error::Error for DateError {}

/// A calendar date, with no time and no zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Date {
    /// The proleptic Gregorian year.
    pub year: i32,
    /// 1 to 12.
    pub month: u8,
    /// 1 to the length of the month.
    pub day: u8,
}

impl Date {
    /// A date, checked.
    ///
    /// # Errors
    ///
    /// [`DateError::OutOfRange`] when the month is not 1 to 12 or the day does
    /// not exist in that month of that year.
    pub fn new(year: i32, month: u8, day: u8) -> Result<Date, DateError> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return Err(DateError::OutOfRange);
        }
        Ok(Date { year, month, day })
    }

    /// A date, unchecked.
    ///
    /// `#[derive(Validate)]` emits this, because it has already rejected an
    /// impossible literal at compile time. Prefer [`Date::new`] everywhere
    /// else: nothing here is unsound, but an impossible date compares as if it
    /// existed.
    #[must_use]
    pub const fn from_ymd(year: i32, month: u8, day: u8) -> Date {
        Date { year, month, day }
    }

    /// Days since 1970-01-01.
    #[must_use]
    pub fn days_from_epoch(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl FromStr for Date {
    type Err = DateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return Err(DateError::Shape);
        }
        let year = digits(&s[0..4])? as i32;
        let month = digits(&s[5..7])? as u8;
        let day = digits(&s[8..10])? as u8;
        Date::new(year, month, day)
    }
}

/// A wall-clock time, with no date and no zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Time {
    /// 0 to 23.
    pub hour: u8,
    /// 0 to 59.
    pub minute: u8,
    /// 0 to 59. Leap seconds are not accepted.
    pub second: u8,
    /// 0 to 999_999_999.
    pub nanosecond: u32,
}

impl Time {
    /// A time, checked.
    ///
    /// # Errors
    ///
    /// [`DateError::OutOfRange`] when a component is outside its range.
    pub fn new(hour: u8, minute: u8, second: u8, nanosecond: u32) -> Result<Time, DateError> {
        if hour > 23 || minute > 59 || second > 59 || nanosecond > 999_999_999 {
            return Err(DateError::OutOfRange);
        }
        Ok(Time {
            hour,
            minute,
            second,
            nanosecond,
        })
    }

    /// A time, unchecked. See [`Date::from_ymd`].
    #[must_use]
    pub const fn from_hms(hour: u8, minute: u8, second: u8) -> Time {
        Time {
            hour,
            minute,
            second,
            nanosecond: 0,
        }
    }

    /// Seconds since midnight.
    #[must_use]
    pub fn seconds_from_midnight(&self) -> i64 {
        i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second)
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}", self.hour, self.minute, self.second)?;
        if self.nanosecond != 0 {
            write!(f, ".{:09}", self.nanosecond)?;
        }
        Ok(())
    }
}

impl FromStr for Time {
    type Err = DateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() < 5 || bytes[2] != b':' {
            return Err(DateError::Shape);
        }
        let hour = digits(&s[0..2])? as u8;
        let minute = digits(&s[3..5])? as u8;
        let mut second = 0u8;
        let mut nanosecond = 0u32;
        if bytes.len() > 5 {
            if bytes[5] != b':' || bytes.len() < 8 {
                return Err(DateError::Shape);
            }
            second = digits(&s[6..8])? as u8;
            if bytes.len() > 8 {
                if bytes[8] != b'.' || bytes.len() == 9 {
                    return Err(DateError::Shape);
                }
                let frac = &s[9..];
                if frac.len() > 9 {
                    return Err(DateError::Shape);
                }
                let scaled = digits(frac)? * 10u64.pow(9 - frac.len() as u32);
                nanosecond = scaled as u32;
            }
        }
        Time::new(hour, minute, second, nanosecond)
    }
}

/// A date and a time, with an optional offset from UTC.
///
/// Ordering is by the instant, so `2026-01-01T00:00:00+01:00` sorts before
/// `2026-01-01T00:00:00Z`. A value with no offset is ordered as if it were
/// UTC, which is the only answer available without a zone database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DateTime {
    /// The calendar part.
    pub date: Date,
    /// The clock part.
    pub time: Time,
    /// Minutes east of UTC, or [`None`] for a value with no offset.
    pub offset_minutes: Option<i16>,
}

impl DateTime {
    /// A date-time with no offset.
    #[must_use]
    pub const fn naive(date: Date, time: Time) -> DateTime {
        DateTime {
            date,
            time,
            offset_minutes: None,
        }
    }

    /// Seconds since the epoch, and the nanoseconds after them.
    #[must_use]
    pub fn instant(&self) -> (i64, u32) {
        let seconds = self.date.days_from_epoch() * 86_400 + self.time.seconds_from_midnight()
            - i64::from(self.offset_minutes.unwrap_or(0)) * 60;
        (seconds, self.time.nanosecond)
    }
}

impl PartialOrd for DateTime {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for DateTime {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.instant().cmp(&other.instant())
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}T{}", self.date, self.time)?;
        match self.offset_minutes {
            None => Ok(()),
            Some(0) => f.write_str("Z"),
            Some(offset) => {
                let sign = if offset < 0 { '-' } else { '+' };
                let abs = offset.unsigned_abs();
                write!(f, "{sign}{:02}:{:02}", abs / 60, abs % 60)
            }
        }
    }
}

impl FromStr for DateTime {
    type Err = DateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (date_part, rest) = s.split_at_checked(10).ok_or(DateError::Shape)?;
        let date: Date = date_part.parse()?;
        let rest = rest
            .strip_prefix('T')
            .or_else(|| rest.strip_prefix('t'))
            .or_else(|| rest.strip_prefix(' '))
            .ok_or(DateError::Shape)?;

        let (time_part, offset) = split_offset(rest)?;
        let time: Time = time_part.parse()?;
        Ok(DateTime {
            date,
            time,
            offset_minutes: offset,
        })
    }
}

fn split_offset(rest: &str) -> Result<(&str, Option<i16>), DateError> {
    if let Some(stripped) = rest.strip_suffix('Z').or_else(|| rest.strip_suffix('z')) {
        return Ok((stripped, Some(0)));
    }
    // An offset is the last `+` or `-` in a string whose time part has none.
    if let Some(position) = rest.rfind(['+', '-']) {
        let (time_part, sign_and_offset) = rest.split_at(position);
        let sign = if sign_and_offset.starts_with('-') { -1 } else { 1 };
        let body = &sign_and_offset[1..];
        let (hours, minutes) = match body.len() {
            5 if body.as_bytes()[2] == b':' => (digits(&body[0..2])?, digits(&body[3..5])?),
            4 => (digits(&body[0..2])?, digits(&body[2..4])?),
            2 => (digits(body)?, 0),
            _ => return Err(DateError::Shape),
        };
        if hours > 23 || minutes > 59 {
            return Err(DateError::OutOfRange);
        }
        let total = sign * (hours as i16 * 60 + minutes as i16);
        return Ok((time_part, Some(total)));
    }
    Ok((rest, None))
}

fn digits(s: &str) -> Result<u64, DateError> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DateError::Shape);
    }
    s.parse::<u64>().map_err(|_| DateError::Shape)
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Howard Hinnant's `days_from_civil`, which is exact for the proleptic
/// Gregorian calendar and needs no table.
fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(if month <= 2 { year - 1 } else { year });
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// ---------------------------------------------------------------------------
// The schemas
// ---------------------------------------------------------------------------

macro_rules! temporal_schema {
    (
        $schema:ident, $builder:ident, $value:ty, $node:ident,
        shape = $shape:expr, noun = $noun:expr
    ) => {
        /// A temporal schema over its ISO 8601 text form.
        #[derive(Clone, Copy, Debug, Default)]
        pub struct $schema {
            min: Option<$value>,
            max: Option<$value>,
        }

        /// The schema, with no bounds yet.
        #[must_use]
        pub fn $builder() -> $schema {
            $schema::default()
        }

        impl $schema {
            /// No earlier than this.
            #[must_use]
            pub fn min(mut self, value: $value) -> Self {
                self.min = Some(value);
                self
            }

            /// No later than this.
            #[must_use]
            pub fn max(mut self, value: $value) -> Self {
                self.max = Some(value);
                self
            }
        }

        impl Schema for $schema {
            type Input = str;
            type Output = $value;

            fn describe(&self) -> SchemaNode {
                SchemaNode::$node(RangeNode {
                    min: self.min.map(|v| v.to_string()),
                    max: self.max.map(|v| v.to_string()),
                })
            }

            fn check(&self, input: &str, at: &mut Path, issues: &mut Issues) {
                self.parse_at(input, at, issues);
            }

            fn parse_at(
                &self,
                input: &str,
                at: &mut Path,
                issues: &mut Issues,
            ) -> Option<$value> {
                let parsed: $value = match input.parse() {
                    Ok(value) => value,
                    Err(_) => {
                        issues.report(at, IssueCode::Format, |i| {
                            i.expected(concat!($noun, " in ", $shape, " form"))
                                .received(quote_received(input))
                        });
                        return None;
                    }
                };
                let mut ok = true;
                if let Some(min) = self.min {
                    if parsed < min {
                        ok = false;
                        issues.report(at, IssueCode::TooSmall, |i| {
                            i.expected(format!("no earlier than {min}"))
                                .received(parsed.to_string())
                        });
                    }
                }
                if let Some(max) = self.max {
                    if parsed > max {
                        ok = false;
                        issues.report(at, IssueCode::TooLarge, |i| {
                            i.expected(format!("no later than {max}"))
                                .received(parsed.to_string())
                        });
                    }
                }
                ok.then_some(parsed)
            }
        }
    };
}

temporal_schema!(
    DateSchema, date, Date, Date,
    shape = "YYYY-MM-DD", noun = "a date"
);
temporal_schema!(
    TimeSchema, time, Time, Time,
    shape = "HH:MM[:SS]", noun = "a time"
);
temporal_schema!(
    DateTimeSchema, datetime, DateTime, DateTime,
    shape = "YYYY-MM-DDTHH:MM[:SS][Z|±HH:MM]", noun = "a date and time"
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_that_does_not_exist_is_rejected() {
        assert_eq!("2026-02-30".parse::<Date>(), Err(DateError::OutOfRange));
        assert!("2024-02-29".parse::<Date>().is_ok());
        assert_eq!("2023-02-29".parse::<Date>(), Err(DateError::OutOfRange));
    }

    #[test]
    fn date_layouts_outside_the_accepted_one_are_rejected() {
        for bad in ["2026-1-1", "26-01-01", "2026/01/01", "2026-01-01T00:00"] {
            assert!(bad.parse::<Date>().is_err(), "accepted {bad}");
        }
    }

    #[test]
    fn times_accept_the_three_lengths() {
        assert_eq!("09:30".parse::<Time>().unwrap(), Time::from_hms(9, 30, 0));
        assert_eq!("09:30:15".parse::<Time>().unwrap(), Time::from_hms(9, 30, 15));
        assert_eq!(
            "09:30:15.250".parse::<Time>().unwrap().nanosecond,
            250_000_000
        );
        assert!("24:00".parse::<Time>().is_err());
    }

    #[test]
    fn offsets_order_by_the_instant() {
        let utc: DateTime = "2026-01-01T00:00:00Z".parse().unwrap();
        let ahead: DateTime = "2026-01-01T00:00:00+01:00".parse().unwrap();
        let behind: DateTime = "2026-01-01T00:00:00-05:00".parse().unwrap();
        assert!(ahead < utc);
        assert!(utc < behind);
    }

    #[test]
    fn epoch_day_zero_is_nineteen_seventy() {
        assert_eq!(Date::new(1970, 1, 1).unwrap().days_from_epoch(), 0);
        assert_eq!(Date::new(1969, 12, 31).unwrap().days_from_epoch(), -1);
        assert_eq!(Date::new(2000, 3, 1).unwrap().days_from_epoch(), 11017);
    }

    #[test]
    fn bounds_report_both_ends() {
        let schema = date()
            .min(Date::new(2020, 1, 1).unwrap())
            .max(Date::new(2030, 1, 1).unwrap());
        assert!(schema.validate("2019-12-31").is_err());
        assert!(schema.validate("2020-01-01").is_ok());
        assert!(schema.validate("2030-01-02").is_err());
    }
}
