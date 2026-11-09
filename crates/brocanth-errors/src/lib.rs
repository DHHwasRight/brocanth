#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! ---
//!
//! What a failure becomes, and who hears about it.
//!
//! Three parts, each small:
//!
//! - [`Caught`]: what an error page is given. The message of a 4xx, which
//!   the client caused and may read; never the message of a 5xx, which is
//!   for the log.
//! - [`plain_page`] and [`development_page`]: the HTML for a project with no
//!   error page of its own, and the page with the cause that a debug build
//!   shows for a 5xx.
//! - [`Reporter`]: the hook another module implements to hear about every
//!   5xx, from a page or from a hook. The [`Errors`] plugin publishes
//!   [`Reporters`] in the services; a plugin adds its reporter in
//!   `after_config`.
//!
//! The error pages themselves, `not-found.rsx` and `error.rsx`, are the
//! project's templates. `brocanth_pages::pages!("src/pages", errors)` finds
//! them and wraps them in the layouts above them.

#[cfg(feature = "runtime")]
mod page;
#[cfg(feature = "runtime")]
mod plugin;

#[cfg(feature = "runtime")]
pub use page::{accepts_html, development_page, plain_page, reason, Caught};
#[cfg(feature = "runtime")]
pub use plugin::{Errors, ErrorsConfig, Origin, Report, Reporter, Reporters};

#[cfg(feature = "scaffold")]
pub mod scaffold;
