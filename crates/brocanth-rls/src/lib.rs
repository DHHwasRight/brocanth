#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! # Map of the crate
//!
//! - [`Role`], [`Table`], [`Policy`], [`Function`], [`crud`]: the
//!   definitions.
//! - [`SqlExpr`] and [`Setting`]: `USING` and `WITH CHECK`, as sea-query
//!   expressions that know what they use.
//! - [`Schema`]: every definition, checked; [`Schema::snapshot`] is the
//!   data the diff reads.
//! - [`diff()`]: two snapshots into a [`Plan`], ordered by the dependency
//!   graph.
//! - [`migration`]: the migration file and `rls make`.
//! - [`catalog::drift`]: the live database against a snapshot.
//!
//! The design and its rejected alternatives are in `docs/rls.md`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

#[cfg(feature = "runtime")]
pub mod catalog;
#[cfg(feature = "runtime")]
mod def;
#[cfg(feature = "runtime")]
pub mod diff;
#[cfg(feature = "runtime")]
pub mod error;
#[cfg(feature = "runtime")]
mod expr;
#[cfg(feature = "runtime")]
pub mod graph;
#[cfg(feature = "runtime")]
mod ident;
#[cfg(feature = "runtime")]
pub mod migration;
#[cfg(feature = "scaffold")]
pub mod scaffold;
#[cfg(feature = "runtime")]
mod schema;
#[cfg(feature = "runtime")]
pub mod snapshot;

#[cfg(feature = "runtime")]
pub use brocanth_db::DbIdentity;
#[cfg(feature = "runtime")]
pub use def::{crud, Function, Grantee, Policy, Role, Table};
#[cfg(feature = "runtime")]
pub use diff::{diff, down, Plan};
#[cfg(feature = "runtime")]
pub use error::RlsError;
#[cfg(feature = "runtime")]
pub use expr::{Setting, SqlExpr};
#[cfg(feature = "runtime")]
pub use ident::{SqlType, MAX_IDENT};
#[cfg(feature = "runtime")]
pub use schema::Schema;
#[cfg(feature = "runtime")]
pub use snapshot::{Command, Privilege, Snapshot};
