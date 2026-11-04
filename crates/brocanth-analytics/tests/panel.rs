//! The analytics tab, rendered by a real admin.

use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use brocanth_admin::policy::{policy_fn, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Fixture, Submission};
use brocanth_analytics::admin::AnalyticsPanel;
use brocanth_analytics::{Day, MemoryStore, Recorder, Store, View};
use brocanth_test::Html;

fn view(day: i32, visitor: i64, path: &str, referrer: &str) -> View {
    View {
        site: "site".to_string(),
        day: Day(day),
        visitor,
        path: path.to_string(),
        query: String::new(),
        referrer: referrer.to_string(),
        browser: "Firefox".to_string(),
        os: "Linux".to_string(),
        device: "desktop".to_string(),
        country: String::new(),
    }
}

/// A recorder on day 20 000 over a store with three views, one of them a
/// path that would be markup if it were not escaped.
fn recorder() -> Recorder {
    let store = Arc::new(MemoryStore::new());
    block_on(store.write(&[
        view(20_000, 1, "/", "https://news.example"),
        view(20_000, 1, "/pricing", ""),
        view(19_999, 2, "/<script>", ""),
    ]))
    .unwrap();
    let recorder = Recorder::new(
        "site",
        760,
        Arc::new(|| UNIX_EPOCH + Duration::from_secs(20_000 * 86_400 + 3_600)),
    );
    recorder.attach(store);
    recorder
}

fn admin(recorder: Recorder, allowed: bool) -> Admin {
    Admin::new(
        Arc::new(Fixture::new()),
        AdminConfig::new()
            .panel(AnalyticsPanel::new(recorder))
            .with_policy(policy_fn(move |_: &Principal<'_>, _: Request<'_>| allowed)),
    )
}

fn get(admin: &Admin, query: &str) -> (u16, String) {
    let response = block_on(admin.dispatch(
        Principal::anonymous(),
        "GET",
        "/admin/analytics",
        query,
        &Submission::new(),
    ))
    .expect("dispatch");
    (
        response.status(),
        response.html().unwrap_or_default().to_string(),
    )
}

#[test]
fn the_tab_shows_visitors_views_and_the_top_pages() {
    let (status, html) = get(&admin(recorder(), true), "");
    assert_eq!(status, 200);
    let page = Html::new(html);
    page.assert_text("2 visitors");
    page.assert_text("3 page views");
    page.assert_text("2024-10-04"); // day 20 000, the end of the range
    page.assert_text("/pricing");
    page.assert_text("https://news.example");
    page.assert_text("Firefox");
}

#[test]
fn a_label_is_escaped() {
    let (_, html) = get(&admin(recorder(), true), "");
    assert!(!html.contains("/<script>"), "the label went out as markup");
    assert!(html.contains("/&lt;script&gt;"));
}

#[test]
fn the_range_is_one_of_three_and_narrows_the_numbers() {
    let (_, html) = get(&admin(recorder(), true), "days=7");
    Html::new(html).assert_text("2024-09-28 to 2024-10-04");
    // An unknown range falls back to 30 days rather than querying a year.
    let (_, html) = get(&admin(recorder(), true), "days=365");
    Html::new(html).assert_text("2024-09-05 to 2024-10-04");
}

#[test]
fn the_tab_is_locked_like_every_other() {
    let (status, html) = get(&admin(recorder(), false), "");
    assert_eq!(status, 403);
    assert!(!html.contains("page views"));
}

#[test]
fn a_store_that_is_not_attached_says_so_instead_of_failing() {
    let recorder = Recorder::new("site", 760, Arc::new(std::time::SystemTime::now));
    let (status, html) = get(&admin(recorder, true), "");
    assert_eq!(status, 200);
    Html::new(html).assert_text("not attached");
}
