#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! # Map of the crate
//!
//! - [`Filters`]: the declaration. Each query-string name maps to one column
//!   of the entity's enum, with a kind and the lookups it allows.
//! - [`Applied`]: what one request asked for, as a [`Pred`] and a sort.
//! - [`FilterError`]: a value or a lookup the declaration refuses, with a
//!   `help:` line. Answer it with a 400.
//!
//! # Not a filter language
//!
//! `brocanth-policy` says there is no string-to-filter parser in the
//! workspace, and PLAN.md §9 is why: PocketBase parses a filter string, and a
//! misspelled field in it becomes `NULL`, and `NULL` becomes access. This
//! crate parses no expression. A name in the query string is looked up in a
//! list the application wrote; a name that is not there names no column. A
//! value is parsed into the column's kind before it becomes a bind. The
//! result is a [`Pred`] over the entity's own enum, so it can only be ANDed
//! with the rule, never replace it.

#![forbid(unsafe_code)]

#[cfg(feature = "runtime")]
mod filters;
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use filters::{Active, Applied, FilterError, Filters, Kind, Lookup, Sort, SORT};

#[cfg(feature = "runtime")]
pub use brocanth_pagination::Direction;
#[cfg(feature = "runtime")]
pub use brocanth_policy::Pred;
