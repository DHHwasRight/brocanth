#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! # Two halves
//!
//! - [`render`] and [`Markdown`] (feature `render`): Markdown to HTML, and
//!   the HTML through an allow-list. What comes out is a
//!   [`SafeHtml`](brocanth_sanitize::SafeHtml).
//! - [`front_matter`] (feature `front-matter`): the TOML between the two
//!   `+++` lines at the top of a file, read into a serde type, and the
//!   body after it.
//!
//! A build script that checks content files takes `front-matter` alone,
//! so it compiles no HTML parser.

#[cfg(feature = "front-matter")]
mod front;
#[cfg(feature = "render")]
mod render;

#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "front-matter")]
pub use front::{front_matter, split, Document, FrontMatterError};
#[cfg(feature = "render")]
pub use render::{render, render_user, Markdown};

/// Re-exported, so a page that renders Markdown needs no second
/// dependency line to name what it gets back.
#[cfg(feature = "render")]
pub use brocanth_sanitize::SafeHtml;
