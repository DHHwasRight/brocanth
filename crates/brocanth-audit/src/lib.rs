#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! # Map of the crate
//!
//! - [`Entry`], one change to one row, and [`Field`], one column of it.
//! - [`Trail`], the queue in front of the store, and a `ChangeSink`.
//! - [`AuditPlugin`] and [`AuditConfig`], the plugin and the `[audit]`
//!   section: the model hook, the writer and the daily purge.
//! - [`AuditStore`], [`Query`], [`MemoryStore`] and `PgStore` (feature
//!   `postgres`).
//! - `admin` (feature `admin`): the History tab, the journal that hears
//!   every admin write, and the link on every record.
//!
//! The decisions and their reasons are in `docs/modules/audit.md`.

#[cfg(feature = "runtime")]
pub mod entry;
#[cfg(feature = "runtime")]
mod plugin;
#[cfg(feature = "runtime")]
pub mod store;
#[cfg(feature = "runtime")]
mod trail;

#[cfg(feature = "admin")]
pub mod admin;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use entry::{format_time, Entry, Field, Kind};
#[cfg(feature = "runtime")]
pub use plugin::{AuditConfig, AuditPlugin};
#[cfg(feature = "postgres")]
pub use postgres::PgStore;
#[cfg(feature = "runtime")]
pub use store::{AuditStore, MemoryStore, Query, StoreError};
#[cfg(feature = "runtime")]
pub use trail::{By, Clock, Trail, MAX_PENDING};
