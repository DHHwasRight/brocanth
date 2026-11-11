#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! What a page says about itself, and the tags a layout writes for it.
//!
//! Three types, one per owner:
//!
//! - [`Site`]: what every page shares, from the `[meta]` settings.
//! - [`Meta`]: what one page declares. Every field is optional.
//! - [`Head`]: the two resolved, which the root layout writes in `<head>`.
//!   Its `Display` is the tags, escaped.

#[cfg(feature = "runtime")]
mod head;

#[cfg(feature = "runtime")]
pub use head::{Alternate, Head, Meta, Site};

#[cfg(feature = "scaffold")]
pub mod scaffold;
