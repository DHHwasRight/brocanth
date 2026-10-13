//! Pagination that does not lie, and the one escape hatch from it.
//!
//! # The claim
//!
//! A page is honest when three things hold:
//!
//! 1. **The filter is in the query.** The policy's condition goes into the
//!    `WHERE` clause, so the page is a window over the admitted set rather
//!    than over the whole table. PLAN.md §9 is the argument; [`ListQuery`]
//!    is the mechanism.
//! 2. **The total counts the same set.** [`ResourceData::count`] gets the same
//!    condition as [`ResourceData::list`], from the same compiled predicate,
//!    in the same call.
//! 3. **The order is total and declared.** `LIMIT`/`OFFSET` over an unordered
//!    query is a different lie: two pages of an unordered result may overlap
//!    for no reason to do with authorisation. [`Sort`] is never absent, and it
//!    defaults to the primary key ascending, which is unique by definition.
//!
//! [`ListQuery`]: crate::ListQuery
//! [`ResourceData::count`]: crate::ResourceData::count
//! [`ResourceData::list`]: crate::ResourceData::list
//! [`Sort`]: crate::Sort
//!
//! # `skipTotal`
//!
//! PocketBase has it because `count(*)` over a large filtered set is the
//! expensive half of a cheap request. That reason is real and it is kept.
//!
//! What is not kept is reporting a number anyway. A skipped total is
//! [`Totals::Skipped`], and the envelope then has **no** `totalItems` and no
//! `totalPages` field, rather than a sentinel. A client that forgets to check
//! for a sentinel renders `-1 results`; a client that forgets to check for a
//! missing field cannot miss it. The type says the same thing to the Rust
//! caller: there is no `u64` to read out of `Totals::Skipped`.

use crate::resource::Resource;

/// What the client asked for, before it is clamped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRequest {
    /// One-based, as PocketBase's `page` is.
    pub page: u64,
    /// `None` means the resource's default.
    pub per_page: Option<u64>,
    /// Skip the `count(*)`.
    pub skip_total: bool,
}

impl Default for PageRequest {
    fn default() -> Self {
        PageRequest {
            page: 1,
            per_page: None,
            skip_total: false,
        }
    }
}

impl PageRequest {
    /// The first page, with totals.
    pub fn new() -> Self {
        PageRequest::default()
    }

    /// The offset and limit for a resource, clamped.
    ///
    /// `page` below one is one, and `per_page` is clamped to
    /// [`Resource::MAX_PER_PAGE`] rather than refused: a client asking for
    /// a million rows gets the maximum, which is what every paginated API
    /// does, and refusing would make a harmless mistake fatal.
    pub fn window<R: Resource + ?Sized>(&self) -> (u64, u64) {
        let per_page = self
            .per_page
            .unwrap_or(R::DEFAULT_PER_PAGE)
            .clamp(1, R::MAX_PER_PAGE);
        let page = self.page.max(1);
        (page.saturating_sub(1).saturating_mul(per_page), per_page)
    }
}

/// How many rows match, or the fact that nobody counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Totals {
    /// Counted over the same condition the page was read with.
    Counted(u64),
    /// The client passed `skipTotal`. There is no number, and the envelope
    /// says so by leaving the fields out.
    Skipped,
}

impl Totals {
    /// How many pages, when there is a total to divide.
    pub fn pages(self, per_page: u64) -> Option<u64> {
        match self {
            Totals::Counted(total) if per_page > 0 => Some(total.div_ceil(per_page)),
            _ => None,
        }
    }

    /// The count, when there is one.
    pub fn count(self) -> Option<u64> {
        match self {
            Totals::Counted(total) => Some(total),
            Totals::Skipped => None,
        }
    }
}

/// One page of rows, and what the envelope needs to describe it.
#[derive(Clone, Debug)]
pub struct Listing<Row> {
    /// The rows, in the order the data source returned them.
    ///
    /// Never filtered after reading. If this is shorter than `per_page` it is
    /// because the database ran out of matching rows, which is the only honest
    /// reason for a short page.
    pub items: Vec<Row>,
    /// The page that was read, one-based and clamped.
    pub page: u64,
    /// The window size that was used, clamped.
    pub per_page: u64,
    /// The total, or the fact that it was skipped.
    pub total: Totals,
}
