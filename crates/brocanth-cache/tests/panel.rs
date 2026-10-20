//! The cache's tab, rendered through the real admin.
//!
//! # Why this one has a test double, when `tests/common/mod.rs` refuses one
//!
//! That refusal is about testing *the cache*: an in-memory backend would
//! prove nothing about the `ON CONFLICT … WHERE` that makes a lease atomic,
//! and a green suite that never touched Postgres would be believed. Every
//! word of it still stands, and none of it is about this file.
//!
//! What is under test here is the screen: that the Clear button posts to the
//! namespace it is drawn beside, that a namespace nobody declared cannot be
//! cleared however the request is spelled, and that a reader who may not act
//! is shown no buttons. The question each test asks is "which method did the
//! panel call, with what" — so the double is the *observation*, not a
//! substitute for the thing being tested. Running this against Postgres
//! would make the suite need a server to answer a question Postgres has no
//! part in.
//!
//! The admin is real, the policy is real, and the page is the page.

#![cfg(feature = "admin")]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use brocanth_admin::policy::{policy_fn, Action, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_cache::admin::CachePanel;
use brocanth_cache::{Cache, CacheError, Key, Namespace, Result};
use brocanth_test::Html;

struct Operator;

/// A cache that records what was asked of it and answers with a fixed row
/// count. Everything a cache does and this screen does not touch is
/// `unimplemented!`, so a screen that started calling `get` would fail
/// loudly rather than quietly pass.
struct Recorder {
    cleared: Mutex<Vec<String>>,
    purged: Mutex<Vec<u64>>,
    rows: AtomicU64,
}

impl Recorder {
    fn new(rows: u64) -> Arc<Recorder> {
        Arc::new(Recorder {
            cleared: Mutex::new(Vec::new()),
            purged: Mutex::new(Vec::new()),
            rows: AtomicU64::new(rows),
        })
    }

    fn cleared(&self) -> Vec<String> {
        self.cleared.lock().unwrap().clone()
    }

    fn purged(&self) -> Vec<u64> {
        self.purged.lock().unwrap().clone()
    }
}

#[async_trait]
impl Cache for Recorder {
    fn max_value_bytes(&self) -> usize {
        1024 * 1024
    }

    async fn migrate(&self) -> Result<()> {
        Ok(())
    }

    async fn get(&self, _key: &Key) -> Result<Option<Vec<u8>>> {
        unimplemented!("the panel reads no entry")
    }

    async fn set(&self, _key: &Key, _value: &[u8], _ttl: Duration) -> Result<()> {
        unimplemented!("the panel writes no entry")
    }

    async fn add(&self, _key: &Key, _value: &[u8], _ttl: Duration) -> Result<bool> {
        unimplemented!("the panel writes no entry")
    }

    async fn delete(&self, _key: &Key) -> Result<bool> {
        unimplemented!("the panel deletes no single entry")
    }

    async fn clear(&self, namespace: &Namespace) -> Result<u64> {
        self.cleared
            .lock()
            .unwrap()
            .push(namespace.as_str().to_string());
        Ok(self.rows.load(Ordering::SeqCst))
    }

    async fn lease(&self, _key: &Key, _ttl: Duration) -> Result<bool> {
        unimplemented!("the panel takes no lease")
    }

    async fn release_lease(&self, _key: &Key) -> Result<()> {
        unimplemented!("the panel takes no lease")
    }

    async fn purge_expired(&self, limit: u64) -> Result<u64> {
        self.purged.lock().unwrap().push(limit);
        Ok(self.rows.load(Ordering::SeqCst).min(limit))
    }
}

/// A cache whose every operation fails, so the screen's error path is a
/// tested path rather than a hope.
struct Broken;

#[async_trait]
impl Cache for Broken {
    fn max_value_bytes(&self) -> usize {
        1024
    }

    async fn migrate(&self) -> Result<()> {
        Ok(())
    }

    async fn get(&self, _key: &Key) -> Result<Option<Vec<u8>>> {
        unimplemented!()
    }

    async fn set(&self, _key: &Key, _value: &[u8], _ttl: Duration) -> Result<()> {
        unimplemented!()
    }

    async fn add(&self, _key: &Key, _value: &[u8], _ttl: Duration) -> Result<bool> {
        unimplemented!()
    }

    async fn delete(&self, _key: &Key) -> Result<bool> {
        unimplemented!()
    }

    async fn clear(&self, _namespace: &Namespace) -> Result<u64> {
        Err(CacheError::Backend(
            "the connection went away".to_string().into(),
        ))
    }

    async fn lease(&self, _key: &Key, _ttl: Duration) -> Result<bool> {
        unimplemented!()
    }

    async fn release_lease(&self, _key: &Key) -> Result<()> {
        unimplemented!()
    }

    async fn purge_expired(&self, _limit: u64) -> Result<u64> {
        unimplemented!()
    }
}

fn ns(name: &str) -> Namespace {
    Namespace::new(name).expect("a valid namespace")
}

/// An admin with the cache's tab on it. `act` decides whether this person
/// may press the buttons.
fn admin(cache: Arc<dyn Cache>, act: bool) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(
                CachePanel::new(cache)
                    .namespace(ns("pages"))
                    .namespace(ns("sessions")),
            )
            .with_policy(policy_fn(move |_: &Principal<'_>, r: Request<'_>| {
                act || r.action != Action::ActOnPanel
            })),
    )
}

fn get(admin: &Admin, path: &str, query: &str) -> Html {
    let who = Operator;
    let response =
        block_on(admin.dispatch(Principal::new(&who), "GET", path, query, &Submission::new()))
            .expect("dispatch");
    assert_eq!(response.status(), 200, "{path}?{query}");
    Html::new(response.html().expect("a page"))
}

fn post(admin: &Admin, path: &str) -> brocanth_admin::Response {
    let who = Operator;
    block_on(admin.dispatch(Principal::new(&who), "POST", path, "", &Submission::new()))
        .expect("dispatch")
}

// ---------------------------------------------------------------------------

#[test]
fn the_screen_says_it_has_no_numbers_and_why() {
    let page = get(&admin(Recorder::new(0), true), "/admin/cache", "");

    // The claim this tab is built on. It is on the screen, in words, so
    // nobody has to read `docs/backoffice.md` to find out why the dashboard
    // is empty.
    page.assert_text("Two buttons and no numbers");
    page.assert_text("purge_expired");
    page.assert_text("stats");
    // And the one figure the trait does hand over.
    page.assert_text("1 MiB");
}

#[test]
fn every_declared_namespace_gets_a_button_that_posts_to_its_own_path() {
    let page = get(&admin(Recorder::new(0), true), "/admin/cache", "");

    page.assert_text("pages");
    page.assert_text("sessions");
    page.assert_form_posts_to("/admin/cache/clear/pages");
    page.assert_form_posts_to("/admin/cache/clear/sessions");
    page.assert_form_posts_to("/admin/cache/purge");
    page.assert_button("Clear");
    page.assert_button("Purge expired");
}

#[test]
fn clearing_a_namespace_calls_the_cache_and_reports_what_it_removed() {
    let cache = Recorder::new(12);
    let a = admin(cache.clone(), true);

    let response = post(&a, "/admin/cache/clear/pages");
    assert_eq!(
        response.status(),
        303,
        "a redirect, so a reload does not press it twice"
    );
    assert_eq!(
        response.location(),
        Some("/admin/cache?cleared=pages&rows=12")
    );
    assert_eq!(cache.cleared(), ["pages"]);

    // And the screen the redirect lands on says what happened.
    let page = get(&a, "/admin/cache", "cleared=pages&rows=12");
    page.assert_text("Cleared `pages`: 12 entries removed.");
}

#[test]
fn purging_passes_the_limit_and_says_when_the_batch_was_full() {
    let cache = Recorder::new(5_000);
    let a = admin(cache.clone(), true);

    let response = post(&a, "/admin/cache/purge");
    assert_eq!(response.status(), 303);
    assert_eq!(response.location(), Some("/admin/cache?purged=1000"));
    assert_eq!(cache.purged(), [1_000], "the panel's own batch limit");

    // A full batch means there is probably more, and the screen says so
    // rather than letting somebody believe the table is clean.
    get(&a, "/admin/cache", "purged=1000").assert_text("There may be more");
    get(&a, "/admin/cache", "purged=12").assert_text("Nothing expired is left.");
}

#[test]
fn a_namespace_nobody_declared_cannot_be_cleared_even_by_hand() {
    // The trait cannot enumerate namespaces, so the declared list is the
    // only statement anywhere about which ones exist. A POST naming one
    // that is not on it is this panel's own 404, and the cache is never
    // touched.
    let cache = Recorder::new(9);
    let a = admin(cache.clone(), true);

    assert_eq!(post(&a, "/admin/cache/clear/secrets").status(), 404);
    assert!(cache.cleared().is_empty(), "and nothing was cleared");
}

#[test]
fn a_reader_who_may_not_act_is_shown_no_buttons_and_is_refused_anyway() {
    let cache = Recorder::new(3);
    let readonly = admin(cache.clone(), false);

    let page = get(&readonly, "/admin/cache", "");
    page.assert_text("pages");
    page.assert_no_button("Clear");
    page.assert_no_button("Purge expired");
    assert!(page.forms().is_empty(), "and there is nothing to post");

    // Hidden is not refused. The refusal is the admin's, before the panel
    // runs, which is why a panel that drew the buttons anyway would still
    // be safe.
    assert_eq!(post(&readonly, "/admin/cache/clear/pages").status(), 403);
    assert_eq!(post(&readonly, "/admin/cache/purge").status(), 403);
    assert!(cache.cleared().is_empty());
    assert!(cache.purged().is_empty());
}

#[test]
fn a_cache_that_fails_says_so_rather_than_redirecting_as_if_it_worked() {
    let a = admin(Arc::new(Broken), true);
    let error = block_on(a.dispatch(
        Principal::new(&Operator),
        "POST",
        "/admin/cache/clear/pages",
        "",
        &Submission::new(),
    ))
    .expect_err("the cache failed");

    assert!(
        error.to_string().contains("the connection went away"),
        "the backend's own words reach the operator: {error}"
    );
}

#[test]
fn a_forged_notice_in_the_query_string_is_not_echoed() {
    // The outcome travels in the query string because the admin has no
    // flash, which makes the query string an input. A namespace that was
    // never declared is not printed back, whatever a pasted link says.
    let a = admin(Recorder::new(0), true);

    let page = get(&a, "/admin/cache", "cleared=%3Cscript%3E&rows=1");
    page.assert_no_text("Cleared");
    page.assert_no_text("<script>");
}

#[test]
fn the_tab_is_in_the_rail_and_a_sub_path_is_the_admins_404() {
    let a = admin(Recorder::new(0), true);

    get(&a, "/admin/cache", "")
        .within("nav", "aria-label", "Sections")
        .assert_link_to("/admin/cache");

    let who = Operator;
    let response = block_on(a.dispatch(
        Principal::new(&who),
        "GET",
        "/admin/cache/nope",
        "",
        &Submission::new(),
    ))
    .expect("dispatch");
    assert_eq!(response.status(), 404);
}

/// Prints the screen, for a human who wants to see it.
#[test]
#[ignore = "prints a page, asserts nothing"]
fn show() {
    let a = admin(Recorder::new(0), true);
    println!("{}", get(&a, "/admin/cache", "purged=1000").source());
}
