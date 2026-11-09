//! Pagination arithmetic, from `brocanth-pagination`.
//!
//! The admin had a paginator of its own here, and `brocanth-resource` had
//! another. Both now use the one in `brocanth-pagination`, so a fix to an
//! off-by-one reaches every list. This module stays so that
//! `brocanth_admin::paging::Page`, which an ejected `list.rsx` imports,
//! still names the type.

pub use brocanth_pagination::{Page, PageLink, Pager};
