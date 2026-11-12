#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! # Map of the crate
//!
//! - [`Index`]: one table's search. [`Index::migration`] writes the
//!   generated `tsvector` column and its indexes; [`Index::query`] writes the
//!   ranked page and its count around a policy [`Condition`].
//! - With `postgres`: [`Index::search`] runs both over a `brocanth_db::Db`.
//! - [`documents`]: the site index, one table of everything a visitor can
//!   find, which the `search` module's page reads.
//!
//! [`Condition`]: brocanth_policy::Condition

#![forbid(unsafe_code)]

#[cfg(feature = "runtime")]
pub mod documents;
#[cfg(feature = "runtime")]
mod index;
#[cfg(feature = "postgres")]
mod run;
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use index::{Index, Migration, SearchQuery, Statement, Weight, MAX_TERM};
#[cfg(feature = "postgres")]
pub use run::Hits;

/// The paginator a search page reads its page with, re-exported so a page
/// names one crate.
#[cfg(feature = "runtime")]
pub use brocanth_pagination as pagination;
/// The rule algebra a search is ANDed with, re-exported for the same reason.
#[cfg(feature = "runtime")]
pub use brocanth_policy as policy;

/// A search that cannot be written or run.
#[cfg(feature = "runtime")]
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SearchError {
    /// The index names no column to search.
    #[error(
        "the search of `{table}` names no column\n\
         help: add one with `.source(Column::Title, Weight::A)`"
    )]
    NoSource {
        /// The table.
        table: &'static str,
    },
    /// A column name is not a bare SQL identifier.
    #[error("{0}\nhelp: name the column with letters, digits and `_`")]
    Compile(#[from] brocanth_policy::CompileError),
    /// The database refused the statement.
    #[cfg(feature = "postgres")]
    #[error("{0}")]
    Db(#[from] brocanth_db::DbError),
}
