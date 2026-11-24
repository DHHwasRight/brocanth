//! `CORSMethodMiddleware`: a preflight answers the verbs of the route.
//!
//! The shape a generated project has: `pages::router()` builds the table,
//! and the `Cors` registration asks a second copy of it for the verbs of a
//! path. Two pages: `/notes` answers GET, `/notes/{id}` GET and DELETE.

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{HookFamilies, Response};
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_middleware::{Cors, CorsConfig};
use brocanth_test::{TestClient, TestRequest, TestResponse};

const ORIGIN: &str = "https://app.example.com";

fn page(text: &'static str) -> Handler<Base> {
    Box::new(move |_cx: &mut RouteCx<'_, Base>| -> Result<Response, _> {
        Ok(http::Response::new(Body::from(text.as_bytes().to_vec())))
    })
}

/// `pages::router()` in a generated project.
fn router() -> Router<Handler<Base>> {
    let mut table = Router::builder();
    table
        .route("/notes", Verb::Get, page("list"))
        .expect("a route");
    table
        .route("/notes/{id}", Verb::Get, page("one"))
        .expect("a route");
    table
        .route("/notes/{id}", Verb::Delete, page("gone"))
        .expect("a route");
    table.build().expect("a table")
}

fn client(config: CorsConfig) -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(Cors::new(config))
        .build()
        .expect("an application");
    TestClient::serving(app, router())
}

/// What the generated registration passes: a function over the table.
fn routed() -> CorsConfig {
    let table = router();
    CorsConfig::from([ORIGIN])
        .methods([
            http::Method::GET,
            http::Method::HEAD,
            http::Method::POST,
            http::Method::DELETE,
        ])
        .route_verbs(move |path| table.allow_for(path))
}

fn preflight(path: &str, asking: &str) -> TestRequest {
    TestRequest::new(Verb::Options, path)
        .header("origin", ORIGIN)
        .header("access-control-request-method", asking)
}

fn permits(response: &TestResponse) -> bool {
    response.headers().iter().any(|(name, _)| {
        name.to_ascii_lowercase()
            .starts_with("access-control-allow-")
    })
}

#[test]
fn a_get_and_delete_page_allows_both_and_nothing_else() {
    let client = client(routed());
    let response = client.send(preflight("/notes/7", "DELETE"));
    response
        .assert_status(204)
        .assert_header("access-control-allow-origin", ORIGIN)
        .assert_header("access-control-allow-methods", "GET, HEAD, DELETE");
}

#[test]
fn a_get_only_page_refuses_a_delete_preflight() {
    let client = client(routed());
    let response = client.send(preflight("/notes", "DELETE"));
    response.assert_status(204);
    assert!(!permits(&response), "{:?}", response.headers());

    let response = client.send(preflight("/notes", "GET"));
    response.assert_header("access-control-allow-methods", "GET, HEAD");
}

#[test]
fn a_path_with_no_route_refuses_every_preflight() {
    let client = client(routed());
    let response = client.send(preflight("/nowhere", "GET"));
    assert!(!permits(&response), "{:?}", response.headers());
}

#[test]
fn the_real_request_reaches_the_page_and_is_marked() {
    let client = client(routed());
    let response = client.send(TestRequest::delete("/notes/7").header("origin", ORIGIN));
    response
        .assert_ok()
        .assert_header("access-control-allow-origin", ORIGIN);
    assert_eq!(response.text(), "gone");
}

#[test]
fn without_the_function_the_configured_list_answers_every_path() {
    let client = client(CorsConfig::from([ORIGIN]));
    let response = client.send(preflight("/notes", "DELETE"));
    response.assert_header("access-control-allow-methods", "GET, HEAD, POST");
}
