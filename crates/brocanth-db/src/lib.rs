#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! # Map of the crate
//!
//! - [`DbConfig`]: the URL, the pool sizes and timeouts, the session
//!   settings, the CA bundle.
//! - [`Db`]: the pool handle. [`Db::transact`] and [`Db::begin`] open a
//!   [`Tx`].
//! - [`Exec`]: `all`, `one`, `optional`, `execute`, `json`, `raw_json`, for a
//!   `Db` and a `Tx` alike.
//! - [`DbError`]: what failed, the Postgres message, and a `help:` line.
//! - [`migrate`]: plain SQL migrations, for a project with no ORM.
//!
//! [`sea_query`] and [`sqlx`] are re-exported, so an application names one
//! dependency and gets the versions this crate was built against.

#[cfg(feature = "runtime")]
pub mod config;
#[cfg(feature = "runtime")]
mod db;
#[cfg(feature = "runtime")]
pub mod error;
#[cfg(feature = "runtime")]
pub mod exec;
#[cfg(feature = "runtime")]
pub mod migrate;
#[cfg(feature = "scaffold")]
pub mod scaffold;
#[cfg(feature = "runtime")]
pub mod shell;

#[cfg(feature = "runtime")]
pub use config::DbConfig;
#[cfg(feature = "runtime")]
pub use db::{Db, Tx};
#[cfg(feature = "runtime")]
pub use error::DbError;
#[cfg(feature = "runtime")]
pub use exec::{Exec, WriteStatement};

#[cfg(feature = "runtime")]
pub use sea_query;
#[cfg(feature = "runtime")]
pub use sqlx;

/// A handler reads the pool off the request context: `ctx.db()` in a
/// generated project. Behind the `plugin` feature, the only code here that
/// names the core.
#[cfg(feature = "plugin")]
#[cfg(feature = "runtime")]
impl brocanth::plugin_context::ContextItem for Db {}
