//! The plugin side: it registers, it provides, it offers, and it does not
//! mount.

mod common;

use std::sync::Arc;

use base64::Engine as _;
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_auth::apple::AppleKey;
use brocanth_auth::clock::{SystemClock, TestClock};
use brocanth_auth::http_client::StubHttp;
use brocanth_auth::memory::MemoryStore;
use brocanth_auth::oauth::{ClientConfig, ClientSecret};
use brocanth_auth::password::{PasswordCost, Passwords};
use brocanth_auth::plugin::{Auth, AuthConfig, AuthPlugin, ConfiguredProvider};
use brocanth_auth::providers::catalog::{APPLE, GITHUB, GOOGLE};
use brocanth_auth::session::{SessionConfig, SigningKey};
use common::TEST_EC_PEM;

type TestAuth = Auth<MemoryStore, StubHttp, TestClock>;

fn config(providers: Vec<ConfiguredProvider>) -> AuthConfig<MemoryStore, StubHttp, TestClock> {
    AuthConfig {
        store: Arc::new(MemoryStore::new()),
        http: Arc::new(StubHttp::new()),
        clock: Arc::new(TestClock::default()),
        signing_key: SigningKey::from_bytes([1u8; 32]),
        session: SessionConfig::default(),
        passwords: Arc::new(Passwords::new(PasswordCost::insecure_for_tests()).unwrap()),
        providers,
    }
}

fn google() -> ConfiguredProvider {
    ConfiguredProvider {
        provider: GOOGLE,
        client: ClientConfig::new("id", "secret", "https://app.test/cb"),
    }
}

#[test]
fn the_plugin_provides_auth_to_the_context_and_offers_middleware_without_mounting_it() {
    let app = AppBuilder::new()
        .plugin(AuthPlugin::new(config(vec![google()])))
        .setup(|app| {
            // The application mounts, at patterns it chose. The plugin was
            // never offered the chance.
            app.router().mount("/*", "auth.session");
            app.router().mount("/admin/*", "auth.require_authenticated");
            Ok(())
        })
        .build()
        .expect("build");

    let offered: Vec<&str> = app
        .router_plan()
        .offers()
        .iter()
        .map(|o| o.name.as_str())
        .collect();
    assert!(offered.contains(&"auth.session"));
    assert!(offered.contains(&"auth.require_authenticated"));
    assert!(app
        .router_plan()
        .offers()
        .iter()
        .all(|o| o.offered_by == "auth"));

    let mounted: Vec<&str> = app
        .router_plan()
        .mounts()
        .iter()
        .map(|m| m.pattern.as_str())
        .collect();
    assert_eq!(mounted, vec!["/*", "/admin/*"]);

    // A handler extracts `Auth` from the context, which compiles only because
    // the plugin was registered.
    let cx = app.context();
    let auth: &TestAuth = cx.extract();
    assert!(auth.provider("google").is_some());
    assert!(auth.provider("github").is_none());
    assert_eq!(auth.providers().len(), 1);
}

#[test]
fn mounting_something_nobody_offered_fails_the_build_with_a_suggestion() {
    let err = AppBuilder::new()
        .plugin(AuthPlugin::new(config(vec![google()])))
        .setup(|app| {
            app.router().mount("/*", "auth.sessions");
            Ok(())
        })
        .build()
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("auth.sessions"), "{message}");
    assert!(message.contains("auth.session"), "{message}");
}

#[test]
fn an_empty_client_id_stops_the_build_rather_than_the_first_sign_in() {
    let err = AppBuilder::new()
        .plugin(AuthPlugin::new(config(vec![ConfiguredProvider {
            provider: GITHUB,
            client: ClientConfig::new("", "secret", "https://app.test/cb"),
        }])))
        .build()
        .unwrap_err();
    assert!(err.to_string().contains("empty client id"), "{err}");
}

#[test]
fn an_unusable_apple_key_stops_the_build() {
    let err = AppBuilder::new()
        .plugin(AuthPlugin::new(config(vec![ConfiguredProvider {
            provider: APPLE,
            client: ClientConfig {
                client_id: "com.example.app".into(),
                secret: ClientSecret::Apple(AppleKey::new(
                    "TEAM",
                    "KEY",
                    "com.example.app",
                    "-----BEGIN PRIVATE KEY-----\nnot a key\n-----END PRIVATE KEY-----",
                )),
                redirect_uri: "https://app.test/cb".into(),
                scopes: None,
            },
        }])))
        .build()
        .unwrap_err();
    assert!(err.to_string().contains("Apple private key"), "{err}");
}

#[test]
fn a_real_apple_key_builds_and_signs_an_es256_client_secret() {
    let key = AppleKey::new("TEAMID1234", "KEYID5678", "com.example.app", TEST_EC_PEM);
    let clock = TestClock::at(1_700_000_000);
    let secret = key.client_secret(&clock).expect("sign");

    let header = jsonwebtoken::decode_header(&secret).expect("header");
    assert_eq!(header.alg, jsonwebtoken::Algorithm::ES256);
    assert_eq!(header.kid.as_deref(), Some("KEYID5678"));

    // The claims Apple requires, read back without verifying — the signature
    // is Apple's to check, the shape is ours.
    let payload = secret.split('.').nth(1).unwrap();
    let json: serde_json::Value = serde_json::from_slice(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload.as_bytes())
            .unwrap(),
    )
    .unwrap();
    assert_eq!(json["iss"], "TEAMID1234");
    assert_eq!(json["sub"], "com.example.app");
    assert_eq!(json["aud"], "https://appleid.apple.com");
    assert_eq!(json["iat"], 1_700_000_000);
    assert_eq!(json["exp"], 1_700_000_000 + 3600);

    // And it is minted fresh each time, rather than being a stored string that
    // expires six months into a deployment nobody is watching.
    clock.advance(10);
    assert_ne!(key.client_secret(&clock).unwrap(), secret);
}

#[test]
fn two_applications_in_one_process_keep_their_own_auth() {
    let a = AppBuilder::new()
        .plugin(AuthPlugin::new(config(vec![google()])))
        .build()
        .unwrap();
    let b = AppBuilder::new()
        .plugin(AuthPlugin::new(config(vec![ConfiguredProvider {
            provider: GITHUB,
            client: ClientConfig::new("id-b", "secret-b", "https://other.test/cb"),
        }])))
        .build()
        .unwrap();

    let (cx_a, cx_b) = (a.context(), b.context());
    let auth_a: &TestAuth = cx_a.extract();
    let auth_b: &TestAuth = cx_b.extract();
    assert!(auth_a.provider("google").is_some());
    assert!(auth_a.provider("github").is_none());
    assert!(auth_b.provider("github").is_some());
    assert!(auth_b.provider("google").is_none());
}

#[test]
fn the_plugin_also_builds_with_the_real_clock() {
    // The generic parameters are not a test affordance: a production
    // application names `SystemClock` and a real store here.
    let auth = Auth::build(AuthConfig {
        store: Arc::new(MemoryStore::new()),
        http: Arc::new(StubHttp::new()),
        clock: Arc::new(SystemClock),
        signing_key: SigningKey::generate().unwrap(),
        session: SessionConfig::default(),
        passwords: Arc::new(Passwords::new(PasswordCost::insecure_for_tests()).unwrap()),
        providers: vec![],
    })
    .expect("build");
    assert!(auth.providers().is_empty());
}
