//! The cache tab of a generated project: `CachePanel::late`.
//!
//! A generated admin builds its tabs with the route table, before the cache
//! plugin opens the cache. So the tab holds a cell, and the admin fills it
//! from the first request that carries the cache on its context. The cell
//! belongs to one admin: two admins in one process see two caches.

#![cfg(feature = "admin")]

use std::sync::Arc;

use brocanth_admin::late::{Late, LateCells};
use brocanth_admin::policy::{policy_fn, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Fixture, Submission};
use brocanth_cache::admin::CachePanel;
use brocanth_cache::{CacheHandle, MemoryCache};
use brocanth_test::Html;

struct Operator;

/// What a request context carries, as far as the tab is concerned.
struct Context {
    cache: Option<CacheHandle>,
}

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

fn admin(cell: Late<CacheHandle>) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .panel(CachePanel::late(cell))
            .with_policy(policy_fn(|_: &Principal<'_>, _: Request<'_>| true)),
    )
}

#[test]
fn the_tab_reads_the_cache_that_arrives_after_the_admin_was_built() {
    let mut late = LateCells::<Context>::new();
    let cell = late.cell(|cx: &Context| cx.cache.clone());
    let admin = admin(cell);

    // Before the plugin starts, there is no cache, and the tab does not
    // pretend there is one.
    late.fill(&Context { cache: None });
    let before = dispatch(&admin, "GET", "/admin/cache");
    assert!(
        before.as_ref().is_none_or(|r| r.status() != 200),
        "the tab rendered with no cache"
    );

    // The first request after the plugin's `before_start`.
    let handle = CacheHandle::new(Arc::new(MemoryCache::new().with_max_value_bytes(2048)));
    late.fill(&Context {
        cache: Some(handle),
    });

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

#[test]
fn two_admins_in_one_process_read_two_caches() {
    let small = Late::ready(CacheHandle::new(Arc::new(
        MemoryCache::new().with_max_value_bytes(1024),
    )));
    let large = Late::ready(CacheHandle::new(Arc::new(
        MemoryCache::new().with_max_value_bytes(4096),
    )));
    let (one, two) = (admin(small), admin(large));
    let page = |admin: &Admin| {
        let page = dispatch(admin, "GET", "/admin/cache").expect("dispatch");
        Html::new(page.html().expect("a page"))
    };
    page(&one).assert_text("1 KiB");
    page(&two).assert_text("4 KiB");
}
