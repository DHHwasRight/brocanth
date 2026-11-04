#![doc = include_str!("../README.md")]
//!
//! ---
//!
//! # Map of the crate
//!
//! - [`Analytics`], the plugin and its request hook.
//! - [`AnalyticsConfig`], the `[analytics]` section.
//! - [`Recorder`], the buffer between the hook and the store, and the
//!   flush.
//! - [`Store`], [`MemoryStore`] and `PgStore` (feature `postgres`), where the
//!   views go, and [`Report`], what comes back.
//! - [`classify`], how a request is reduced before anything is kept.
//! - `admin::AnalyticsPanel` (feature `admin`), the dashboard tab.
//!
//! The design and its reasons are in `docs/analytics.md`.

pub mod classify;
pub mod config;
pub mod day;
mod plugin;
pub mod recorder;
pub mod store;

#[cfg(feature = "admin")]
pub mod admin;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "scaffold")]
pub mod scaffold;

pub use config::AnalyticsConfig;
pub use day::Day;
pub use plugin::Analytics;
#[cfg(feature = "postgres")]
pub use postgres::PgStore;
pub use recorder::{Pending, Recorder};
pub use store::{Dimension, Line, MemoryStore, Report, Store, StoreError, View};
