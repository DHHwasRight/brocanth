//! The two files the layout inlines.
//!
//! Here and not in the template, because `include_str!` inside a `.rsx`
//! file resolves against the module that expands the macro -- `lib.rs` --
//! rather than against the template. A relative path that means something
//! other than where it is written is a trap; one indirection removes it.
//!
//! Both are inlined rather than served. The admin answers with a page and
//! mounts no asset route, and widening that seam for two files would be the
//! tail wagging the dog. Together they are about 22 kB on a backoffice page
//! that is already behind a login.

/// The stylesheet, built from `style/admin.css`. See `style/README.md`.
pub(crate) const STYLE: &str = include_str!("admin.css");

/// dp-swap, vendored in `vendor/`. It enhances links and deliberately omits
/// mutating verbs, so every form here is a real POST that reloads the page.
/// PLAN.md §5 is the argument and the list of what is left for the server.
pub(crate) const SCRIPT: &str = include_str!("../vendor/dpswap.min.js");
