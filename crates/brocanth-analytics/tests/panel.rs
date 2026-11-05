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
    on("site", day, visitor, path, referrer)
}

fn on(site: &str, day: i32, visitor: i64, path: &str, referrer: &str) -> View {
    View {
        site: site.to_string(),
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

/// A recorder on day 20 000 over a store with three views of its own site,
/// one of them a path that would be markup if it were not escaped, and one
/// view of each of two static sites, one named like markup.
fn recorder() -> Recorder {
    let store = Arc::new(MemoryStore::new());
    block_on(store.write(&[
        view(20_000, 1, "/", "https://news.example"),
        view(20_000, 1, "/pricing", ""),
        view(19_999, 2, "/<script>", ""),
        on("docs.example", 20_000, 7, "/guide", ""),
        on("<b>bold", 20_000, 8, "/x", ""),
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
fn any_two_dates_make_a_range() {
    let (_, html) = get(&admin(recorder(), true), "from=2024-10-03&to=2024-10-03");
    let page = Html::new(html);
    page.assert_text("2024-10-03 to 2024-10-03");
    page.assert_text("1 visitors");
    page.assert_text("1 page views");
    // The form shows the range it drew.
    let from = page.field("from").expect("a from field");
    assert_eq!(from.attr("value"), Some("2024-10-03"));
}

#[test]
fn a_range_backwards_too_long_or_unreadable_falls_back_to_30_days() {
    for query in [
        "from=2024-10-04&to=2024-10-01",
        "from=2020-01-01&to=2024-10-04",
        "from=yesterday&to=2024-10-04",
        "from=2024-02-30&to=2024-10-04",
    ] {
        let (_, html) = get(&admin(recorder(), true), query);
        Html::new(html).assert_text("2024-09-05 to 2024-10-04");
    }
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

#[test]
fn the_selector_lists_the_servers_site_first_then_every_site_with_rows() {
    let (_, html) = get(&admin(recorder(), true), "");
    let page = Html::new(html.clone());
    assert_eq!(
        page.field("site").expect("a site selector").name(),
        "select"
    );
    let options: Vec<_> = page
        .tags("option")
        .iter()
        .map(|o| {
            (
                o.attr("value").unwrap_or("").to_string(),
                o.has_attr("selected"),
            )
        })
        .collect();
    assert_eq!(
        options,
        vec![
            ("site".to_string(), true),
            ("<b>bold".to_string(), false),
            ("docs.example".to_string(), false),
        ],
        "the server's site first and chosen, then the others sorted"
    );
    // The default is the server's own numbers.
    page.assert_text("2 visitors");
    page.assert_text("/pricing");
    assert!(!html.contains("/guide"));
}

#[test]
fn a_site_shows_only_its_own_views() {
    let (_, html) = get(&admin(recorder(), true), "site=docs.example");
    let page = Html::new(html.clone());
    page.assert_text("1 visitors");
    page.assert_text("/guide");
    assert!(!html.contains("/pricing"));
    assert!(html.contains("value=\"docs.example\" selected"));
    // The range links and the date form carry the site.
    assert!(html.contains("days=7&amp;site=docs.example"));
    let hidden = html.matches("name=\"site\" value=\"docs.example\"").count();
    assert_eq!(hidden, 1, "the date form keeps the site");
}

#[test]
fn the_site_keeps_the_range_and_the_range_keeps_the_site() {
    let (_, html) = get(
        &admin(recorder(), true),
        "site=docs.example&from=2024-10-03&to=2024-10-04",
    );
    let page = Html::new(html.clone());
    page.assert_text("2024-10-03 to 2024-10-04");
    page.assert_text("/guide");
    assert!(html.contains("name=\"from\" value=\"2024-10-03\""));
    let (_, html) = get(&admin(recorder(), true), "site=docs.example&days=7");
    assert!(html.contains("name=\"days\" value=\"7\""));
}

#[test]
fn an_unknown_site_is_the_servers_own() {
    let (_, html) = get(&admin(recorder(), true), "site=nowhere.example");
    let page = Html::new(html.clone());
    page.assert_text("2 visitors");
    assert!(!html.contains("nowhere.example"));
}

#[test]
fn a_site_name_is_escaped() {
    let (_, html) = get(&admin(recorder(), true), "site=%3Cb%3Ebold");
    assert!(!html.contains("<b>bold"), "the site went out as markup");
    assert!(html.contains("&lt;b&gt;bold"));
    assert!(html.contains("site=%3Cb%3Ebold"), "the link encodes it");
    Html::new(html).assert_text("/x");
}
