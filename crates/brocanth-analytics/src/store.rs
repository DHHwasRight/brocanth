//! [`Store`]: where page views go, and the reports that come back.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

use crate::day::Day;

/// One page view, as it is stored.
///
/// Every field is coarse on purpose. There is no address, no user agent,
/// no time of day, no cookie and no account: [`visitor`](View::visitor) is
/// a hash that changes every day and cannot be reversed once that day's
/// salt is deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    /// The site name, from the settings.
    pub site: String,
    /// The UTC day of the visit.
    pub day: Day,
    /// The visitor of that day: 8 bytes of `SHA-256(salt of the day ‖
    /// SHA-256(site ‖ address ‖ user agent))`. Equal for two views of one
    /// visitor on one day, unrelated across days.
    pub visitor: i64,
    /// The path, without its query. See [`crate::classify::clean_path`].
    pub path: String,
    /// The allowed query parameters, or empty.
    pub query: String,
    /// The origin the visitor came from, or empty.
    pub referrer: String,
    /// The browser family.
    pub browser: String,
    /// The system family.
    pub os: String,
    /// `desktop`, `mobile` or `tablet`.
    pub device: String,
    /// A two-letter country code, or empty.
    pub country: String,
}

/// A store failed. The message is the store's own; a request never sees
/// it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreError(pub String);

impl StoreError {
    /// An error with this message.
    pub fn new(message: impl Into<String>) -> Self {
        StoreError(message.into())
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// One line of a breakdown: a page, a referrer, a browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// What is counted: `/pricing`, `https://news.example`, `Firefox`.
    pub label: String,
    /// Page views.
    pub views: i64,
    /// Visitors, counted once per day: see [`Report::visitors`].
    pub visitors: i64,
}

/// What a breakdown is grouped by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    /// The stored path.
    Path,
    /// The referrer origin. Views with none are left out.
    Referrer,
    /// The browser family.
    Browser,
    /// The system family.
    Os,
    /// The device kind.
    Device,
    /// The country. Views with none are left out.
    Country,
}

impl Dimension {
    /// Every dimension, in the order a dashboard shows them.
    pub const ALL: [Dimension; 6] = [
        Dimension::Path,
        Dimension::Referrer,
        Dimension::Browser,
        Dimension::Os,
        Dimension::Device,
        Dimension::Country,
    ];

    /// The column name, which is also the label of the breakdown.
    pub fn column(self) -> &'static str {
        match self {
            Dimension::Path => "path",
            Dimension::Referrer => "referrer",
            Dimension::Browser => "browser",
            Dimension::Os => "os",
            Dimension::Device => "device",
            Dimension::Country => "country",
        }
    }

    fn of(self, view: &View) -> &str {
        match self {
            Dimension::Path => &view.path,
            Dimension::Referrer => &view.referrer,
            Dimension::Browser => &view.browser,
            Dimension::Os => &view.os,
            Dimension::Device => &view.device,
            Dimension::Country => &view.country,
        }
    }
}

/// The numbers for one site over a range of days, both ends included.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Page views.
    pub views: i64,
    /// Visitors, summed day by day.
    ///
    /// The hash changes at midnight, so one person on two days is two
    /// visitors. That is the price of a hash nobody can follow across
    /// days, and every counter built this way pays it.
    pub visitors: i64,
    /// Each day of the range that had a view: `(day, views, visitors)`.
    pub days: Vec<(Day, i64, i64)>,
    /// Breakdowns, one per [`Dimension`], most views first, ten lines at
    /// most each.
    pub breakdowns: Vec<(Dimension, Vec<Line>)>,
}

impl Report {
    /// The breakdown by `dimension`, or an empty one.
    pub fn breakdown(&self, dimension: Dimension) -> &[Line] {
        self.breakdowns
            .iter()
            .find(|(d, _)| *d == dimension)
            .map_or(&[], |(_, lines)| lines.as_slice())
    }
}

/// How many lines a breakdown keeps.
pub const TOP: usize = 10;

/// Where page views are kept.
///
/// [`MemoryStore`] for tests and a process that does not need its numbers
/// after a restart; `PgStore` (feature `postgres`) for everything else.
#[async_trait]
pub trait Store: Send + Sync {
    /// The salt of `day`, made on first use.
    ///
    /// Shared by every instance of the application through the store, so
    /// two instances behind one load balancer count a visitor once.
    async fn salt(&self, day: Day) -> Result<[u8; 32], StoreError>;

    /// Keeps these views.
    async fn write(&self, views: &[View]) -> Result<(), StoreError>;

    /// Deletes every salt of a day before `today`, and every view of a day
    /// before `keep_from` (`None` keeps every view).
    ///
    /// Deleting the salt is what makes yesterday's hashes anonymous: without
    /// it, nobody can recompute a hash from an address to test a guess.
    async fn forget(&self, today: Day, keep_from: Option<Day>) -> Result<(), StoreError>;

    /// The report for `site` from `from` to `to`, both included.
    async fn report(&self, site: &str, from: Day, to: Day) -> Result<Report, StoreError>;

    /// Every site that has a row, sorted.
    async fn sites(&self) -> Result<Vec<String>, StoreError>;
}

/// A store in memory: gone at restart.
///
/// The salts live here too, so a restart also starts a new salt: a visitor
/// before and after a restart on the same day counts twice.
#[derive(Default)]
pub struct MemoryStore {
    state: Mutex<State>,
    failing: AtomicBool,
}

#[derive(Default)]
struct State {
    salts: HashMap<Day, [u8; 32]>,
    views: Vec<View>,
}

impl MemoryStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every view kept, in write order. For a test that asserts on what
    /// was stored.
    pub fn views(&self) -> Vec<View> {
        self.lock().views.clone()
    }

    /// The days that still have a salt.
    pub fn salt_days(&self) -> Vec<Day> {
        let mut days: Vec<Day> = self.lock().salts.keys().copied().collect();
        days.sort();
        days
    }

    /// Makes every [`Store::write`] fail until called again with `false`.
    /// For the test that a failed write never fails a request.
    pub fn fail_writes(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[async_trait]
impl Store for MemoryStore {
    async fn salt(&self, day: Day) -> Result<[u8; 32], StoreError> {
        let mut state = self.lock();
        if let Some(salt) = state.salts.get(&day) {
            return Ok(*salt);
        }
        let salt = crate::recorder::fresh_salt()?;
        state.salts.insert(day, salt);
        Ok(salt)
    }

    async fn write(&self, views: &[View]) -> Result<(), StoreError> {
        if self.failing.load(Ordering::SeqCst) {
            return Err(StoreError::new("the memory store was told to fail"));
        }
        self.lock().views.extend_from_slice(views);
        Ok(())
    }

    async fn forget(&self, today: Day, keep_from: Option<Day>) -> Result<(), StoreError> {
        let mut state = self.lock();
        state.salts.retain(|day, _| *day >= today);
        if let Some(keep_from) = keep_from {
            state.views.retain(|view| view.day >= keep_from);
        }
        Ok(())
    }

    async fn report(&self, site: &str, from: Day, to: Day) -> Result<Report, StoreError> {
        let state = self.lock();
        let views: Vec<&View> = state
            .views
            .iter()
            .filter(|v| v.site == site && v.day >= from && v.day <= to)
            .collect();
        Ok(summarise(&views))
    }

    async fn sites(&self) -> Result<Vec<String>, StoreError> {
        let mut sites: Vec<String> = self.lock().views.iter().map(|v| v.site.clone()).collect();
        sites.sort();
        sites.dedup();
        Ok(sites)
    }
}

/// The report over views already filtered to one site and one range.
pub(crate) fn summarise(views: &[&View]) -> Report {
    let unique = |views: &[&View]| {
        views
            .iter()
            .map(|v| (v.day, v.visitor))
            .collect::<HashSet<_>>()
            .len() as i64
    };
    let mut by_day: HashMap<Day, Vec<&View>> = HashMap::new();
    for view in views {
        by_day.entry(view.day).or_default().push(view);
    }
    let mut days: Vec<(Day, i64, i64)> = by_day
        .iter()
        .map(|(day, views)| (*day, views.len() as i64, unique(views)))
        .collect();
    days.sort_by_key(|(day, _, _)| *day);
    let breakdowns = Dimension::ALL
        .iter()
        .map(|&dimension| {
            let mut groups: HashMap<&str, Vec<&View>> = HashMap::new();
            for view in views {
                let label = dimension.of(view);
                if !label.is_empty() {
                    groups.entry(label).or_default().push(view);
                }
            }
            let mut lines: Vec<Line> = groups
                .into_iter()
                .map(|(label, views)| Line {
                    label: label.to_string(),
                    views: views.len() as i64,
                    visitors: unique(&views),
                })
                .collect();
            lines.sort_by(|a, b| b.views.cmp(&a.views).then_with(|| a.label.cmp(&b.label)));
            lines.truncate(TOP);
            (dimension, lines)
        })
        .collect();
    Report {
        views: views.len() as i64,
        visitors: unique(views),
        days,
        breakdowns,
    }
}
