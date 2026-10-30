//! The rate limiter, through the whole pipeline, on both plugins.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::HookFamilies;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_middleware::rate_limit::{
    Key, MemoryStore, RateLimitConfig, Rule, SharedConfig, Subject,
};
use brocanth_middleware::{RateLimit, SharedRateLimit};
use brocanth_test::{TestClient, TestRequest, TestResponse};

fn ok() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| Ok(http::Response::new(Body::from(b"ok".to_vec()))))
}

fn table() -> Router<Handler<Base>> {
    let mut table = Router::builder();
    table.route("/", Verb::Get, ok()).expect("a route");
    table.route("/login", Verb::Post, ok()).expect("a route");
    table.route("/api/items", Verb::Get, ok()).expect("a route");
    table.build().expect("a table")
}

/// Two groups: a tight one on the login form, a loose one on everything.
fn config() -> RateLimitConfig {
    RateLimitConfig::rules([
        Rule::new("login", 2, 60)
            .paths(["/login"])
            .methods(["POST"]),
        Rule::new("site", 5, 60),
    ])
    .ip_header("fly-client-ip")
}

/// A store on a clock the test moves, starting at the top of a minute.
fn store() -> (Arc<MemoryStore>, Arc<AtomicU64>) {
    let now = Arc::new(AtomicU64::new(6_000));
    let clock = now.clone();
    let store = MemoryStore::with_clock(move || Duration::from_secs(clock.load(Ordering::SeqCst)));
    (Arc::new(store), now)
}

fn local(
    config: RateLimitConfig,
    store: Arc<MemoryStore>,
) -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(RateLimit::with_store(config, store))
        .build()
        .expect("an application");
    TestClient::serving(app, table())
}

fn from(ip: &str, request: TestRequest) -> TestRequest {
    request.header("fly-client-ip", ip)
}

fn login(ip: &str) -> TestRequest {
    from(ip, TestRequest::post("/login").form(&[("user", "a")]))
}

fn assert_refused(response: &TestResponse, rule: &str, limit: &str, retry: &str) {
    response
        .assert_status(429)
        .assert_header("retry-after", retry)
        .assert_header("ratelimit-limit", limit)
        .assert_header("ratelimit-remaining", "0")
        .assert_header("ratelimit-reset", retry);
    assert!(
        response.text().contains(&format!("rule `{rule}`")),
        "{}",
        response.text()
    );
}

#[test]
fn the_login_group_refuses_the_third_post_from_one_address() {
    let (store, _) = store();
    let client = local(config(), store);
    client
        .send(login("192.0.2.1"))
        .assert_ok()
        .assert_header("ratelimit-remaining", "1");
    client
        .send(login("192.0.2.1"))
        .assert_ok()
        .assert_header("ratelimit-remaining", "0");
    assert_refused(&client.send(login("192.0.2.1")), "login", "2", "60");

    // Another address has its own count.
    client.send(login("192.0.2.2")).assert_ok();
    // And a GET is not in the login group.
    client
        .send(from("192.0.2.1", TestRequest::get("/")))
        .assert_ok();
}

#[test]
fn the_window_ends_and_the_count_starts_again() {
    let (store, now) = store();
    let client = local(config(), store);
    for _ in 0..2 {
        client.send(login("192.0.2.1")).assert_ok();
    }
    now.store(6_045, Ordering::SeqCst);
    assert_refused(&client.send(login("192.0.2.1")), "login", "2", "15");
    now.store(6_060, Ordering::SeqCst);
    client.send(login("192.0.2.1")).assert_ok();
}

#[test]
fn the_headers_report_the_rule_nearest_its_limit() {
    let (store, _) = store();
    let client = local(config(), store);
    // One POST counts against both rules: login has 1 left of 2, site 4
    // of 5. The client is told the tighter one.
    client
        .send(login("192.0.2.1"))
        .assert_header("ratelimit-limit", "2")
        .assert_header("ratelimit-policy", "2;w=60");
    client
        .send(from("192.0.2.1", TestRequest::get("/api/items")))
        .assert_header("ratelimit-limit", "5")
        .assert_header("ratelimit-remaining", "3");
}

#[test]
fn a_signed_in_user_is_counted_by_account() {
    let (store, _) = store();
    let config = RateLimitConfig::rules([Rule::new("api", 1, 60)
        .paths(["/api"])
        .key([Key::User, Key::Ip])])
    .ip_header("fly-client-ip");
    let app = AppBuilder::new()
        .plugin_hooking(RateLimit::with_store(config, store))
        .build()
        .expect("an application");
    let client = TestClient::serving(app, table()).seeding(|state| {
        state.insert(Subject("42".into()));
    });
    // Two addresses, one account, one count.
    client
        .send(from("192.0.2.1", TestRequest::get("/api/items")))
        .assert_ok();
    client
        .send(from("192.0.2.2", TestRequest::get("/api/items")))
        .assert_status(429);
}

#[test]
fn a_request_with_no_key_is_not_counted() {
    let (store, _) = store();
    let client = local(config(), store.clone());
    for _ in 0..10 {
        client.get("/").assert_ok();
    }
    assert!(store.is_empty());
}

#[test]
fn a_rule_that_counts_nothing_stops_the_build() {
    let built = AppBuilder::new()
        .plugin_hooking(RateLimit::new(RateLimitConfig::rules([Rule::new(
            "s", 1, 60,
        )
        .key([Key::Session])])))
        .build();
    assert!(
        built.is_err(),
        "a session rule with no cookie name was accepted"
    );
}

#[test]
fn the_shared_plugin_counts_through_any_store() {
    let (store, _) = store();
    let app = AppBuilder::new()
        .plugin_hooking(SharedRateLimit::new(SharedConfig {
            config: config(),
            store,
        }))
        .build()
        .expect("an application");
    let client = TestClient::serving(app, table());
    brocanth_test::rt::block_on(async {
        client.send_async(login("192.0.2.1")).await.assert_ok();
        client.send_async(login("192.0.2.1")).await.assert_ok();
        let refused = client.send_async(login("192.0.2.1")).await;
        assert_refused(&refused, "login", "2", "60");
    });
}
