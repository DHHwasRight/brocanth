#![doc = include_str!("../README.md")]
//!
//! # Decisions
//!
//! - **Sizes are decimal (SI).** `1 kB` is 1000 bytes, as a disk vendor, a
//!   network and macOS count them. A page that shows an upload limit and a
//!   file's size must use one unit, and SI is the one a reader's other
//!   tools show. `KiB` is not offered: two units on one site is the bug.
//! - **A time is a whole unit, rounded down.** `90` seconds is "1 minute
//!   ago", not "2 minutes ago": a reader who sees "2 minutes" expects at
//!   least two. Under a minute is "just now", because a seconds count is
//!   stale by the time the page is read.
//! - **A month is 30 days and a year is 365.** The phrase is approximate by
//!   design; a calendar-exact value belongs in a date.
//! - **Four languages, then English.** The phrases are tables in this crate
//!   for `en`, `fr`, `de` and `es`. A tag in another language gets English,
//!   never a key or an empty string.

#[cfg(feature = "scaffold")]
pub mod scaffold;

use std::time::SystemTime;

/// A language this crate has phrases for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Lang {
    /// English, and the fallback for any other tag.
    En,
    /// French.
    Fr,
    /// German.
    De,
    /// Spanish.
    Es,
}

impl Lang {
    /// The language of a BCP 47 tag, by its first subtag: `fr-CA` is
    /// [`Lang::Fr`]. A language with no table here is [`Lang::En`].
    pub fn from_tag(tag: &str) -> Self {
        let primary = tag.split(['-', '_']).next().unwrap_or_default();
        match primary.to_ascii_lowercase().as_str() {
            "fr" => Lang::Fr,
            "de" => Lang::De,
            "es" => Lang::Es,
            _ => Lang::En,
        }
    }
}

/// The helpers, for one language.
///
/// Cheap to make and to copy: make one per request with
/// [`Humanize::for_tag`], or with [`Humanize::with_formats`] when the
/// project has the `i18n` module.
#[derive(Clone, Copy, Debug)]
pub struct Humanize {
    lang: Lang,
    #[cfg(feature = "i18n")]
    formats: Option<&'static brocanth_i18n::Formats>,
}

/// A unit of time, longest last.
#[derive(Clone, Copy)]
enum Unit {
    Minute,
    Hour,
    Day,
    Month,
    Year,
}

const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;
const MONTH: u64 = 30 * DAY;
const YEAR: u64 = 365 * DAY;

impl Humanize {
    /// The helpers in `lang`.
    pub const fn new(lang: Lang) -> Self {
        Self {
            lang,
            #[cfg(feature = "i18n")]
            formats: None,
        }
    }

    /// The helpers in the language of a BCP 47 tag. See [`Lang::from_tag`].
    pub fn for_tag(tag: &str) -> Self {
        Self::new(Lang::from_tag(tag))
    }

    /// The helpers in the language of an `i18n` locale. Numbers are grouped
    /// by its CLDR formats, so a page groups them as its catalog does.
    ///
    /// ```ignore
    /// let h = Humanize::with_formats(crate::plugins::i18n::locale(ctx).formats());
    /// ```
    #[cfg(feature = "i18n")]
    pub fn with_formats(formats: &'static brocanth_i18n::Formats) -> Self {
        Self {
            lang: Lang::from_tag(formats.tag()),
            formats: Some(formats),
        }
    }

    /// The language of the phrases.
    pub fn lang(&self) -> Lang {
        self.lang
    }

    /// How long ago, from a number of seconds: `180` is "3 minutes ago",
    /// `-7200` is "in 2 hours". Under a minute either way is "just now".
    pub fn ago(&self, seconds: i64) -> String {
        let future = seconds < 0;
        let span = seconds.unsigned_abs();
        if span < MINUTE {
            return self.just_now().to_string();
        }
        let (count, unit) = if span < HOUR {
            (span / MINUTE, Unit::Minute)
        } else if span < DAY {
            (span / HOUR, Unit::Hour)
        } else if span < MONTH {
            (span / DAY, Unit::Day)
        } else if span < YEAR {
            (span / MONTH, Unit::Month)
        } else {
            (span / YEAR, Unit::Year)
        };
        self.relative(count, unit, future)
    }

    /// How long from `then` to `now`, as [`Humanize::ago`] says it. A
    /// `then` after `now` is in the future.
    pub fn since(&self, then: SystemTime, now: SystemTime) -> String {
        let seconds = match now.duration_since(then) {
            Ok(past) => i64::try_from(past.as_secs()).unwrap_or(i64::MAX),
            Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
        };
        self.ago(seconds)
    }

    /// A size in bytes, in decimal units: `1500000` is "1.5 MB". One
    /// decimal, and none when it is zero: "2 kB", not "2.0 kB".
    pub fn file_size(&self, bytes: u64) -> String {
        let units: [&str; 6] = match self.lang {
            Lang::Fr => ["o", "ko", "Mo", "Go", "To", "Po"],
            _ => ["B", "kB", "MB", "GB", "TB", "PB"],
        };
        // French puts a no-break space before a unit; the others a space.
        let gap = if self.lang == Lang::Fr { '\u{a0}' } else { ' ' };
        if bytes < 1000 {
            return format!("{}{gap}{}", self.number(bytes as i64), units[0]);
        }
        // Tenths of the unit, rounded half up, in integers so 999 950 bytes
        // is "1 MB" and never "1000 kB".
        let mut unit = 0;
        let mut scale: u128 = 1;
        let mut tenths: u128;
        loop {
            unit += 1;
            scale *= 1000;
            tenths = (u128::from(bytes) * 10 + scale / 2) / scale;
            if tenths < 10_000 || unit == units.len() - 1 {
                break;
            }
        }
        let whole = tenths / 10;
        let tenth = tenths % 10;
        let comma = matches!(self.lang, Lang::Fr | Lang::De | Lang::Es);
        let value = if tenth == 0 {
            whole.to_string()
        } else if comma {
            format!("{whole},{tenth}")
        } else {
            format!("{whole}.{tenth}")
        };
        format!("{value}{gap}{}", units[unit])
    }

    /// An ordinal number: "1st", "2nd", "11th" in English, "1er", "2e" in
    /// French, "1." in German, "1.º" in Spanish.
    pub fn ordinal(&self, n: i64) -> String {
        let abs = n.unsigned_abs();
        let suffix = match self.lang {
            Lang::En => match (abs % 10, abs % 100) {
                (_, 11..=13) => "th",
                (1, _) => "st",
                (2, _) => "nd",
                (3, _) => "rd",
                _ => "th",
            },
            Lang::Fr => {
                if abs == 1 {
                    "er"
                } else {
                    "e"
                }
            }
            Lang::De => ".",
            Lang::Es => ".º",
        };
        format!("{n}{suffix}")
    }

    /// An integer with the language's grouping: "1,234,567" in English,
    /// "1 234 567" in French (a narrow no-break space), "1.234.567" in
    /// German and Spanish. Spanish leaves a four-digit number whole.
    ///
    /// With [`Humanize::with_formats`], CLDR groups it instead.
    pub fn number(&self, n: i64) -> String {
        #[cfg(feature = "i18n")]
        if let Some(formats) = self.formats {
            return formats.number(n);
        }
        let digits = n.unsigned_abs().to_string();
        let (separator, minimum) = match self.lang {
            Lang::En => (",", 4),
            Lang::Fr => ("\u{202f}", 4),
            Lang::De => (".", 4),
            // CLDR's minimum grouping digits for Spanish is 2: 1234, 12.345.
            Lang::Es => (".", 5),
        };
        let grouped = if digits.len() < minimum {
            digits
        } else {
            let mut out = String::with_capacity(digits.len() + digits.len() / 3 * 3);
            for (i, c) in digits.chars().enumerate() {
                if i > 0 && (digits.len() - i).is_multiple_of(3) {
                    out.push_str(separator);
                }
                out.push(c);
            }
            out
        };
        if n < 0 {
            format!("-{grouped}")
        } else {
            grouped
        }
    }

    fn just_now(&self) -> &'static str {
        match self.lang {
            Lang::En => "just now",
            Lang::Fr => "à l’instant",
            Lang::De => "gerade eben",
            Lang::Es => "ahora mismo",
        }
    }

    fn relative(&self, count: u64, unit: Unit, future: bool) -> String {
        let n = self.number(count as i64);
        // French counts 0 and 1 as singular; the others only 1. `count` is
        // never 0 here.
        let one = count == 1;
        match self.lang {
            Lang::En => {
                let word = match unit {
                    Unit::Minute => "minute",
                    Unit::Hour => "hour",
                    Unit::Day => "day",
                    Unit::Month => "month",
                    Unit::Year => "year",
                };
                let s = if one { "" } else { "s" };
                if future {
                    format!("in {n} {word}{s}")
                } else {
                    format!("{n} {word}{s} ago")
                }
            }
            Lang::Fr => {
                let word = match (unit, one) {
                    (Unit::Minute, true) => "minute",
                    (Unit::Minute, false) => "minutes",
                    (Unit::Hour, true) => "heure",
                    (Unit::Hour, false) => "heures",
                    (Unit::Day, true) => "jour",
                    (Unit::Day, false) => "jours",
                    (Unit::Month, _) => "mois",
                    (Unit::Year, true) => "an",
                    (Unit::Year, false) => "ans",
                };
                if future {
                    format!("dans {n} {word}")
                } else {
                    format!("il y a {n} {word}")
                }
            }
            Lang::De => {
                // "vor" takes the dative, so a plural day is "Tagen" there and
                // "Tagen" after "in" too; the singular has no ending.
                let word = match (unit, one) {
                    (Unit::Minute, true) => "Minute",
                    (Unit::Minute, false) => "Minuten",
                    (Unit::Hour, true) => "Stunde",
                    (Unit::Hour, false) => "Stunden",
                    (Unit::Day, true) => "Tag",
                    (Unit::Day, false) => "Tagen",
                    (Unit::Month, true) => "Monat",
                    (Unit::Month, false) => "Monaten",
                    (Unit::Year, true) => "Jahr",
                    (Unit::Year, false) => "Jahren",
                };
                if future {
                    format!("in {n} {word}")
                } else {
                    format!("vor {n} {word}")
                }
            }
            Lang::Es => {
                let word = match (unit, one) {
                    (Unit::Minute, true) => "minuto",
                    (Unit::Minute, false) => "minutos",
                    (Unit::Hour, true) => "hora",
                    (Unit::Hour, false) => "horas",
                    (Unit::Day, true) => "día",
                    (Unit::Day, false) => "días",
                    (Unit::Month, true) => "mes",
                    (Unit::Month, false) => "meses",
                    (Unit::Year, true) => "año",
                    (Unit::Year, false) => "años",
                };
                if future {
                    format!("dentro de {n} {word}")
                } else {
                    format!("hace {n} {word}")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn en() -> Humanize {
        Humanize::new(Lang::En)
    }

    #[test]
    fn a_time_is_a_whole_unit_rounded_down() {
        let h = en();
        assert_eq!(h.ago(0), "just now");
        assert_eq!(h.ago(59), "just now");
        assert_eq!(h.ago(-30), "just now");
        assert_eq!(h.ago(60), "1 minute ago");
        assert_eq!(h.ago(119), "1 minute ago");
        assert_eq!(h.ago(180), "3 minutes ago");
        assert_eq!(h.ago(3600), "1 hour ago");
        assert_eq!(h.ago(-7200), "in 2 hours");
        assert_eq!(h.ago(86_400 * 3), "3 days ago");
        assert_eq!(h.ago(86_400 * 45), "1 month ago");
        assert_eq!(h.ago(86_400 * 800), "2 years ago");
        assert_eq!(h.ago(i64::MIN), "in 292,471,208,677 years");
    }

    #[test]
    fn each_language_has_its_own_phrases_and_plurals() {
        let fr = Humanize::for_tag("fr-CA");
        assert_eq!(fr.ago(60), "il y a 1 minute");
        assert_eq!(fr.ago(86_400 * 60), "il y a 2 mois");
        assert_eq!(fr.ago(-86_400 * 400), "dans 1 an");
        let de = Humanize::for_tag("de");
        assert_eq!(de.ago(86_400 * 3), "vor 3 Tagen");
        assert_eq!(de.ago(-3600), "in 1 Stunde");
        let es = Humanize::for_tag("es_MX");
        assert_eq!(es.ago(86_400 * 31), "hace 1 mes");
        assert_eq!(es.ago(-600), "dentro de 10 minutos");
        assert_eq!(Humanize::for_tag("ja").ago(120), "2 minutes ago");
    }

    #[test]
    fn since_reads_two_instants_either_way() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let then = now - Duration::from_secs(300);
        assert_eq!(en().since(then, now), "5 minutes ago");
        assert_eq!(en().since(now, then), "in 5 minutes");
    }

    #[test]
    fn a_size_is_decimal_with_one_digit_and_no_trailing_zero() {
        let h = en();
        assert_eq!(h.file_size(0), "0 B");
        assert_eq!(h.file_size(999), "999 B");
        assert_eq!(h.file_size(1000), "1 kB");
        assert_eq!(h.file_size(1500), "1.5 kB");
        assert_eq!(h.file_size(1_500_000), "1.5 MB");
        assert_eq!(h.file_size(999_949), "999.9 kB");
        assert_eq!(h.file_size(999_950), "1 MB");
        assert_eq!(h.file_size(u64::MAX), "18446.7 PB");
        assert_eq!(Humanize::for_tag("fr").file_size(2_500), "2,5\u{a0}ko");
        assert_eq!(Humanize::for_tag("de").file_size(2_500_000_000), "2,5 GB");
    }

    #[test]
    fn ordinals_follow_each_language() {
        let h = en();
        let got: Vec<String> = [1, 2, 3, 4, 11, 12, 13, 21, 22, 101, 111, -1]
            .iter()
            .map(|&n| h.ordinal(n))
            .collect();
        assert_eq!(
            got,
            [
                "1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "101st",
                "111th", "-1st"
            ]
        );
        assert_eq!(Humanize::for_tag("fr").ordinal(1), "1er");
        assert_eq!(Humanize::for_tag("fr").ordinal(2), "2e");
        assert_eq!(Humanize::for_tag("de").ordinal(3), "3.");
        assert_eq!(Humanize::for_tag("es").ordinal(5), "5.º");
    }

    #[test]
    fn numbers_are_grouped_by_three_with_the_language_separator() {
        assert_eq!(en().number(0), "0");
        assert_eq!(en().number(999), "999");
        assert_eq!(en().number(1234), "1,234");
        assert_eq!(en().number(-1_234_567), "-1,234,567");
        assert_eq!(
            Humanize::for_tag("fr").number(1234567),
            "1\u{202f}234\u{202f}567"
        );
        assert_eq!(Humanize::for_tag("de").number(1234567), "1.234.567");
        assert_eq!(Humanize::for_tag("es").number(1234), "1234");
        assert_eq!(Humanize::for_tag("es").number(12345), "12.345");
    }

    #[cfg(feature = "i18n")]
    #[test]
    fn with_i18n_formats_cldr_groups_the_numbers() {
        let formats: &'static brocanth_i18n::Formats =
            Box::leak(Box::new(brocanth_i18n::Formats::new("fr")));
        let h = Humanize::with_formats(formats);
        assert_eq!(h.lang(), Lang::Fr);
        assert_eq!(h.number(1234567), formats.number(1234567));
        assert_eq!(h.ago(180), format!("il y a {} minutes", formats.number(3)));
    }
}
