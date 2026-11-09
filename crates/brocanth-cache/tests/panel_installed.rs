//! The cache tab of a generated project: `CachePanel::installed`.
//!
//! A generated admin builds its tabs with the route table, before the cache
//! plugin opens the cache. So the tab finds the cache when it renders, in
//! the process-wide slot `CacheHandle::install` fills. The slot is global,
//! so this file is its own test binary with one test, in one order.

#![cfg(feature = "admin")]

use std::sync::Arc;

use brocanth_admin::policy::{policy_fn, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Fixture, Submission};
use brocanth_cache::admin::CachePanel;
use brocanth_cache::{CacheHandle, MemoryCache};
use brocanth_test::Html;

struct Operator;

fn dispatch(admin: &Admin, method: &str, path: &str) -> Option<brocanth_admin::Response> {
    block_on(admin.dispatch(
        Principal::new(&Operator),
        method,
        path,
        "",
        &Submission::new(),
    ))
    .ok()
}

#[test]
fn the_tab_reads_the_cache_installed_after_the_admin_was_built() {
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .panel(CachePanel::installed())
            .with_policy(policy_fn(|_: &Principal<'_>, _: Request<'_>| true)),
    );

    // Before the plugin starts, there is no cache, and the tab does not
    // pretend there is one.
    assert!(CacheHandle::installed().is_none());
    let before = dispatch(&admin, "GET", "/admin/cache");
    assert!(
        before.as_ref().is_none_or(|r| r.status() != 200),
        "the tab rendered with no cache installed"
    );

    // The plugin's `before_start`.
    let handle = CacheHandle::new(Arc::new(MemoryCache::new().with_max_value_bytes(2048)));
    assert!(handle.install(), "the first install wins");
    assert!(!CacheHandle::new(Arc::new(MemoryCache::new())).install());

    let page = dispatch(&admin, "GET", "/admin/cache").expect("dispatch");
    assert_eq!(page.status(), 200);
    let page = Html::new(page.html().expect("a page"));
    page.assert_text("2 KiB");
    page.assert_form_posts_to("/admin/cache/purge");

    let purged = dispatch(&admin, "POST", "/admin/cache/purge").expect("dispatch");
    assert!(
        (300..400).contains(&purged.status()),
        "purge redirects, got {}",
        purged.status()
    );
}
