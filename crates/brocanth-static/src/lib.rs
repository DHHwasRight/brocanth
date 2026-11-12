#![doc = include_str!("../README.md")]
//!
//! ---
//!
//! A brocanth application, written to a directory any static host serves.
//!
//! The pages are the same pages. [`Export`] sends each path through the
//! application's own handler, as a GET with no cookie and no query, and
//! writes the answer:
//!
//! - an HTML page at `/a/b` to `a/b/index.html`, so `/a/b` and `/a/b/`
//!   both work on GitHub Pages, S3 or nginx;
//! - a path whose last segment has a dot (`/data/sales.json`) to that name;
//! - for each HTML page, one partial per layout depth `1..=depth` to
//!   `partials/<depth>/a/b/index.html`, requested with `DP-Partial: <depth>`.
//!   dp-swap's `partialDir` reads them (github.com/brocanth/htmx);
//! - whatever a missing path answers to `404.html`;
//! - every asset at the path it is published at.
//!
//! With a base path (`/d3set` for `https://<org>.github.io/d3set/`), every
//! root-relative URL in the HTML and CSS it writes is prefixed. See
//! [`rebase`] for what counts as a URL.
//!
//! # Every depth, always
//!
//! A page with one layout answers depth 2 with the same bytes as depth 1.
//! The export writes both. dp-swap asks for the number of layers the
//! client already holds, which the page it is leaving decides, not the
//! page it is going to: a missing file would be a failed navigation, and
//! a duplicate costs only disk.
//!
//! # Who depends on this
//!
//! The generated `export` command (`brocanth create --with static`) is a
//! thin caller of this crate. The d3set chart gallery
//! (github.com/DHHwasRight/d3set, `rust/gallery/src/export.rs`) calls it
//! directly, with a base path and two layout layers, and publishes the
//! result to GitHub Pages. `tests/d3set.rs` mirrors what that gallery
//! needs, so a change that breaks it fails here first.

pub mod rebase;

#[cfg(feature = "runtime")]
mod export;
#[cfg(feature = "runtime")]
pub use export::*;

/// Behind the `scaffold` feature, which is off: the generator is the CLI's
/// business and not an application's.
#[cfg(feature = "scaffold")]
pub mod scaffold;
