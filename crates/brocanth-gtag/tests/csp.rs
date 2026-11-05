//! The security headers let the tag load once the visitor accepts.
//!
//! What a generated project with both modules registers: the
//! Content-Security-Policy allows the consent script by its hash and
//! Google's origins on top, and nothing else changes.

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::Response;
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_gtag::{Gtag, GtagConfig, CSP, SCRIPT};
use brocanth_middleware::{script_hash, SecurityHeaders, SecurityHeadersConfig};
use brocanth_test::TestClient;

fn page() -> Handler<Base> {
    Box::new(|_cx: &mut RouteCx<'_, Base>| {
        Ok::<Response, _>(
            http::Response::builder()
                .status(200)
                .header("content-type", "text/html")
                .body(Body::from(b"<html><body>Hi</body></html>".to_vec()))
                .expect("a response"),
        )
    })
}

/// One directive's sources, from a policy header.
fn directive<'a>(policy: &'a str, name: &str) -> Vec<&'a str> {
    policy
        .split(';')
        .map(str::trim)
        .find_map(|d| d.strip_prefix(name).map(str::trim))
        .map(|sources| sources.split(' ').collect())
        .unwrap_or_default()
}

#[test]
fn the_policy_allows_the_consent_script_and_googles_origins() {
    let app = AppBuilder::new()
        .plugin_hooking(SecurityHeaders::new(
            SecurityHeadersConfig::default().page_script(SCRIPT, CSP),
        ))
        .plugin_hooking(Gtag::new(GtagConfig::default().measuring("G-ABC123")))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    table.route("/", Verb::Get, page()).expect("a route");
    let client = TestClient::serving(app, table.build().expect("a table"));

    let response = client.get("/");
    let policy = response
        .header("content-security-policy")
        .expect("a policy")
        .to_string();

    // The inline script on the page is the one the policy names.
    let text = response.text();
    let start = text.rfind("<script>").expect("the consent script") + "<script>".len();
    let end = start + text[start..].find("</script>").expect("it closes");
    let on_page = &text[start..end];
    assert_eq!(on_page, SCRIPT);
    let script_src = directive(&policy, "script-src");
    assert!(
        script_src.contains(&script_hash(on_page).as_str()),
        "{policy}"
    );
    assert!(
        script_src.contains(&"https://www.googletagmanager.com"),
        "{policy}"
    );
    assert!(!script_src.contains(&"'unsafe-inline'"), "{policy}");

    let connect = directive(&policy, "connect-src");
    assert!(connect.contains(&"'self'"), "the base stays: {policy}");
    assert!(
        connect.contains(&"https://*.google-analytics.com"),
        "{policy}"
    );
    assert!(
        connect.contains(&"https://*.analytics.google.com"),
        "{policy}"
    );
    let img = directive(&policy, "img-src");
    assert!(img.contains(&"https://*.google-analytics.com"), "{policy}");
}

#[test]
fn an_origin_does_not_bring_back_a_directive_the_file_removed() {
    let headers = SecurityHeadersConfig::default()
        .csp(brocanth_middleware::Csp::default().without("img-src"))
        .page_script(SCRIPT, CSP);
    let policy = headers.csp.render(&headers.frames).expect("a policy");
    assert!(directive(&policy, "img-src").is_empty(), "{policy}");
    assert!(!directive(&policy, "connect-src").is_empty(), "{policy}");
}
