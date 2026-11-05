//! An `[gtag]` section of a brocanth-config file builds the plugin.

#![cfg(feature = "config")]

use brocanth::plugin_app::AppBuilder;
use brocanth_config::{AppBuilderConfigExt, Config};
use brocanth_gtag::{Gtag, GtagConfig};

const TOML: &str = r#"
[gtag]
enabled = true
measurement_id = "G-ABC123"
allow_params = ["utm_source", "page"]
exclude = ["/admin", "/account"]

[gtag.banner]
text = "Nous mesurons les visites si vous acceptez."
accept = "Accepter"
refuse = "Refuser"
policy_url = "/confidentialite"
"#;

#[test]
fn the_section_reads_into_the_config_type_and_keeps_its_defaults() {
    let config = Config::from_toml("shop", "production", TOML).expect("TOML");
    let gtag: GtagConfig = config.section("gtag").expect("a section");
    assert!(gtag.enabled);
    assert_eq!(gtag.allow_params, vec!["utm_source", "page"]);
    assert!(!gtag.tracks("/account/orders"));
    assert_eq!(gtag.banner.refuse, "Refuser");
    // What the file does not set keeps its default.
    assert_eq!(gtag.consent_mode, "basic");
    assert_eq!(gtag.consent_days, 182);
    assert_eq!(gtag.banner.manage, "Cookie settings");

    AppBuilder::new()
        .with_config(config)
        .plugin_hooking_from_config::<Gtag>("gtag")
        .expect("[gtag] fits GtagConfig")
        .build()
        .expect("the section is claimed");
}
