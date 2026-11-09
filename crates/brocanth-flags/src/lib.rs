#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! # Map of the crate
//!
//! - [`Flag`], one flag the code reads, and [`Actor`], who asks.
//! - [`Rule`], the stored answer: on for everyone, a percentage, a list of
//!   users, a list of groups.
//! - [`Flags`], the in-process copy a page reads without waiting.
//! - [`FlagsPlugin`] and [`FlagsConfig`], the plugin and the `[flags]`
//!   section.
//! - [`FlagStore`], [`MemoryStore`] and `PgStore` (feature `postgres`).
//! - `admin::FlagsPanel` (feature `admin`), the tab that turns them.
//!
//! The decisions and their reasons are in `docs/modules/flags.md`.

#[cfg(feature = "runtime")]
mod flags;
#[cfg(feature = "runtime")]
mod plugin;
#[cfg(feature = "runtime")]
pub mod rule;
#[cfg(feature = "runtime")]
pub mod store;

#[cfg(feature = "admin")]
pub mod admin;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
pub use flags::Flags;
#[cfg(feature = "runtime")]
pub use plugin::{FlagsConfig, FlagsPlugin};
#[cfg(feature = "postgres")]
pub use postgres::PgStore;
#[cfg(feature = "runtime")]
pub use rule::{bucket, valid_key, Actor, Flag, Rule};
#[cfg(feature = "runtime")]
pub use store::{FlagStore, MemoryStore, StoreError};
