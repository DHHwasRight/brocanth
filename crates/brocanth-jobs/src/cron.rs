//! A five-field cron expression, and the next time it fires.
//!
//! Written here rather than taken from a crate, for two reasons that are not
//! "not invented here". First, it is the only randomness-free, I/O-free,
//! two-hundred-line thing in this crate, so a dependency buys very little.
//! Second, and the one that matters: **the semantics are a design decision and
//! this crate needs to own them.**
//!
//! # Everything is UTC
//!
//! A cron expression here is evaluated in UTC and there is no way to ask for a
//! local time zone. This is a deliberate restriction, not a missing feature.
//! A daily job at 02:30 local time runs twice on the day the clocks go back and
//! not at all on the day they go forward, and the second case is a report that
//! silently did not happen. Cron implementations that offer a time zone offer
//! that bug. An application that genuinely wants "09:00 in Paris" can compute
//! the UTC hour for its own offset and accept that it shifts twice a year,
//! which is at least a shift it chose.
//!
//! # The syntax
//!
//! Five fields: `minute hour day-of-month month day-of-week`.
//!
//! * `*` — every value
//! * `5` — exactly five
//! * `1-5` — a range, inclusive
//! * `*/15` — every fifteenth, from the start of the field's range
//! * `1-20/5` — every fifth within a range
//! * `1,5,9` — a list of any of the above
//! * month names `jan`–`dec` and day names `sun`–`sat`, case-insensitive
//! * day-of-week accepts `7` as well as `0` for Sunday
//!
//! Plus the usual shorthands: `@yearly`, `@annually`, `@monthly`, `@weekly`,
//! `@daily`, `@midnight`, `@hourly`.
//!
//! When **both** day-of-month and day-of-week are restricted, a day matches if
//! **either** does. That is Vixie cron's rule, it surprises everybody once, and
//! deviating from it would surprise everybody twice.
//!
//! Seconds are not a field. A queue that polls every few seconds cannot honour
//! a per-second schedule, and accepting one would be a promise this crate
//! cannot keep.

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};

use crate::error::Error;

/// A parsed cron expression.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cron {
    source: String,
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    day_restricted: bool,
    weekday_restricted: bool,
}

impl Cron {
    /// Parses a five-field expression or an `@` shorthand.
    ///
    /// ```
    /// use brocanth_jobs::Cron;
    /// use chrono::{TimeZone, Utc};
    ///
    /// let cron = Cron::parse("30 3 * * *").unwrap();
    /// let from = Utc.with_ymd_and_hms(2026, 3, 1, 4, 0, 0).unwrap();
    /// let next = cron.next_after(from).unwrap();
    /// assert_eq!(next, Utc.with_ymd_and_hms(2026, 3, 2, 3, 30, 0).unwrap());
    /// ```
    pub fn parse(expression: &str) -> Result<Self, Error> {
        let trimmed = expression.trim();
        let expanded = match trimmed.to_ascii_lowercase().as_str() {
            "@yearly" | "@annually" => "0 0 1 1 *".to_string(),
            "@monthly" => "0 0 1 * *".to_string(),
            "@weekly" => "0 0 * * 0".to_string(),
            "@daily" | "@midnight" => "0 0 * * *".to_string(),
            "@hourly" => "0 * * * *".to_string(),
            _ => trimmed.to_string(),
        };

        let fields: Vec<&str> = expanded.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(Error::Cron {
                expression: expression.to_string(),
                reason: format!(
                    "expected 5 fields (minute hour day-of-month month day-of-week), got {}",
                    fields.len()
                ),
            });
        }

        let fail = |reason: String| Error::Cron {
            expression: expression.to_string(),
            reason,
        };

        let minutes = parse_field(fields[0], 0, 59, &[]).map_err(&fail)?;
        let hours = parse_field(fields[1], 0, 23, &[]).map_err(&fail)?;
        let days = parse_field(fields[2], 1, 31, &[]).map_err(&fail)?;
        let months = parse_field(fields[3], 1, 12, MONTH_NAMES).map_err(&fail)?;
        let weekdays = parse_field(fields[4], 0, 7, DAY_NAMES).map_err(&fail)?;
        // Sunday is both 0 and 7; fold 7 onto 0 so the lookup is by weekday
        // number alone.
        let weekdays = if weekdays & (1 << 7) != 0 {
            (weekdays | 1) & !(1 << 7)
        } else {
            weekdays
        };

        Ok(Cron {
            source: trimmed.to_string(),
            minutes,
            hours,
            days,
            months,
            weekdays,
            day_restricted: fields[2] != "*",
            weekday_restricted: fields[4] != "*",
        })
    }

    /// The expression as it was written.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The first firing time strictly after `after`.
    ///
    /// `None` only for an expression that can never fire — `0 0 30 2 *`, the
    /// thirtieth of February — which is reported rather than searched for
    /// forever. The search gives up after four years, which covers a leap day.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        // Start at the next whole minute: a schedule has minute resolution, and
        // "strictly after" must not return the minute we are already in.
        let mut at = (after + Duration::minutes(1))
            .with_second(0)?
            .with_nanosecond(0)?;
        let limit = after + Duration::days(366 * 4);

        while at <= limit {
            if !self.month_matches(at.month()) {
                at = start_of_next_month(at)?;
                continue;
            }
            if !self.day_matches(at) {
                at = start_of_next_day(at)?;
                continue;
            }
            if !bit(self.hours, at.hour()) {
                at = start_of_next_hour(at)?;
                continue;
            }
            if !bit(self.minutes, at.minute()) {
                at += Duration::minutes(1);
                continue;
            }
            return Some(at);
        }
        None
    }

    fn month_matches(&self, month: u32) -> bool {
        bit(self.months, month)
    }

    fn day_matches(&self, at: DateTime<Utc>) -> bool {
        let dom = bit(self.days, at.day());
        let dow = bit(self.weekdays, at.weekday().num_days_from_sunday());
        match (self.day_restricted, self.weekday_restricted) {
            // Vixie's rule: both restricted means either may match.
            (true, true) => dom || dow,
            (true, false) => dom,
            (false, true) => dow,
            (false, false) => true,
        }
    }
}

fn bit(mask: u64, value: u32) -> bool {
    value < 64 && mask & (1u64 << value) != 0
}

fn start_of_next_day(at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let day = at.date_naive().succ_opt()?;
    Some(Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0)?))
}

fn start_of_next_hour(at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    (at + Duration::hours(1)).with_minute(0)?.with_second(0)
}

fn start_of_next_month(at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let (year, month) = if at.month() == 12 {
        (at.year() + 1, 1)
    } else {
        (at.year(), at.month() + 1)
    };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).single()
}

const MONTH_NAMES: &[(&str, u32)] = &[
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

const DAY_NAMES: &[(&str, u32)] = &[
    ("sun", 0),
    ("mon", 1),
    ("tue", 2),
    ("wed", 3),
    ("thu", 4),
    ("fri", 5),
    ("sat", 6),
];

/// One field, as a bitmask.
fn parse_field(field: &str, min: u32, max: u32, names: &[(&str, u32)]) -> Result<u64, String> {
    let mut mask = 0u64;
    for part in field.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("`{field}` has an empty list entry"));
        }
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step
                    .parse()
                    .map_err(|_| format!("`{step}` is not a step in `{field}`"))?;
                if step == 0 {
                    return Err(format!("a step of 0 in `{field}` would never advance"));
                }
                (range, step)
            }
            None => (part, 1),
        };

        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            (value_of(a, min, max, names)?, value_of(b, min, max, names)?)
        } else {
            let v = value_of(range, min, max, names)?;
            // `5/15` means "from 5 onwards, every fifteenth", which is how cron
            // reads a bare value with a step.
            if step > 1 {
                (v, max)
            } else {
                (v, v)
            }
        };

        if lo > hi {
            return Err(format!("`{range}` counts backwards in `{field}`"));
        }
        let mut v = lo;
        while v <= hi {
            mask |= 1u64 << v;
            v += step;
        }
    }
    Ok(mask)
}

fn value_of(text: &str, min: u32, max: u32, names: &[(&str, u32)]) -> Result<u32, String> {
    let text = text.trim();
    let value = if let Some((_, v)) = names
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(text))
    {
        *v
    } else {
        text.parse::<u32>()
            .map_err(|_| format!("`{text}` is not a number"))?
    };
    if value < min || value > max {
        return Err(format!("`{text}` is outside {min}-{max}"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    #[test]
    fn every_minute() {
        let c = Cron::parse("* * * * *").unwrap();
        assert_eq!(
            c.next_after(at(2026, 5, 1, 10, 0)),
            Some(at(2026, 5, 1, 10, 1))
        );
    }

    #[test]
    fn a_step_counts_from_the_start_of_the_range() {
        let c = Cron::parse("*/15 * * * *").unwrap();
        assert_eq!(
            c.next_after(at(2026, 5, 1, 10, 0)),
            Some(at(2026, 5, 1, 10, 15))
        );
        assert_eq!(
            c.next_after(at(2026, 5, 1, 10, 50)),
            Some(at(2026, 5, 1, 11, 0))
        );
    }

    #[test]
    fn a_list_and_a_range() {
        let c = Cron::parse("0 9,17 * * mon-fri").unwrap();
        // Friday 2026-05-01 at 17:00, then Monday the 4th at 09:00.
        assert_eq!(
            c.next_after(at(2026, 5, 1, 10, 0)),
            Some(at(2026, 5, 1, 17, 0))
        );
        assert_eq!(
            c.next_after(at(2026, 5, 1, 18, 0)),
            Some(at(2026, 5, 4, 9, 0))
        );
    }

    #[test]
    fn shorthands_expand() {
        assert_eq!(
            Cron::parse("@daily")
                .unwrap()
                .next_after(at(2026, 5, 1, 10, 0)),
            Some(at(2026, 5, 2, 0, 0))
        );
        assert_eq!(
            Cron::parse("@hourly")
                .unwrap()
                .next_after(at(2026, 5, 1, 10, 30)),
            Some(at(2026, 5, 1, 11, 0))
        );
        assert_eq!(
            Cron::parse("@monthly")
                .unwrap()
                .next_after(at(2026, 5, 15, 10, 0)),
            Some(at(2026, 6, 1, 0, 0))
        );
    }

    #[test]
    fn day_of_month_and_day_of_week_are_an_or_when_both_are_restricted() {
        // The first of the month, or any Monday. 2026-06-01 is itself a Monday,
        // so look at a month where they differ: July 2026 starts on Wednesday.
        let c = Cron::parse("0 0 1 * mon").unwrap();
        assert_eq!(
            c.next_after(at(2026, 6, 30, 12, 0)),
            Some(at(2026, 7, 1, 0, 0)),
            "the first matches on day-of-month alone"
        );
        assert_eq!(
            c.next_after(at(2026, 7, 1, 1, 0)),
            Some(at(2026, 7, 6, 0, 0)),
            "the next Monday matches on day-of-week alone"
        );
    }

    #[test]
    fn sunday_is_both_zero_and_seven() {
        let zero = Cron::parse("0 0 * * 0").unwrap();
        let seven = Cron::parse("0 0 * * 7").unwrap();
        let from = at(2026, 5, 1, 0, 0);
        assert_eq!(zero.next_after(from), seven.next_after(from));
        assert_eq!(zero.next_after(from), Some(at(2026, 5, 3, 0, 0)));
    }

    #[test]
    fn a_leap_day_is_found_rather_than_searched_for_forever() {
        let c = Cron::parse("0 0 29 2 *").unwrap();
        assert_eq!(
            c.next_after(at(2026, 3, 1, 0, 0)),
            Some(at(2028, 2, 29, 0, 0))
        );
    }

    #[test]
    fn a_date_that_cannot_happen_gives_up() {
        let c = Cron::parse("0 0 30 2 *").unwrap();
        assert_eq!(c.next_after(at(2026, 3, 1, 0, 0)), None);
    }

    #[test]
    fn the_next_time_is_strictly_after() {
        let c = Cron::parse("0 * * * *").unwrap();
        let on_the_hour = at(2026, 5, 1, 10, 0);
        assert_eq!(c.next_after(on_the_hour), Some(at(2026, 5, 1, 11, 0)));
    }

    #[test]
    fn bad_expressions_say_what_is_wrong() {
        for (expression, needle) in [
            ("* * * *", "expected 5 fields"),
            ("60 * * * *", "outside 0-59"),
            ("* 25 * * *", "outside 0-23"),
            ("*/0 * * * *", "never advance"),
            ("5-1 * * * *", "counts backwards"),
            ("x * * * *", "not a number"),
            ("0 0 * notamonth *", "not a number"),
        ] {
            let e = Cron::parse(expression).expect_err(expression);
            let text = e.to_string();
            assert!(text.contains(needle), "`{expression}` said `{text}`");
        }
    }

    #[test]
    fn names_are_accepted_for_months_and_days() {
        let c = Cron::parse("0 0 * JAN Sun").unwrap();
        assert_eq!(
            c.next_after(at(2026, 12, 1, 0, 0)),
            Some(at(2027, 1, 3, 0, 0))
        );
    }
}
