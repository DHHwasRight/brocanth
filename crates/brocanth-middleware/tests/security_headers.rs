//! The security headers, through the whole pipeline, on a page that
//! inlines dp-swap's scripts the way a layout does.

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::{App, AppBuilder};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{Both, HookFamilies, HookSource, NoHooks};
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_middleware::dp::{PREFETCH, SCRIPT};
use brocanth_middleware::{script_hash, Frames, SecurityHeaders, SecurityHeadersConfig};
use brocanth_test::TestClient;

/// A layout, with both scripts inline and nothing between the tags but
/// the constant. A template that adds a newline changes the hash.
fn page() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| {
        let body = format!(
            "<!DOCTYPE html><html><body><main>hi</main>\
             <script>{SCRIPT}</script><script>{PREFETCH}</script></body></html>"
        );
        Ok(http::Response::builder()
            .header("content-type", "text/html; charset=utf-8")
            .body(Body::from(body.into_bytes()))
            .expect("a response"))
    })
}

/// A page that must be framed, and says so itself.
fn widget() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| {
        Ok(http::Response::builder()
            .header("x-frame-options", "SAMEORIGIN")
            .body(Body::from(b"widget".to_vec()))
            .expect("a response"))
    })
}

fn client(config: SecurityHeadersConfig) -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(SecurityHeaders::new(config))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    table.route("/", Verb::Get, page()).expect("a route");
    table
        .route("/widget", Verb::Get, widget())
        .expect("a route");
    TestClient::serving(app, table.build().expect("a table"))
}

/// Every inline script on the page, as the browser hashes it.
fn inline_scripts(html: &str) -> Vec<&str> {
    html.split("<script>")
        .skip(1)
        .filter_map(|rest| rest.split_once("</script>").map(|(text, _)| text))
        .collect()
}

#[test]
fn every_inline_script_on_the_page_is_allowed_by_the_policy() {
    let response = client(SecurityHeadersConfig::default()).get("/");
    response.assert_ok();
    let csp = response
        .header("content-security-policy")
        .expect("a policy");
    let scripts = inline_scripts(response.text());
    assert_eq!(scripts.len(), 2);
    for text in scripts {
        assert!(csp.contains(&script_hash(text)), "{csp}");
    }
    assert!(!csp.contains("script-src 'self' 'unsafe-inline'"), "{csp}");
}

#[test]
fn a_plain_http_request_gets_every_header_but_hsts() {
    let response = client(SecurityHeadersConfig::default()).get("/");
    response
        .assert_header("x-content-type-options", "nosniff")
        .assert_header("referrer-policy", "strict-origin-when-cross-origin")
        .assert_header("x-frame-options", "DENY");
    assert!(response.header("permissions-policy").is_some());
    assert!(response.header("strict-transport-security").is_none());
}

#[test]
fn behind_tls_every_response_gets_hsts() {
    let response = client(SecurityHeadersConfig::default().behind_tls()).get("/");
    response.assert_header("strict-transport-security", "max-age=31536000");
    // A 404 too: a browser that reaches a missing page over HTTPS must
    // remember the rule as well.
    client(SecurityHeadersConfig::default().behind_tls())
        .get("/missing")
        .assert_status(404)
        .assert_header("strict-transport-security", "max-age=31536000");
}

#[test]
fn the_handler_s_own_header_wins() {
    client(SecurityHeadersConfig::default().frames(Frames::Deny))
        .get("/widget")
        .assert_header("x-frame-options", "SAMEORIGIN");
}

#[test]
fn the_hook_record_is_nameable_and_a_bad_policy_stops_the_build() {
    type Hooks = Both<<SecurityHeaders as HookSource>::Families, NoHooks>;
    let app: App<Base, Hooks> = AppBuilder::new()
        .plugin_hooking(SecurityHeaders::new(SecurityHeadersConfig::default()))
        .build()
        .expect("an application");
    assert!(app
        .plugin_named::<SecurityHeaders>("security_headers")
        .is_some());

    let refused = AppBuilder::new()
        .plugin_hooking(SecurityHeaders::new(
            SecurityHeadersConfig::default().referrer_policy("nope"),
        ))
        .build();
    assert!(refused.is_err());
}
