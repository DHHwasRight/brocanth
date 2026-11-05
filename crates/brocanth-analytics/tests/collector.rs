//! The collector: page views a static site sends, through the whole
//! pipeline, on a store in memory.

use std::sync::Arc;

use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::HookFamilies;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_analytics::{Analytics, AnalyticsConfig, MemoryStore, Recorder};
use brocanth_test::{TestClient, TestRequest};

const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0";
const SAFARI: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
const ADDRESS: &str = "203.0.113.7";
const DOCS: &str = "https://docs.example";

fn table() -> Router<Handler<Base>> {
    let page: Handler<Base> = Box::new(|_cx: &mut RouteCx<'_, Base>| {
        Ok(http::Response::builder()
            .header("content-type", "text/html")
            .body(Body::from(b"<h1>Home</h1>".to_vec()))
            .expect("a response"))
    });
    let mut table = Router::builder();
    table.route("/", Verb::Get, page).unwrap();
    table.build().unwrap()
}

struct Rig<H: HookFamilies> {
    client: TestClient<Base, H, Handler<Base>>,
    recorder: Recorder,
    store: Arc<MemoryStore>,
}

fn rig(origins: &[&str]) -> Rig<impl HookFamilies> {
    let store = Arc::new(MemoryStore::new());
    let analytics = Analytics::with_store(
        AnalyticsConfig {
            site: "test-site".to_string(),
            ip_header: Some("fly-client-ip".to_string()),
            collector_origins: origins.iter().map(|o| o.to_string()).collect(),
            ..AnalyticsConfig::default()
        },
        store.clone(),
    );
    let recorder = analytics.recorder();
    let app = AppBuilder::new()
        .plugin_hooking(analytics)
        .build()
        .expect("an application");
    Rig {
        client: TestClient::serving(app, table()),
        recorder,
        store,
    }
}

fn collecting() -> Rig<impl HookFamilies> {
    // A trailing slash and upper case in the setting still match.
    rig(&["HTTPS://docs.example/"])
}

fn beacon(origin: &str, body: &str) -> TestRequest {
    TestRequest::post("/_a/v")
        .header("user-agent", SAFARI)
        .header("fly-client-ip", ADDRESS)
        .header("origin", origin)
        .body(
            "application/x-www-form-urlencoded",
            body.as_bytes().to_vec(),
        )
}

fn flush(recorder: &Recorder) -> Result<usize, String> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(recorder.flush())
        .map_err(|e| e.to_string())
}

#[test]
fn a_beacon_from_a_listed_origin_is_a_page_view_reduced_like_the_hooks() {
    let rig = collecting();
    rig.client
        .send(beacon(
            DOCS,
            "p=%2Fguide%3Futm_source%3Dnews%26token%3Ds3cret&r=https%3A%2F%2Fnews.example%2Fitem%3Fid%3D4",
        ))
        .assert_status(204)
        .assert_header("cache-control", "no-store");
    assert_eq!(flush(&rig.recorder), Ok(1));
    let view = &rig.store.views()[0];
    assert_eq!(view.path, "/guide");
    assert_eq!(view.query, "utm_source=news");
    assert_eq!(view.referrer, "https://news.example");
    assert_eq!(
        (view.browser.as_str(), view.device.as_str()),
        ("Safari", "mobile")
    );
}

#[test]
fn the_image_fallback_is_read_from_the_query_and_the_referer() {
    let rig = collecting();
    rig.client
        .send(
            TestRequest::get("/_a/v?p=%2Fguide&r=")
                .header("user-agent", FIREFOX)
                .header("fly-client-ip", ADDRESS)
                .header("referer", "https://docs.example/guide"),
        )
        .assert_status(200)
        .assert_header("content-type", "image/gif");
    assert_eq!(flush(&rig.recorder), Ok(1));
    assert_eq!(rig.store.views()[0].path, "/guide");
}

#[test]
fn a_page_view_keeps_the_servers_site_and_a_beacon_takes_its_origins_host() {
    let rig = collecting();
    rig.client
        .send(
            TestRequest::get("/")
                .header("user-agent", SAFARI)
                .header("fly-client-ip", ADDRESS),
        )
        .assert_ok();
    rig.client
        .send(beacon(DOCS, "p=%2Fguide&r="))
        .assert_status(204);
    flush(&rig.recorder).unwrap();
    let views = rig.store.views();
    assert_eq!(views.len(), 2);
    let page = views.iter().find(|v| v.path == "/").expect("the page view");
    let guide = views
        .iter()
        .find(|v| v.path == "/guide")
        .expect("the beacon");
    assert_eq!(page.site, "test-site");
    assert_eq!(guide.site, "docs.example");
    // The site is in the hash: one browser on two sites is two unrelated
    // visitors.
    assert_ne!(page.visitor, guide.visitor);
}

#[test]
fn two_collector_origins_are_two_sites() {
    let rig = rig(&[DOCS, "https://blog.example:8443"]);
    rig.client
        .send(beacon(DOCS, "p=%2Fguide"))
        .assert_status(204);
    rig.client
        .send(beacon("https://blog.example:8443", "p=%2Fpost"))
        .assert_status(204);
    rig.client
        .send(beacon("https://blog.example:8443", "p=%2Fpost"))
        .assert_status(204);
    assert_eq!(flush(&rig.recorder), Ok(3));
    let views = rig.store.views();
    let site_of = |path: &str| {
        views
            .iter()
            .filter(|v| v.path == path)
            .map(|v| v.site.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(site_of("/guide"), vec!["docs.example"]);
    assert_eq!(site_of("/post"), vec!["blog.example:8443"; 2]);
    // One browser on the same site and day is one visitor.
    let posts: Vec<_> = views.iter().filter(|v| v.path == "/post").collect();
    assert_eq!(posts[0].visitor, posts[1].visitor);
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    assert_eq!(
        rt.block_on(rig.recorder.sites()).unwrap(),
        vec!["test-site", "blog.example:8443", "docs.example"],
        "the server's site first, then every site with rows"
    );
    let today = rig.recorder.today();
    let blog = rt
        .block_on(rig.recorder.report_for("blog.example:8443", today, today))
        .unwrap();
    assert_eq!((blog.views, blog.visitors), (2, 1));
    let own = rt.block_on(rig.recorder.report(today, today)).unwrap();
    assert_eq!(own.views, 0, "beacon rows are not the server's numbers");
}

#[test]
fn a_beacon_that_fails_a_check_is_answered_the_same_and_not_counted() {
    let rig = collecting();
    // Not a listed origin.
    rig.client
        .send(beacon("https://evil.example", "p=%2F"))
        .assert_status(204);
    // No origin and no referer.
    rig.client
        .send(
            TestRequest::post("/_a/v")
                .header("user-agent", SAFARI)
                .body("application/x-www-form-urlencoded", b"p=%2F".to_vec()),
        )
        .assert_status(204);
    // Too large.
    let big = format!("p=%2F&r={}", "a".repeat(5_000));
    rig.client.send(beacon(DOCS, &big)).assert_status(204);
    // Not a path.
    rig.client
        .send(beacon(DOCS, "p=https%3A%2F%2Fevil.example%2F"))
        .assert_status(204);
    // A bot.
    rig.client
        .send(
            TestRequest::post("/_a/v")
                .header("user-agent", "curl/8.4.0")
                .header("origin", DOCS)
                .body("application/x-www-form-urlencoded", b"p=%2F".to_vec()),
        )
        .assert_status(204);
    // An excluded path.
    rig.client
        .send(beacon(DOCS, "p=%2Fadmin%2Fusers"))
        .assert_status(204);
    assert_eq!(flush(&rig.recorder), Ok(0));
}

#[test]
fn a_referrer_from_the_static_site_itself_is_not_a_referrer() {
    let rig = collecting();
    rig.client
        .send(beacon(DOCS, "p=%2Fb&r=https%3A%2F%2Fdocs.example%2Fa"))
        .assert_status(204);
    flush(&rig.recorder).unwrap();
    assert_eq!(rig.store.views()[0].referrer, "");
}

#[test]
fn the_csrf_check_does_not_refuse_a_beacon() {
    use brocanth::plugin_api::Plugin;
    use brocanth_middleware::{Csrf, CsrfConfig};

    let store = Arc::new(MemoryStore::new());
    let analytics = Analytics::with_store(
        AnalyticsConfig {
            collector_origins: vec![DOCS.to_string()],
            ..AnalyticsConfig::default()
        },
        store.clone(),
    );
    let recorder = analytics.recorder();
    // CSRF registered first, so only the declared order puts the collector
    // ahead of it.
    let app = AppBuilder::new()
        .plugin_hooking(Csrf::new(CsrfConfig::default()))
        .plugin_hooking(analytics)
        .build()
        .expect("an application");
    let client = TestClient::serving(app, table());
    client.send(beacon(DOCS, "p=%2Fguide")).assert_status(204);
    // Any other POST still meets the check.
    assert_eq!(
        client
            .send(
                TestRequest::post("/")
                    .header("user-agent", SAFARI)
                    .form(&[("a", "b")])
            )
            .status(),
        403
    );
    assert_eq!(flush(&recorder), Ok(1));
    assert_eq!(store.views()[0].path, "/guide");
}

#[test]
fn without_listed_origins_there_is_no_collector() {
    let rig = rig(&[]);
    let response = rig.client.send(beacon(DOCS, "p=%2F"));
    assert_ne!(response.status(), 204, "the path is the application's");
    assert_eq!(flush(&rig.recorder), Ok(0));
}
