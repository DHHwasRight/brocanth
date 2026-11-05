//! The beacon: the snippet a static site's pages carry.

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::HookFamilies;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_analytics::{Beacon, BeaconConfig};
use brocanth_test::{TestClient, TestRequest};

fn answer(content_type: &'static str, body: &'static [u8]) -> Handler<Base> {
    Box::new(move |_cx: &mut RouteCx<'_, Base>| {
        Ok(http::Response::builder()
            .header("content-type", content_type)
            .body(Body::from(body.to_vec()))
            .expect("a response"))
    })
}

fn table() -> Router<Handler<Base>> {
    let mut table = Router::builder();
    table
        .route(
            "/",
            Verb::Get,
            answer(
                "text/html; charset=utf-8",
                b"<html><body><h1>Docs</h1></body></html>",
            ),
        )
        .unwrap();
    table
        .route(
            "/part",
            Verb::Get,
            answer("text/html", b"<main>part</main>"),
        )
        .unwrap();
    table
        .route(
            "/data.json",
            Verb::Get,
            answer("application/json", b"{\"a\":\"</body>\"}"),
        )
        .unwrap();
    table.build().unwrap()
}

fn client(config: BeaconConfig) -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(Beacon::new(config))
        .build()
        .expect("an application");
    TestClient::serving(app, table())
}

fn collecting(respect_gpc: bool) -> BeaconConfig {
    BeaconConfig {
        collector: Some("https://app.example.com/_a/v".to_string()),
        respect_gpc,
    }
}

#[test]
fn a_page_carries_the_snippet_before_its_body_ends() {
    let response = client(collecting(false)).send(TestRequest::get("/"));
    let html = response.text();
    let tag = html
        .find("<script data-collector=\"https://app.example.com/_a/v\">")
        .expect("the snippet");
    assert!(tag < html.rfind("</body>").unwrap());
    assert!(html.contains("sendBeacon"));
    assert!(!html.contains("data-gpc=\"1\""));
}

#[test]
fn respecting_gpc_is_on_the_tag() {
    let html = client(collecting(true))
        .send(TestRequest::get("/"))
        .text()
        .to_string();
    assert!(html.contains("data-gpc=\"1\""));
}

#[test]
fn a_partial_and_json_are_left_alone() {
    let client = client(collecting(false));
    assert_eq!(
        client
            .send(TestRequest::get("/part").header("dp-partial", "1"))
            .text(),
        "<main>part</main>"
    );
    assert_eq!(
        client.send(TestRequest::get("/data.json")).text(),
        "{\"a\":\"</body>\"}"
    );
}

#[test]
fn without_a_collector_nothing_is_added() {
    let html = client(BeaconConfig::default())
        .send(TestRequest::get("/"))
        .text()
        .to_string();
    assert_eq!(html, "<html><body><h1>Docs</h1></body></html>");
}

#[test]
fn a_collector_that_is_not_a_url_stops_the_build() {
    let built = AppBuilder::new()
        .plugin_hooking(Beacon::new(BeaconConfig {
            collector: Some("app.example.com/_a/v".to_string()),
            respect_gpc: false,
        }))
        .build();
    let Err(error) = built else {
        panic!("a relative collector was accepted");
    };
    assert!(error.to_string().contains("help:"), "{error}");
}
