#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! # Map of the crate
//!
//! - [`Page`] and [`Limits`]: offset pages, clamped on construction.
//! - [`Cursor`], [`KeysetPage`] and, with `policy`, [`Keyset`]: keyset pages.
//! - [`Params`], [`Links`], [`Pager`], [`KeysetPager`]: what a request asked
//!   for, and the links a page answers with.
//! - [`query`]: the query-string reader every list here shares.
//!
//! # Why one crate
//!
//! `brocanth-admin` and `brocanth-resource` each had a paginator of their
//! own, with the same clamps written twice. Neither may depend on the other,
//! and a paginator is not authorisation, so it is not `brocanth-policy`'s.
//! It is here, with no dependency, and both take it.

#![forbid(unsafe_code)]

#[cfg(feature = "runtime")]
mod cursor;
#[cfg(feature = "policy")]
mod keyset;
#[cfg(feature = "runtime")]
mod links;
#[cfg(feature = "runtime")]
mod page;
#[cfg(feature = "runtime")]
pub mod query;
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use cursor::{Cursor, CursorError, Key, KeysetPage};
#[cfg(feature = "policy")]
pub use keyset::Keyset;
#[cfg(feature = "runtime")]
pub use links::{KeysetPager, Links, PageLink, Pager, ParamError, Params, AFTER, PAGE, PER_PAGE};
#[cfg(feature = "runtime")]
pub use page::{Limits, Page};

/// Which way a list is sorted.
#[cfg(feature = "runtime")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Smallest first.
    #[default]
    Ascending,
    /// Largest first.
    Descending,
}
