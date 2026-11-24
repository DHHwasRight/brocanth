//! A calendar date: what a post's `date` is.
//!
//! Three numbers and no time zone, because a post is dated by its author's
//! calendar, not by an instant. The feed writes midnight UTC, which every
//! reader takes. No `chrono`, `time` or `jiff`: nothing here adds, subtracts
//! or converts a date.

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;

/// A calendar date, from `2026-11-22`.
///
/// In TOML front matter it is a bare date, `date = 2026-11-22`, or the
/// same text in quotes. A time is refused: a post is dated by day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}

impl Date {
    /// The date, or `None` when the day does not exist in that month.
    pub fn new(year: u16, month: u8, day: u8) -> Option<Self> {
        let leap =
            (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400);
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        (1..=days)
            .contains(&day)
            .then_some(Self { year, month, day })
    }

    pub fn year(&self) -> u16 {
        self.year
    }

    pub fn month(&self) -> u8 {
        self.month
    }

    pub fn day(&self) -> u8 {
        self.day
    }

    /// Midnight UTC on this date, as RFC 3339: `2026-11-22T00:00:00Z`.
    /// What an Atom feed writes.
    pub fn rfc3339(&self) -> String {
        format!("{self}T00:00:00Z")
    }
}

impl fmt::Display for Date {
    /// `2026-11-22`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

impl FromStr for Date {
    type Err = String;

    /// `YYYY-MM-DD`, and nothing after it.
    fn from_str(text: &str) -> Result<Self, String> {
        let bad = || format!("`{text}` is not a date: write it as YYYY-MM-DD, such as 2026-11-22");
        let mut parts = text.split('-');
        let (Some(y), Some(m), Some(d), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(bad());
        };
        if y.len() != 4 || m.len() != 2 || d.len() != 2 {
            return Err(bad());
        }
        let number = |part: &str| part.parse::<u16>().map_err(|_| bad());
        let (year, month, day) = (number(y)?, number(m)?, number(d)?);
        Date::new(year, month as u8, day as u8)
            .ok_or_else(|| format!("{text} is not a day of the calendar"))
    }
}

impl<'de> Deserialize<'de> for Date {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(DateVisitor)
    }
}

struct DateVisitor;

impl<'de> Visitor<'de> for DateVisitor {
    type Value = Date;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a date, such as 2026-11-22")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Date, E> {
        text.parse().map_err(E::custom)
    }

    /// A TOML date arrives as a map with one private key; `toml` reads it.
    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Date, A::Error> {
        let datetime =
            toml::value::Datetime::deserialize(de::value::MapAccessDeserializer::new(map))?;
        match (datetime.date, datetime.time) {
            (Some(date), None) => Date::new(date.year, date.month, date.day).ok_or_else(|| {
                de::Error::custom(format!("{datetime} is not a day of the calendar"))
            }),
            _ => Err(de::Error::custom(format!(
                "`{datetime}` has a time; a date is the day alone, such as 2026-11-22"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Front {
        date: Date,
    }

    #[test]
    fn a_toml_date_and_a_quoted_one_both_read() {
        let bare: Front = toml::from_str("date = 2026-11-22").unwrap();
        let quoted: Front = toml::from_str("date = \"2026-11-22\"").unwrap();
        assert_eq!(bare.date, quoted.date);
        assert_eq!(bare.date.to_string(), "2026-11-22");
        assert_eq!(bare.date.rfc3339(), "2026-11-22T00:00:00Z");
    }

    #[test]
    fn a_time_and_an_impossible_day_are_refused() {
        let timed = toml::from_str::<Front>("date = 2026-11-22T10:00:00Z")
            .err()
            .unwrap();
        assert!(timed.to_string().contains("has a time"), "{timed}");
        assert!(toml::from_str::<Front>("date = \"2026-02-30\"").is_err());
        assert!(toml::from_str::<Front>("date = \"22/11/2026\"").is_err());
    }

    #[test]
    fn dates_order_by_the_calendar() {
        let a: Date = "2025-12-31".parse().unwrap();
        let b: Date = "2026-01-01".parse().unwrap();
        assert!(a < b);
        assert!(Date::new(2024, 2, 29).is_some());
        assert!(Date::new(2025, 2, 29).is_none());
    }
}
