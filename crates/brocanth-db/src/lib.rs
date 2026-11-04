#![doc = include_str!("../README.md")]
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
//!
//! [`sea_query`] and [`sqlx`] are re-exported, so an application names one
//! dependency and gets the versions this crate was built against.

pub mod config;
mod db;
pub mod error;
pub mod exec;
#[cfg(feature = "scaffold")]
pub mod scaffold;

pub use config::DbConfig;
pub use db::{Db, Tx};
pub use error::DbError;
pub use exec::{Exec, WriteStatement};

pub use sea_query;
pub use sqlx;

/// A handler reads the pool off the request context: `ctx.db()` in a
/// generated project. Behind the `plugin` feature, the only code here that
/// names the core.
#[cfg(feature = "plugin")]
impl brocanth::plugin_context::ContextItem for Db {}
