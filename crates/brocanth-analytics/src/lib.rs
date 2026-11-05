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
//! - [`Beacon`], the snippet a static site carries, and the collector
//!   that counts what it sends (`collector_origins`).
//!
//! The design and its reasons are in `docs/analytics.md`.

#[cfg(feature = "runtime")]
pub mod beacon;
pub mod classify;
#[cfg(feature = "runtime")]
mod collector;
pub mod config;
pub mod day;
#[cfg(feature = "runtime")]
mod plugin;
#[cfg(feature = "runtime")]
pub mod recorder;
#[cfg(feature = "runtime")]
pub mod store;

#[cfg(feature = "admin")]
pub mod admin;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use beacon::{Beacon, BeaconConfig};
pub use config::AnalyticsConfig;
pub use day::Day;
#[cfg(feature = "runtime")]
pub use plugin::Analytics;
#[cfg(feature = "postgres")]
pub use postgres::PgStore;
#[cfg(feature = "runtime")]
pub use recorder::{Pending, Recorder};
#[cfg(feature = "runtime")]
pub use store::{Dimension, Line, MemoryStore, Report, Store, StoreError, View};
