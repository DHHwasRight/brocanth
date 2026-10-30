//! CLDR, through ICU4X: plural categories, numbers and dates for one locale.
//!
//! The generated `Locale::formats()` keeps one [`Formats`] per locale in a
//! `static`, built on first use. Building one reads compiled-in data and
//! allocates nothing per request after that.

use icu_decimal::input::Decimal;
use icu_decimal::DecimalFormatter;
use icu_locale_core::Locale;
use icu_plurals::{PluralCategory, PluralRules};

/// A CLDR plural category.
///
/// Which numbers fall in which category is the locale's business: `0` is
/// [`Plural::One`] in French and [`Plural::Other`] in English, and
/// `1000000` is [`Plural::Many`] in French.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Plural {
    Zero,
    One,
    Two,
    Few,
    Many,
    Other,
}

/// How much of a date to write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateLength {
    /// `10/30/26`, `30/10/2026`
    Short,
    /// `Oct 30, 2026`, `30 oct. 2026`
    Medium,
    /// `October 30, 2026`, `30 octobre 2026`
    Long,
}

/// A calendar date, in the Gregorian calendar.
///
/// A plain value, so a catalog's `{when:date}` placeholder has a type that
/// does not change with the `dates` feature. [`Date::new`] refuses a day
/// that does not exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    year: i32,
    month: u8,
    day: u8,
}

impl Date {
    /// `None` for a month outside 1–12 or a day the month does not have.
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        let last = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
            2 => 28,
            _ => return None,
        };
        (1..=last)
            .contains(&day)
            .then_some(Self { year, month, day })
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn month(self) -> u8 {
        self.month
    }

    pub fn day(self) -> u8 {
        self.day
    }
}

/// Plural rules and formatters for one locale.
pub struct Formats {
    tag: &'static str,
    plurals: PluralRules,
    decimal: DecimalFormatter,
    #[cfg(feature = "dates")]
    dates: dates::Cells,
}

impl Formats {
    /// The formats for a BCP 47 tag.
    ///
    /// The build script has already refused a catalog file whose name is not
    /// a tag, so a tag that does not parse here is a bug, and the panic says
    /// which tag. A locale ICU4X has no data for gets its parent's, down to
    /// the root locale, which is CLDR's own fallback and not an error.
    pub fn new(tag: &'static str) -> Self {
        let locale: Locale = tag
            .parse()
            .unwrap_or_else(|e| panic!("`{tag}` is not a BCP 47 tag: {e}"));
        Self {
            tag,
            plurals: PluralRules::try_new_cardinal((&locale).into())
                .unwrap_or_else(|e| panic!("plural rules for `{tag}`: {e}")),
            decimal: DecimalFormatter::try_new((&locale).into(), Default::default())
                .unwrap_or_else(|e| panic!("number format for `{tag}`: {e}")),
            #[cfg(feature = "dates")]
            dates: dates::Cells::new(locale),
        }
    }

    pub fn tag(&self) -> &'static str {
        self.tag
    }

    /// The plural category of `n` in this locale.
    pub fn plural(&self, n: u64) -> Plural {
        match self.plurals.category_for(n) {
            PluralCategory::Zero => Plural::Zero,
            PluralCategory::One => Plural::One,
            PluralCategory::Two => Plural::Two,
            PluralCategory::Few => Plural::Few,
            PluralCategory::Many => Plural::Many,
            PluralCategory::Other => Plural::Other,
        }
    }

    /// An integer with this locale's grouping: `1,234,567`, `1 234 567`.
    pub fn number(&self, n: i64) -> String {
        self.decimal.format(&Decimal::from(n)).to_string()
    }

    /// A count, which is what a plural message's `{count}` is.
    pub fn count(&self, n: u64) -> String {
        self.decimal.format(&Decimal::from(n)).to_string()
    }

    /// A fixed-point number: `fixed(123456, 2)` is `1,234.56` or `1 234,56`.
    ///
    /// An integer and a scale rather than an `f64`, so an amount of money
    /// never goes through a binary fraction on its way to the screen.
    pub fn fixed(&self, units: i64, scale: u8) -> String {
        let mut d = Decimal::from(units);
        d.multiply_pow10(-i16::from(scale));
        // `multiply_pow10` keeps trailing zeros as written: `1050, 2` stays
        // `10.50`, which is what a price needs.
        self.decimal.format(&d).to_string()
    }

    /// A date in this locale.
    #[cfg(feature = "dates")]
    pub fn date(&self, date: Date, length: DateLength) -> String {
        self.dates.format(self.tag, date, length)
    }
}

impl std::fmt::Debug for Formats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Formats").field("tag", &self.tag).finish()
    }
}

#[cfg(feature = "dates")]
mod dates {
    use std::sync::OnceLock;

    use icu_calendar::{Date as IcuDate, Gregorian};
    use icu_datetime::fieldsets::YMD;
    use icu_datetime::FixedCalendarDateTimeFormatter;
    use icu_locale_core::Locale;

    use super::{Date, DateLength};

    type Formatter = FixedCalendarDateTimeFormatter<Gregorian, YMD>;

    /// One formatter per length, each built the first time it is asked for.
    /// Gregorian only, which keeps every other calendar's data out of the
    /// binary.
    pub(super) struct Cells {
        locale: Locale,
        short: OnceLock<Formatter>,
        medium: OnceLock<Formatter>,
        long: OnceLock<Formatter>,
    }

    impl Cells {
        pub(super) fn new(locale: Locale) -> Self {
            Self {
                locale,
                short: OnceLock::new(),
                medium: OnceLock::new(),
                long: OnceLock::new(),
            }
        }

        pub(super) fn format(&self, tag: &str, date: Date, length: DateLength) -> String {
            let (cell, fields) = match length {
                DateLength::Short => (&self.short, YMD::short()),
                DateLength::Medium => (&self.medium, YMD::medium()),
                DateLength::Long => (&self.long, YMD::long()),
            };
            let formatter = cell.get_or_init(|| {
                Formatter::try_new((&self.locale).into(), fields)
                    .unwrap_or_else(|e| panic!("date format for `{tag}`: {e}"))
            });
            // `Date::new` already refused an impossible day, so this cannot
            // fail for a value of our type.
            let date = IcuDate::try_new_gregorian(date.year, date.month, date.day)
                .expect("Date::new validated the day");
            formatter.format(&date).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn february_has_a_leap_day_only_in_a_leap_year() {
        assert!(Date::new(2024, 2, 29).is_some());
        assert!(Date::new(2026, 2, 29).is_none());
        assert!(Date::new(1900, 2, 29).is_none());
        assert!(Date::new(2000, 2, 29).is_some());
        assert!(Date::new(2026, 13, 1).is_none());
        assert!(Date::new(2026, 4, 31).is_none());
    }

    #[test]
    fn zero_is_singular_in_french_and_plural_in_english() {
        let en = Formats::new("en");
        let fr = Formats::new("fr");
        assert_eq!(en.plural(0), Plural::Other);
        assert_eq!(fr.plural(0), Plural::One);
        assert_eq!(en.plural(1), Plural::One);
        assert_eq!(fr.plural(1), Plural::One);
        assert_eq!(en.plural(2), Plural::Other);
        assert_eq!(fr.plural(2), Plural::Other);
        assert_eq!(fr.plural(1_000_000), Plural::Many);
    }

    #[test]
    fn numbers_are_grouped_the_way_each_locale_groups_them() {
        assert_eq!(Formats::new("en").number(-1_234_567), "-1,234,567");
        // French groups with a narrow no-break space, U+202F.
        assert_eq!(
            Formats::new("fr").number(1_234_567),
            "1\u{202f}234\u{202f}567"
        );
        assert_eq!(Formats::new("en").fixed(123_456, 2), "1,234.56");
        assert_eq!(Formats::new("fr").fixed(1050, 2), "10,50");
    }

    #[cfg(feature = "dates")]
    #[test]
    fn dates_follow_the_locale() {
        let day = Date::new(2026, 10, 30).expect("a real day");
        let en = Formats::new("en");
        let fr = Formats::new("fr");
        assert_eq!(en.date(day, DateLength::Long), "October 30, 2026");
        assert_eq!(fr.date(day, DateLength::Long), "30 octobre 2026");
        assert_eq!(en.date(day, DateLength::Medium), "Oct 30, 2026");
        assert_eq!(fr.date(day, DateLength::Medium), "30 oct. 2026");
        assert_eq!(en.date(day, DateLength::Short), "10/30/26");
        assert_eq!(fr.date(day, DateLength::Short), "30/10/2026");
    }
}
