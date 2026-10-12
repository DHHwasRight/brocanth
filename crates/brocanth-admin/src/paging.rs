//! Pagination arithmetic.
//!
//! Small, and worth its own file because every off-by-one an admin can have
//! lives here: an empty table that reports "page 1 of 0", a last page that
//! offers a next link, and an exact multiple that renders one empty page past
//! the end. [`Page`] clamps on construction, so a handler cannot carry an
//! out-of-range page number any further than this type.

/// One page of a listing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Page {
    number: usize,
    per_page: usize,
    total: usize,
}

impl Page {
    /// Builds a page, clamping the request into range.
    ///
    /// `per_page` of zero becomes one, a number below one becomes one, and a
    /// number past the end becomes the last page. An empty table has one page,
    /// which is the page that says it is empty.
    pub fn new(number: usize, per_page: usize, total: usize) -> Self {
        let per_page = per_page.max(1);
        let total_pages = total.div_ceil(per_page).max(1);
        Page {
            number: number.clamp(1, total_pages),
            per_page,
            total,
        }
    }

    /// The current page, counting from one.
    pub fn number(&self) -> usize {
        self.number
    }

    /// Rows per page.
    pub fn per_page(&self) -> usize {
        self.per_page
    }

    /// Rows in total, across every page.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Pages in total. Never zero.
    pub fn total_pages(&self) -> usize {
        self.total.div_ceil(self.per_page).max(1)
    }

    /// Rows to skip to reach this page.
    pub fn offset(&self) -> usize {
        (self.number - 1) * self.per_page
    }

    /// Rows to read for this page.
    pub fn limit(&self) -> usize {
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
    pub fn previous(&self) -> Option<usize> {
        (!self.is_first()).then(|| self.number - 1)
    }

    /// The next page number, when there is one.
    pub fn next(&self) -> Option<usize> {
        (!self.is_last()).then(|| self.number + 1)
    }

    /// The one-based inclusive row range this page shows, or `None` when the
    /// table is empty.
    pub fn range(&self) -> Option<(usize, usize)> {
        if self.total == 0 {
            return None;
        }
        let first = self.offset() + 1;
        let last = (self.offset() + self.per_page).min(self.total);
        Some((first, last))
    }

    /// Up to `span` page numbers centred on the current one, clamped to the
    /// range that exists. `span` of zero yields the current page alone.
    pub fn window(&self, span: usize) -> Vec<usize> {
        let span = span.max(1);
        let total_pages = self.total_pages();
        if total_pages <= span {
            return (1..=total_pages).collect();
        }
        let half = span / 2;
        let mut first = self.number.saturating_sub(half).max(1);
        if first + span - 1 > total_pages {
            first = total_pages + 1 - span;
        }
        (first..first + span).collect()
    }
}
