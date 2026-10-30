//! Both day-2 hooks, built from a brocanth-config document.
//!
//! The instance name of each plugin is its section, so registering it
//! claims the section and the unclaimed-section check passes.

#![cfg(feature = "config")]

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_config::{AppBuilderConfigExt, Config};
use brocanth_middleware::rate_limit::Key;
use brocanth_middleware::{
    Frames, Hsts, RateLimit, RateLimitConfig, Rule, SecurityHeaders, SecurityHeadersConfig,
    ONE_YEAR,
};

const TOML: &str = r#"
[security_headers]
behind_tls = true
frames = "same_origin"

[security_headers.hsts]
max_age = 63072000
include_subdomains = true
preload = true

[security_headers.csp]
report_only = true

[security_headers.csp.directives]
img-src = ["'self'", "https://images.example.com"]

[rate_limit]
ip_header = "fly-client-ip"
session_cookie = "_shop_session"

[[rate_limit.rules]]
name = "login"
paths = ["/login"]
methods = ["POST"]
limit = 5
window_secs = 60

[[rate_limit.rules]]
name = "api"
limit = 600
window_secs = 60
key = ["user", "session", "ip"]
"#;

#[test]
fn both_sections_read_into_their_types() {
    let config = Config::from_toml("shop", "production", TOML).expect("TOML");

    let headers: SecurityHeadersConfig = config.section("security_headers").expect("a section");
    assert!(headers.behind_tls);
    assert_eq!(headers.frames, Frames::SameOrigin);
    assert_eq!(
        headers.hsts,
        Hsts::max_age(ONE_YEAR * 2).include_subdomains().preload()
    );
    assert!(headers.csp.report_only);
    // A key the file does not set keeps its default.
    assert!(headers.nosniff);
    assert!(headers.csp.dp_scripts);
    headers.check().expect("a valid policy");

    let limits: RateLimitConfig = config.section("rate_limit").expect("a section");
    assert_eq!(limits.rules.len(), 2);
    assert_eq!(
        limits.rules[0],
        Rule::new("login", 5, 60)
            .paths(["/login"])
            .methods(["POST"])
    );
    assert_eq!(limits.rules[1].paths, vec!["/".to_string()]);
    assert_eq!(limits.rules[1].key, vec![Key::User, Key::Session, Key::Ip]);
    assert!(limits.fail_open);
    limits.check().expect("valid rules");
}

#[test]
fn registering_both_claims_both_sections() {
    let config = Config::from_toml("shop", "production", TOML).expect("TOML");
    let headers: SecurityHeadersConfig = config.section("security_headers").unwrap();
    let limits: RateLimitConfig = config.section("rate_limit").unwrap();
    AppBuilder::new()
        .with_config(config)
        .plugin_hooking(SecurityHeaders::new(headers))
        .plugin_hooking(RateLimit::new(limits))
        .build()
        .expect("both sections are claimed");
}

#[test]
fn a_misspelt_key_is_an_error() {
    let config = Config::from_toml(
        "shop",
        "production",
        "[security_headers]\nbehind_tsl = true\n",
    )
    .expect("TOML");
    assert!(config
        .section::<SecurityHeadersConfig>("security_headers")
        .is_err());
}
