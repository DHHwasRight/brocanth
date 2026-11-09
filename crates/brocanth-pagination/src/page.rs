//! Offset pages: a number, a size and a total.
//!
//! Small, and worth its own file because every off-by-one a list can have
//! lives here: an empty table that reports "page 1 of 0", a last page that
//! offers a next link, and an exact multiple that renders one empty page past
//! the end. [`Page`] clamps on construction, so a handler cannot carry an
//! out-of-range page number any further than this type.
//!
//! This was `brocanth_admin::paging::Page`. The admin re-exports it.

/// One page of a listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    number: u64,
    per_page: u64,
    total: u64,
}

impl Page {
    /// Builds a page, clamping the request into range.
    ///
    /// `per_page` of zero becomes one, a number below one becomes one, and a
    /// number past the end becomes the last page. An empty table has one page,
    /// which is the page that says it is empty.
    pub fn new(number: u64, per_page: u64, total: u64) -> Self {
        let per_page = per_page.max(1);
        let total_pages = total.div_ceil(per_page).max(1);
        Page {
            number: number.clamp(1, total_pages),
            per_page,
            total,
        }
    }

    /// The current page, counting from one.
    pub fn number(&self) -> u64 {
        self.number
    }

    /// Rows per page.
    pub fn per_page(&self) -> u64 {
        self.per_page
    }

    /// Rows in total, across every page.
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Pages in total. Never zero.
    pub fn total_pages(&self) -> u64 {
        self.total.div_ceil(self.per_page).max(1)
    }

    /// Rows to skip to reach this page.
    pub fn offset(&self) -> u64 {
        (self.number - 1) * self.per_page
    }

    /// Rows to read for this page.
    pub fn limit(&self) -> u64 {
        self.per_page
    }

    /// Whether this is the first page.
    pub fn is_first(&self) -> bool {
        self.number == 1
    }

    /// Whether this is the last page.
    pub fn is_last(&self) -> bool {
        self.number == self.total_pages()
    }

    /// Whether there are no rows at all.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// The previous page number, when there is one.
    pub fn previous(&self) -> Option<u64> {
        (!self.is_first()).then(|| self.number - 1)
    }

    /// The next page number, when there is one.
    pub fn next(&self) -> Option<u64> {
        (!self.is_last()).then(|| self.number + 1)
    }

    /// The one-based inclusive row range this page shows, or `None` when the
    /// table is empty.
    pub fn range(&self) -> Option<(u64, u64)> {
        if self.total == 0 {
            return None;
        }
        let first = self.offset() + 1;
        // Saturating: offset and size can each exceed half of `u64`.
        let last = self.offset().saturating_add(self.per_page).min(self.total);
        Some((first, last))
    }

    /// Up to `span` page numbers centred on the current one, clamped to the
    /// range that exists. `span` of zero yields the current page alone.
    pub fn window(&self, span: u64) -> Vec<u64> {
        let span = span.max(1);
        let total_pages = self.total_pages();
        if total_pages <= span {
            return (1..=total_pages).collect();
        }
        let half = span / 2;
        let mut first = self.number.saturating_sub(half).max(1);
        // Inclusive ends, and a checked add: `first + span` overflows when
        // the last page is `u64::MAX`. Past the end, the window ends on the
        // last page, which `total_pages > span` keeps above zero.
        let last = match first.checked_add(span - 1) {
            Some(last) if last <= total_pages => last,
            _ => total_pages,
        };
        first = last - (span - 1);
        (first..=last).collect()
    }

    /// `1–25 of 95 · page 1 of 4`, or `Nothing to show` for an empty table.
    pub fn summary(&self) -> String {
        match self.range() {
            Some((first, last)) => format!(
                "{first}–{last} of {total} · page {number} of {pages}",
                total = self.total,
                number = self.number,
                pages = self.total_pages()
            ),
            None => "Nothing to show".to_string(),
        }
    }
}

/// How many rows a page may hold: a default, and a ceiling.
///
/// The ceiling is applied, not refused: a client that asks for a million
/// rows gets the maximum, which is what every paginated API does, and a
/// refusal would make a harmless mistake fatal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Rows per page when the request names no size.
    pub default: u64,
    /// The most rows a page may hold.
    pub max: u64,
}

impl Default for Limits {
    /// 25 rows, at most 100.
    fn default() -> Self {
        Limits {
            default: 25,
            max: 100,
        }
    }
}

impl Limits {
    /// A default and a ceiling. A ceiling below the default is raised to it.
    pub fn new(default: u64, max: u64) -> Self {
        let default = default.max(1);
        Limits {
            default,
            max: max.max(default),
        }
    }

    /// The page size to use: the asked size, or the default, between one
    /// and the ceiling.
    pub fn per_page(&self, asked: Option<u64>) -> u64 {
        asked.unwrap_or(self.default).clamp(1, self.max.max(1))
    }

    /// The offset and limit for page `number` (one-based), with no total.
    ///
    /// A page below one is one. For a list with a total, build a [`Page`]
    /// instead: it also clamps a page past the end.
    pub fn window(&self, number: u64, asked: Option<u64>) -> (u64, u64) {
        let per_page = self.per_page(asked);
        (
            number.max(1).saturating_sub(1).saturating_mul(per_page),
            per_page,
        )
    }

    /// The page to read, with its total.
    pub fn page(&self, number: u64, asked: Option<u64>, total: u64) -> Page {
        Page::new(number, self.per_page(asked), total)
    }
}
