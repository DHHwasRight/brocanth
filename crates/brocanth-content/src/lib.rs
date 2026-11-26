#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! # Two compilations, one set of types
//!
//! A collection is read twice. `build.rs` reads each file with the `build`
//! feature: it parses the front matter into the project's own type, stops
//! the build on the first bad file it finds (all of them, in fact, one
//! line each), and writes the list of files the binary embeds. The binary
//! reads the same files with the `runtime` feature, once at start: front
//! matter again, and the body through `brocanth-markdown`.
//!
//! The type is shared through a `#[path]` module in `build.rs`, so the
//! check and the binary cannot disagree about a field. It may use `serde`,
//! `std` and this crate's [`Date`] and [`FrontMatter`], nothing else of the
//! project: `build.rs` compiles it alone.

#[cfg(any(feature = "build", feature = "runtime"))]
mod date;
#[cfg(any(feature = "build", feature = "runtime"))]
mod front;

#[cfg(feature = "build")]
pub mod build;
#[cfg(feature = "runtime")]
mod collection;
#[cfg(feature = "runtime")]
pub mod feed;

#[cfg(feature = "scaffold")]
pub mod scaffold;
#[cfg(feature = "scaffold")]
mod scaffold_feed;

#[cfg(feature = "runtime")]
pub use collection::{Collection, Entry, View};
#[cfg(any(feature = "build", feature = "runtime"))]
pub use date::Date;
#[cfg(any(feature = "build", feature = "runtime"))]
pub use front::{is_slug, read_name, FrontMatter, Source};
