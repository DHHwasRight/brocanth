//! The flash, through the whole pipeline: a form posts, the page writes a
//! message and redirects, and the next page shows it once.

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{HookFamilies, Response};
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_middleware::{Flash, FlashConfig, FlashPlugin};
use brocanth_test::{TestClient, TestRequest, TestResponse};

const LAYOUT: &str = r#"<!DOCTYPE html><html><body><div id="flash" role="status"></div><main>Home</main></body></html>"#;

fn home() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| {
        Ok(http::Response::builder()
            .status(200)
            .header("content-type", "text/html; charset=utf-8")
            .body(Body::from(LAYOUT.as_bytes().to_vec()))
            .expect("a response"))
    })
}

fn save() -> Handler<Base> {
    Box::new(|cx: &mut RouteCx<'_, Base>| {
        cx.state_mut()
            .get_mut::<Flash>()
            .expect("the read hook ran")
            .success("Saved.");
        let response: Response = http::Response::builder()
            .status(303)
            .header("location", "/")
            .body(Body::from(Vec::new()))
            .expect("a response");
        Ok(response)
    })
}

fn client() -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    client_with(FlashConfig::default().insecure())
}

fn client_with(config: FlashConfig) -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(FlashPlugin::new(config))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    table.route("/", Verb::Get, home()).expect("a route");
    table.route("/save", Verb::Post, save()).expect("a route");
    TestClient::serving(app, table.build().expect("a table"))
}

/// `name=value` out of a `Set-Cookie`.
fn pair(response: &TestResponse) -> Option<String> {
    response
        .header("set-cookie")
        .and_then(|line| line.split(';').next())
        .map(str::to_string)
}

fn flash_text(response: &TestResponse) -> String {
    response
        .html()
        .tags("div")
        .into_iter()
        .find(|div| div.attr("id") == Some("flash"))
        .expect("the layout has the flash element")
        .text()
}

#[test]
fn a_message_written_before_a_redirect_shows_on_the_next_page_once() {
    let client = client();
    let posted = client.post("/save", &[]);
    posted.assert_redirects("/");
    let cookie = pair(&posted).expect("the redirect carries the message");

    let shown = client.send(TestRequest::get("/").header("cookie", &cookie));
    shown.assert_ok();
    assert_eq!(flash_text(&shown).trim(), "Saved.");
    let cleared = shown.header("set-cookie").expect("the cookie is cleared");
    assert!(cleared.contains("Max-Age=0"), "{cleared}");

    // The browser dropped the cookie, so the next page is quiet.
    let again = client.get("/");
    assert_eq!(flash_text(&again).trim(), "");
}

#[test]
fn a_page_with_no_message_sets_no_cookie() {
    let client = client();
    let page = client.get("/");
    assert!(page.header("set-cookie").is_none());
}

#[test]
fn a_cookie_name_that_cannot_go_in_a_header_stops_the_build() {
    let built = AppBuilder::new()
        .plugin_hooking(FlashPlugin::new(FlashConfig {
            cookie: "a b".into(),
            ..FlashConfig::default()
        }))
        .build();
    let Err(why) = built else {
        panic!("a cookie name with a space was accepted");
    };
    assert!(why.to_string().contains("help:"), "{why}");
}

/// Two instances behind one balancer: the second reads the message the
/// first wrote when both have the key in `key_env`.
#[test]
fn two_instances_with_the_key_in_key_env_read_each_others_messages() {
    // A name no other test reads, so setting it races with nothing.
    std::env::set_var("A046_FLASH_SHARED_KEY", "a key both instances read");
    let config = || FlashConfig {
        key_env: "A046_FLASH_SHARED_KEY".into(),
        ..FlashConfig::default().insecure()
    };
    let first = client_with(config());
    let second = client_with(config());
    let cookie = pair(&first.post("/save", &[])).expect("a message");
    let shown = second.send(TestRequest::get("/").header("cookie", &cookie));
    assert_eq!(flash_text(&shown).trim(), "Saved.");
}

/// Without a key each process draws its own: another instance cannot read
/// the cookie, and it shows nothing rather than an unsigned message.
#[test]
fn without_a_key_another_instance_ignores_the_cookie() {
    let config = || FlashConfig {
        key_env: "A046_FLASH_KEY_NOBODY_SETS".into(),
        ..FlashConfig::default().insecure()
    };
    let first = client_with(config());
    let second = client_with(config());
    let cookie = pair(&first.post("/save", &[])).expect("a message");
    let shown = second.send(TestRequest::get("/").header("cookie", &cookie));
    assert_eq!(flash_text(&shown).trim(), "");
    let same = first.send(TestRequest::get("/").header("cookie", &cookie));
    assert_eq!(
        flash_text(&same).trim(),
        "Saved.",
        "its own instance reads it"
    );
}

/// The default sends the cookie over HTTPS only.
#[test]
fn the_default_cookie_is_secure() {
    let posted = client_with(FlashConfig::default()).post("/save", &[]);
    let line = posted.header("set-cookie").expect("a message");
    assert!(line.contains("Secure"), "{line}");
}
