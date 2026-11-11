//! What a browser receives, through the whole pipeline.

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{HookFamilies, Response};
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_gtag::{Gtag, GtagConfig};
use brocanth_test::{TestClient, TestRequest};

const PAGE: &str = "<!DOCTYPE html><html><head><title>Shop</title></head>\
                    <body><main>Hello</main></body></html>";

fn answer(kind: &'static str, body: &'static str) -> Handler<Base> {
    Box::new(move |_cx: &mut RouteCx<'_, Base>| {
        Ok::<Response, _>(
            http::Response::builder()
                .status(200)
                .header("content-type", kind)
                .body(Body::from(body.as_bytes().to_vec()))
                .expect("a response"),
        )
    })
}

fn client(config: GtagConfig) -> TestClient<Base, impl HookFamilies, Handler<Base>> {
    let app = AppBuilder::new()
        .plugin_hooking(Gtag::new(config))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    for path in ["/", "/admin", "/admin/users"] {
        table
            .route(path, Verb::Get, answer("text/html; charset=utf-8", PAGE))
            .expect("a route");
    }
    table
        .route("/data.json", Verb::Get, answer("application/json", "{}"))
        .expect("a route");
    TestClient::serving(app, table.build().expect("a table"))
}

/// On, with the admin kept out as a generated project keeps it: from the
/// admin's module file, through `exclude_path`.
fn on() -> GtagConfig {
    GtagConfig::default()
        .measuring("G-ABC123")
        .exclude_path("/admin")
}

#[test]
fn before_consent_no_google_script_is_requested_and_every_consent_is_denied() {
    let page = client(on()).get("/");
    let text = page.text();
    assert!(text.contains(r#"id="brocanth-consent""#), "{text}");
    // No element asks for Google's script: the consent script builds the
    // URL only after the visitor accepts.
    assert!(!text.contains(r#"src="https://www.googletagmanager.com"#));
    assert!(!text.contains("<script async"));
    for kind in [
        "ad_storage",
        "ad_user_data",
        "ad_personalization",
        "analytics_storage",
    ] {
        assert!(
            text.contains(&format!(r#"{kind}: "denied""#)),
            "{kind} defaults to denied"
        );
    }
    // The markup sits inside the body, after the page's own content.
    let banner = text.find("brocanth-consent").expect("the banner");
    assert!(text.find("<main>").expect("the page") < banner);
    assert!(banner < text.rfind("</body>").expect("the body closes"));
}

#[test]
fn global_privacy_control_and_do_not_track_mean_no_banner_and_no_tag() {
    let client = client(on());
    for (name, value) in [("sec-gpc", "1"), ("dnt", "1")] {
        let page = client.send(TestRequest::get("/").header(name, value));
        assert!(!page.text().contains("brocanth-consent"), "{name}");
        assert!(!page.text().contains("googletagmanager"), "{name}");
    }
    let vary = client.get("/");
    assert!(
        vary.header("vary").is_some_and(|v| v.contains("Sec-GPC")),
        "a shared cache keeps one copy per signal"
    );
}

#[test]
fn the_admin_never_gets_the_tag() {
    let client = client(on());
    for path in ["/admin", "/admin/users"] {
        assert!(
            !client.get(path).text().contains("brocanth-consent"),
            "{path}"
        );
    }
}

#[test]
fn off_by_default_a_fragment_and_json_are_left_alone() {
    assert_eq!(client(GtagConfig::default()).get("/").text(), PAGE);
    let client = client(on());
    let partial = client.send(TestRequest::get("/").header("dp-partial", "1"));
    assert_eq!(
        partial.text(),
        PAGE,
        "a partial goes into a page that has it"
    );
    assert_eq!(client.get("/data.json").text(), "{}");
}

#[test]
fn a_malformed_measurement_id_stops_the_application_at_startup() {
    let built = AppBuilder::new()
        .plugin_hooking(Gtag::new(GtagConfig::default().measuring("UA-12345-1")))
        .build();
    let Err(error) = built else {
        panic!("a Universal Analytics id must not start");
    };
    assert!(error.to_string().contains("help:"), "{error}");
}

#[test]
fn a_cookie_domain_reaches_the_script_and_a_bad_one_stops_startup() {
    let mut config = on();
    config.cookie_domain = Some("example.co.uk".into());
    let page = client(config).get("/");
    let text = page.text();
    assert!(text.contains(r#""domain":"example.co.uk""#), "{text}");

    let mut bad = on();
    bad.cookie_domain = Some("https://example.com/".into());
    let error = bad.check().expect_err("a URL is not a domain");
    assert!(error.contains("help:"), "{error}");
}

#[test]
fn without_a_consent_log_the_script_reports_nothing_and_the_path_is_not_answered() {
    let client = client(on());
    assert!(client.get("/").text().contains(r#""log":false"#));
    assert_eq!(
        client
            .get("/__consent?c=granted&id=0123456789abcdef&v=1a2b")
            .status(),
        404
    );
}

#[test]
fn a_consent_log_records_the_choice_and_nothing_that_names_the_visitor() {
    let dir = std::env::temp_dir().join(format!("brocanth-gtag-{}", std::process::id()));
    let file = dir.join("consent.jsonl");
    let _ = std::fs::remove_file(&file);
    let mut config = on();
    config.consent_log = Some(file.display().to_string());
    let version = config.banner_version();
    let client = client(config);
    assert!(client.get("/").text().contains(r#""log":true"#));

    let answer = client.send(
        TestRequest::get(format!(
            "/__consent?c=granted&id=0123456789abcdef&v={version}"
        ))
        .header("x-forwarded-for", "203.0.113.7")
        .header("user-agent", "Mozilla/5.0 Firefox/130.0"),
    );
    assert_eq!(answer.status(), 204);
    assert_eq!(answer.header("cache-control"), Some("no-store"));
    assert_eq!(
        client
            .get("/__consent?c=maybe&id=0123456789abcdef&v=1")
            .status(),
        400
    );
    assert_eq!(
        client
            .send(TestRequest::post(
                "/__consent?c=denied&id=0123456789abcdef&v=1"
            ))
            .status(),
        405
    );

    let log = std::fs::read_to_string(&file).expect("the log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 1, "{log}");
    let line = lines[0];
    assert!(line.contains(r#""choice":"granted""#), "{line}");
    assert!(line.contains(r#""id":"0123456789abcdef""#), "{line}");
    assert!(line.contains(&format!(r#""seen":"{version}""#)), "{line}");
    assert!(line.contains(&format!(r#""banner":"{version}""#)), "{line}");
    assert!(!line.contains("203.0.113.7"), "no address: {line}");
    assert!(!line.contains("Firefox"), "no user agent: {line}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_choice_is_read_strictly() {
    use brocanth_gtag::parse_choice;
    let ok = parse_choice("c=denied&id=abcdef12&v=00ff").expect("a choice");
    assert_eq!(ok.choice, "denied");
    for bad in [
        "",
        "c=granted",
        "c=yes&id=abcdef12&v=1",
        "c=granted&id=short&v=1",
        "c=granted&id=abcdef12&v=xyz",
        "c=granted&id=abcdef12&v=1&extra=1",
        "c=granted&id=abc\"def12&v=1",
    ] {
        assert!(parse_choice(bad).is_none(), "{bad:?}");
    }
}
