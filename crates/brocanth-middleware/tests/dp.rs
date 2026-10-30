//! dp-swap's mutating verbs, through the whole pipeline.
//!
//! The client script sends `DP-Request`, the CSRF token in a header, and a
//! form body. These tests send what it sends and assert on what it reads
//! back: the status, the `DP-*` headers, the out-of-band fragments, and
//! whether the layout came with the page.

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{HookFamilies, Response};
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_input::{rerender, Entrance, FieldErrors};
use brocanth_middleware::dp::{Directive, DpRequest, DpSwap, Oob};
use brocanth_middleware::{Csrf, CsrfConfig, CsrfToken};
use brocanth_test::{TestClient, TestRequest, TestResponse};
use zod::{Entry, Text};

#[derive(Entry)]
#[allow(dead_code)]
struct Item {
    #[check(Text::new().min(3))]
    name: String,
}

/// The form, the same function on the first render and on the 422.
fn form(errors: &FieldErrors) -> String {
    format!(
        r#"<form id="add" method="post" action="/items" dp-post dp-target="this">
             <input name="name">{}
             <button>Add</button>
           </form>"#,
        errors.render("name")
    )
}

/// The layout, unless the client said it holds it.
fn page(cx: &RouteCx<'_, Base>, body: &str) -> String {
    let partial = cx.state().get::<DpRequest>().map_or(0, |dp| dp.partial);
    if partial > 0 {
        return body.to_string();
    }
    let meta = cx
        .state()
        .get::<CsrfToken>()
        .map(CsrfToken::meta_tag)
        .unwrap_or_default();
    format!(
        r#"<!DOCTYPE html><html><head>{meta}</head><body><nav id="chrome">Shop</nav><main dp-layout="/">{body}</main></body></html>"#
    )
}

fn html(status: u16, body: String) -> Response {
    http::Response::builder()
        .status(status)
        .header("content-type", "text/html; charset=utf-8")
        .body(Body::from(body.into_bytes()))
        .expect("a response")
}

/// `brocanth-input` answers in `brocanth-server`'s type; the core wants
/// its own.
fn core(sent: brocanth_server::Response) -> Response {
    let mut out = http::Response::builder().status(sent.status);
    for (name, value) in sent.headers() {
        out = out.header(name, value);
    }
    let bytes = sent.body.bytes().map(|b| b.to_vec()).unwrap_or_default();
    out.body(Body::from(bytes)).expect("a response")
}

fn show() -> Handler<Base> {
    Box::new(|cx: &mut RouteCx<'_, Base>| {
        let body = page(cx, &form(&FieldErrors::none()));
        Ok(html(200, body))
    })
}

fn add() -> Handler<Base> {
    Box::new(|cx: &mut RouteCx<'_, Base>| {
        let request = cx.request();
        let kind = request
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok());
        let at = Entrance::new(kind, "", request.body());
        let input = at.input().expect("a form");
        if let Err(faults) = Item::parse(input) {
            return Ok(core(rerender(&faults, form)));
        }
        // The new row, and the badge elsewhere on the page.
        let body = format!(
            "{}<li>added</li>",
            Oob::replace(r#"<span id="count">1</span>"#)
        );
        let mut response = html(200, body);
        Directive::swap()
            .retarget("#list")
            .reswap(brocanth_middleware::dp::Style::BeforeEnd)
            .trigger("item:added")
            .apply(response.headers_mut())
            .expect("valid headers");
        Ok(response)
    })
}

fn done() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| {
        let mut response = html(200, String::new());
        Directive::redirect("/items")
            .apply(response.headers_mut())
            .expect("valid headers");
        Ok(response)
    })
}

fn client() -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(Csrf::new(CsrfConfig::default().insecure()))
        .plugin_hooking(DpSwap::new(()))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    table.route("/items", Verb::Get, show()).expect("a route");
    table.route("/items", Verb::Post, add()).expect("a route");
    table
        .route("/checkout", Verb::Post, done())
        .expect("a route");
    TestClient::serving(app, table.build().expect("a table"))
}

/// The cookie the first page hands out.
fn token(client: &TestClient<Base, impl HookFamilies, Handler<Base>>) -> String {
    let first = client.get("/items");
    let set = first
        .header("set-cookie")
        .expect("a first visit gets a cookie");
    let value = set
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .map(|(_, v)| v.to_string())
        .expect("name=value");
    // The page carries the same value for the script to read.
    assert!(
        first.text().contains(&format!(
            r#"<meta name="csrf-token" content="{value}" data-header="x-csrf-token">"#
        )),
        "{}",
        first.text()
    );
    value
}

/// What `dpmutate.js` sends for a `dp-post`.
fn dp_post(path: &str, token: Option<&str>, form: &[(&str, &str)]) -> TestRequest {
    let mut request = TestRequest::post(path)
        .form(form)
        .header("dp-request", "true");
    if let Some(token) = token {
        request = request
            .header("cookie", &format!("csrf_token={token}"))
            .header("x-csrf-token", token);
    }
    request
}

fn vary(response: &TestResponse) -> Vec<String> {
    response
        .headers()
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("vary"))
        .map(|(_, value)| value.clone())
        .collect()
}

#[test]
fn a_dp_post_without_the_token_is_refused() {
    let client = client();
    client
        .send(dp_post("/items", None, &[("name", "lamp")]))
        .assert_status(403);
}

#[test]
fn a_dp_post_with_the_token_in_the_header_is_accepted() {
    let client = client();
    let token = token(&client);
    client
        .send(dp_post("/items", Some(&token), &[("name", "lamp")]))
        .assert_ok();
}

#[test]
fn a_wrong_token_in_the_header_is_refused() {
    let client = client();
    let token = token(&client);
    let wrong = "0".repeat(token.len());
    let request = TestRequest::post("/items")
        .form(&[("name", "lamp")])
        .header("dp-request", "true")
        .header("cookie", &format!("csrf_token={token}"))
        .header("x-csrf-token", &wrong);
    client.send(request).assert_status(403);
}

#[test]
fn an_invalid_form_comes_back_as_a_422_fragment_with_its_errors() {
    let client = client();
    let token = token(&client);
    let response = client.send(dp_post("/items", Some(&token), &[("name", "ab")]));

    response.assert_status(422);
    let html = response.html();
    html.assert_field("name");
    assert!(
        response.text().contains(r#"id="name-error""#),
        "the error sits beside its field: {}",
        response.text()
    );
    // A fragment for the form's place, not a page.
    assert!(!response.text().contains("<nav"), "{}", response.text());
}

#[test]
fn a_partial_request_gets_the_page_without_its_layout() {
    let client = client();

    let whole = client.get("/items");
    whole.assert_ok();
    assert!(whole.text().contains(r#"<nav id="chrome">"#));

    let partial = client.send(
        TestRequest::get("/items")
            .header("dp-request", "true")
            .header("dp-partial", "1"),
    );
    partial.assert_ok();
    assert!(!partial.text().contains("<nav"), "{}", partial.text());
    assert!(partial.text().contains(r#"<form id="add""#));
}

#[test]
fn every_response_varies_by_the_dp_headers() {
    // Both shapes of the same URL, so a cache must key on the headers.
    let client = client();
    let whole = client.get("/items");
    assert!(
        vary(&whole).iter().any(|v| v.contains("DP-Partial")),
        "{:?}",
        whole.headers()
    );
}

#[test]
fn a_write_can_steer_the_swap_and_send_an_out_of_band_fragment() {
    let client = client();
    let token = token(&client);
    let response = client.send(dp_post("/items", Some(&token), &[("name", "lamp")]));

    response
        .assert_ok()
        .assert_header("dp-retarget", "#list")
        .assert_header("dp-reswap", "beforeend")
        .assert_header("dp-trigger", "item:added");
    assert!(
        response
            .text()
            .contains(r#"<span dp-swap-oob="outerHTML" id="count">1</span>"#),
        "{}",
        response.text()
    );
    assert!(response.text().contains("<li>added</li>"));
}

#[test]
fn a_redirect_is_a_header_the_client_follows_not_a_status() {
    let client = client();
    let token = token(&client);
    let response = client.send(dp_post("/checkout", Some(&token), &[]));
    response.assert_ok().assert_header("dp-redirect", "/items");
    assert!(response.header("dp-retarget").is_none());
}
