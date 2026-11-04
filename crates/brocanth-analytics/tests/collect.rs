//! What the hook counts, through the whole pipeline, on a store in memory.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::HookFamilies;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_analytics::{Analytics, AnalyticsConfig, Day, MemoryStore, Recorder};
use brocanth_test::{TestClient, TestRequest};

const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";
const SAFARI: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
const ADDRESS: &str = "203.0.113.7";

fn answer(content_type: &'static str, body: &'static [u8]) -> Handler<Base> {
    Box::new(move |_cx: &mut RouteCx<'_, Base>| {
        Ok(http::Response::builder()
            .header("content-type", content_type)
            .body(Body::from(body.to_vec()))
            .expect("a response"))
    })
}

fn table() -> Router<Handler<Base>> {
    let html = "text/html; charset=utf-8";
    let mut table = Router::builder();
    table
        .route("/", Verb::Get, answer(html, b"<h1>Home</h1>"))
        .unwrap();
    table
        .route("/pricing", Verb::Get, answer(html, b"<h1>Pricing</h1>"))
        .unwrap();
    table
        .route("/admin/users", Verb::Get, answer(html, b"<h1>Users</h1>"))
        .unwrap();
    table
        .route("/app.css", Verb::Get, answer("text/css", b"body{}"))
        .unwrap();
    table
        .route("/api/items", Verb::Get, answer("application/json", b"[]"))
        .unwrap();
    table
        .route("/form", Verb::Post, answer(html, b"<p>ok</p>"))
        .unwrap();
    table.build().unwrap()
}

/// A clock the test moves, starting at noon on day 20 000.
fn clock() -> (
    Arc<AtomicU64>,
    impl Fn() -> SystemTime + Send + Sync + 'static,
) {
    let now = Arc::new(AtomicU64::new(20_000 * 86_400 + 12 * 3_600));
    let read = now.clone();
    (now, move || {
        UNIX_EPOCH + Duration::from_secs(read.load(Ordering::SeqCst))
    })
}

fn config() -> AnalyticsConfig {
    AnalyticsConfig {
        site: "test-site".to_string(),
        ip_header: Some("fly-client-ip".to_string()),
        country_header: Some("cf-ipcountry".to_string()),
        ..AnalyticsConfig::default()
    }
}

struct Rig<H: HookFamilies> {
    client: TestClient<Base, H, Handler<Base>>,
    recorder: Recorder,
    store: Arc<MemoryStore>,
    now: Arc<AtomicU64>,
}

fn rig(config: AnalyticsConfig) -> Rig<impl HookFamilies> {
    let store = Arc::new(MemoryStore::new());
    let (now, clock) = clock();
    let analytics = Analytics::with_clock(config, store.clone(), clock);
    let recorder = analytics.recorder();
    let app = AppBuilder::new()
        .plugin_hooking(analytics)
        .build()
        .expect("an application");
    Rig {
        client: TestClient::serving(app, table()),
        recorder,
        store,
        now,
    }
}

fn visit(path: &str, ua: &str) -> TestRequest {
    TestRequest::get(path)
        .header("user-agent", ua)
        .header("fly-client-ip", ADDRESS)
}

fn flush(recorder: &Recorder) -> Result<usize, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(recorder.flush())
        .map_err(|e| e.to_string())
}

#[test]
fn one_html_get_is_one_page_view() {
    let rig = rig(config());
    rig.client.send(visit("/pricing", FIREFOX)).assert_ok();
    assert_eq!(flush(&rig.recorder), Ok(1));
    let views = rig.store.views();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].site, "test-site");
    assert_eq!(views[0].path, "/pricing");
    assert_eq!(views[0].browser, "Firefox");
    assert_eq!(views[0].os, "Linux");
    assert_eq!(views[0].device, "desktop");
    assert_eq!(views[0].day, Day(20_000));
}

#[test]
fn assets_json_posts_admin_bots_and_prefetches_are_not_counted() {
    let rig = rig(config());
    rig.client.send(visit("/app.css", FIREFOX)).assert_ok();
    rig.client.send(visit("/api/items", FIREFOX)).assert_ok();
    rig.client.send(visit("/admin/users", FIREFOX)).assert_ok();
    rig.client.send(visit("/missing", FIREFOX));
    rig.client
        .send(
            TestRequest::post("/form")
                .header("user-agent", FIREFOX)
                .form(&[("a", "b")]),
        )
        .assert_ok();
    rig.client
        .send(visit(
            "/",
            "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)",
        ))
        .assert_ok();
    rig.client.send(visit("/", "curl/8.4.0")).assert_ok();
    rig.client
        .send(visit("/", FIREFOX).header("sec-purpose", "prefetch"))
        .assert_ok();
    assert_eq!(flush(&rig.recorder), Ok(0));
    assert!(rig.store.views().is_empty(), "{:?}", rig.store.views());
}

#[test]
fn a_dp_swap_partial_counts_once_like_the_load_it_replaces() {
    let rig = rig(config());
    rig.client.send(visit("/", FIREFOX)).assert_ok();
    rig.client
        .send(visit("/pricing", FIREFOX).header("dp-partial", "1"))
        .assert_ok();
    assert_eq!(flush(&rig.recorder), Ok(2));
}

#[test]
fn the_same_visitor_on_one_day_counts_once_and_again_the_next_day() {
    let rig = rig(config());
    rig.client.send(visit("/", FIREFOX)).assert_ok();
    rig.client.send(visit("/pricing", FIREFOX)).assert_ok();
    rig.client.send(visit("/", SAFARI)).assert_ok();
    assert_eq!(flush(&rig.recorder), Ok(3));
    let day_one = rig.store.views();
    assert_eq!(
        day_one[0].visitor, day_one[1].visitor,
        "one visitor, two pages"
    );
    assert_ne!(
        day_one[0].visitor, day_one[2].visitor,
        "another browser, another visitor"
    );

    // The next day: a new salt, so the same person is a new hash.
    rig.now.fetch_add(86_400, Ordering::SeqCst);
    rig.client.send(visit("/", FIREFOX)).assert_ok();
    assert_eq!(flush(&rig.recorder), Ok(1));
    let views = rig.store.views();
    assert_eq!(views[3].day, Day(20_001));
    assert_ne!(
        views[3].visitor, day_one[0].visitor,
        "the hash does not link days"
    );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let report = runtime
        .block_on(rig.recorder.report(Day(20_000), Day(20_001)))
        .unwrap();
    assert_eq!(report.views, 4);
    assert_eq!(report.visitors, 3, "two on day one, one on day two");
    assert_eq!(report.days, vec![(Day(20_000), 3, 2), (Day(20_001), 1, 1)]);
}

#[test]
fn an_earlier_days_salt_is_deleted_once_the_day_is_over() {
    let rig = rig(config());
    rig.client.send(visit("/", FIREFOX)).assert_ok();
    flush(&rig.recorder).unwrap();
    assert_eq!(rig.store.salt_days(), vec![Day(20_000)]);
    rig.now.fetch_add(86_400, Ordering::SeqCst);
    rig.client.send(visit("/", FIREFOX)).assert_ok();
    flush(&rig.recorder).unwrap();
    assert_eq!(rig.store.salt_days(), vec![Day(20_001)]);
}

#[test]
fn no_address_and_no_user_agent_is_ever_stored() {
    let rig = rig(config());
    rig.client
        .send(
            visit(
                "/pricing?utm_source=news&token=s3cret&email=jane%40example.com",
                SAFARI,
            )
            .header("referer", "https://news.example/item?id=42&user=jane")
            .header("cf-ipcountry", "fr"),
        )
        .assert_ok();
    flush(&rig.recorder).unwrap();
    let views = rig.store.views();
    assert_eq!(views.len(), 1);
    let view = &views[0];
    let fields = [
        &view.site,
        &view.path,
        &view.query,
        &view.referrer,
        &view.browser,
        &view.os,
        &view.device,
        &view.country,
    ];
    for field in fields {
        assert!(!field.contains(ADDRESS), "{field:?} holds the address");
        assert!(!field.contains("Mozilla"), "{field:?} holds the user agent");
        assert!(
            !field.contains("s3cret") && !field.contains("jane"),
            "{field:?}"
        );
    }
    assert_eq!(view.query, "utm_source=news");
    assert_eq!(view.referrer, "https://news.example");
    assert_eq!(view.country, "FR");
    assert_eq!(
        (view.browser.as_str(), view.device.as_str()),
        ("Safari", "mobile")
    );
}

#[test]
fn query_parameters_outside_the_allow_list_are_dropped() {
    let rig = rig(AnalyticsConfig {
        query_allow: vec!["ref".to_string()],
        ..config()
    });
    rig.client
        .send(visit("/?ref=hn&utm_source=news&session=abc", FIREFOX))
        .assert_ok();
    flush(&rig.recorder).unwrap();
    assert_eq!(rig.store.views()[0].query, "ref=hn");
    assert_eq!(rig.store.views()[0].path, "/");
}

#[test]
fn a_failed_write_never_fails_the_request_and_is_retried() {
    let rig = rig(config());
    rig.store.fail_writes(true);
    rig.client.send(visit("/", FIREFOX)).assert_ok();
    assert!(flush(&rig.recorder).is_err());
    assert_eq!(
        rig.recorder.pending(),
        1,
        "the batch went back on the buffer"
    );
    assert!(rig.recorder.last_error().is_some());
    // The store is down, and the site still answers.
    rig.client.send(visit("/pricing", FIREFOX)).assert_ok();
    rig.store.fail_writes(false);
    assert_eq!(flush(&rig.recorder), Ok(2));
    assert_eq!(rig.recorder.last_error(), None);
    let paths: Vec<String> = rig.store.views().into_iter().map(|v| v.path).collect();
    assert_eq!(paths, ["/", "/pricing"]);
}

#[test]
fn gpc_is_counted_unless_the_site_respects_it() {
    let counted = rig(config());
    counted
        .client
        .send(visit("/", FIREFOX).header("sec-gpc", "1"))
        .assert_ok();
    assert_eq!(flush(&counted.recorder), Ok(1));

    let respected = rig(AnalyticsConfig {
        respect_gpc: true,
        ..config()
    });
    respected
        .client
        .send(visit("/", FIREFOX).header("sec-gpc", "1"))
        .assert_ok();
    respected
        .client
        .send(visit("/", FIREFOX).header("dnt", "1"))
        .assert_ok();
    assert_eq!(flush(&respected.recorder), Ok(0));
}

#[test]
fn disabled_installs_no_hook() {
    let rig = rig(AnalyticsConfig {
        enabled: false,
        ..config()
    });
    rig.client.send(visit("/", FIREFOX)).assert_ok();
    assert_eq!(rig.recorder.pending(), 0);
}

#[test]
fn rows_past_the_retention_are_deleted() {
    let rig = rig(AnalyticsConfig {
        retention_days: 30,
        ..config()
    });
    rig.client.send(visit("/", FIREFOX)).assert_ok();
    flush(&rig.recorder).unwrap();
    rig.now.fetch_add(31 * 86_400, Ordering::SeqCst);
    rig.client.send(visit("/pricing", FIREFOX)).assert_ok();
    flush(&rig.recorder).unwrap();
    let paths: Vec<String> = rig.store.views().into_iter().map(|v| v.path).collect();
    assert_eq!(paths, ["/pricing"]);
}
