//! The banner in the request's locale, with the `i18n` hook in front.
//!
//! The locale type here is written by hand. An application gets the same
//! trait implementation from `brocanth_i18n::build`, with one `message` arm
//! per key that takes no argument.

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_body::Body;
use brocanth::plugin_context::{After, Base};
use brocanth::plugin_hooks::{HookFamilies, Response};
use brocanth::plugin_routes::{Handler, RouteCx};
use brocanth::routing::{Router, Verb};
use brocanth_gtag::{Gtag, GtagConfig};
use brocanth_i18n::{I18nHook, Locales, NegotiatorConfig};
use brocanth_test::{TestClient, TestRequest};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Locale {
    En,
    Fr,
}

impl Locales for Locale {
    const ALL: &'static [Self] = &[Locale::En, Locale::Fr];
    const DEFAULT: Self = Locale::En;

    fn tag(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Fr => "fr",
        }
    }

    fn parent(self) -> Option<Self> {
        None
    }

    /// French has the banner. English has none of its keys, so the
    /// settings stand.
    fn message(self, key: &str) -> Option<String> {
        let text = match (self, key) {
            (Locale::Fr, "gtag.banner.text") => "Mesurer les visites ?",
            (Locale::Fr, "gtag.banner.accept") => "Accepter",
            (Locale::Fr, "gtag.banner.refuse") => "Refuser",
            (Locale::Fr, "gtag.banner.manage") => "Gérer les cookies",
            _ => return None,
        };
        Some(text.to_string())
    }
}

/// The i18n hook puts the negotiated locale on the context, so it is on
/// the provider list.
type Provided = After<I18nHook<Locale>, Base>;

const PAGE: &str = "<!DOCTYPE html><html><body><main>Hello</main></body></html>";

fn page<E: 'static>() -> Handler<E> {
    Box::new(|_cx: &mut RouteCx<'_, E>| {
        Ok::<Response, _>(
            http::Response::builder()
                .status(200)
                .header("content-type", "text/html; charset=utf-8")
                .body(Body::from(PAGE.as_bytes().to_vec()))
                .expect("a response"),
        )
    })
}

fn client() -> TestClient<Provided, impl HookFamilies, Handler<Provided>> {
    let app = AppBuilder::new()
        .plugin_hooking(I18nHook::<Locale>::new(NegotiatorConfig::default()))
        .plugin_hooking(Gtag::new(GtagConfig::default().measuring("G-ABC123")))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    table.route("/", Verb::Get, page()).expect("a route");
    TestClient::serving(app, table.build().expect("a table"))
}

#[test]
fn the_banner_speaks_the_language_the_browser_asks_for() {
    let client = client();
    let fr = client.send(TestRequest::get("/").header("accept-language", "fr-FR, en;q=0.5"));
    assert_eq!(fr.header("content-language"), Some("fr"));
    let text = fr.text();
    assert!(
        text.contains(r#"id="brocanth-consent" lang="fr""#),
        "{text}"
    );
    assert!(text.contains("Mesurer les visites ?"), "{text}");
    assert!(text.contains(">Accepter</button>"), "{text}");
    assert!(text.contains(">Refuser</button>"), "{text}");
    assert!(!text.contains(">Accept</button>"), "{text}");

    let en = client.get("/");
    let en = en.text();
    assert!(en.contains(">Accept</button>"), "{en}");
    assert!(en.contains(r#"id="brocanth-consent" lang="en""#), "{en}");
}

#[test]
fn a_locale_chosen_by_link_changes_the_banner_and_the_version_it_reports() {
    let client = client();
    let fr = client.get("/?locale=fr");
    let en = client.get("/?locale=en");
    let (fr, en) = (fr.text(), en.text());
    let version = |text: &str| {
        let at = text.find(r#""version":""#).expect("a version") + 11;
        text[at..at + 8].to_string()
    };
    assert!(fr.contains(">Accepter</button>"), "{fr}");
    assert_ne!(version(fr), version(en), "two texts, two versions");
}

#[test]
fn without_the_i18n_hook_the_banner_keeps_its_settings() {
    let app = AppBuilder::new()
        .plugin_hooking(Gtag::new(GtagConfig::default().measuring("G-ABC123")))
        .build()
        .expect("an application");
    let mut table = Router::builder();
    table.route("/", Verb::Get, page()).expect("a route");
    let client = TestClient::serving(app, table.build().expect("a table"));
    let page = client.send(TestRequest::get("/").header("accept-language", "fr"));
    let text = page.text();
    assert!(text.contains(">Accept</button>"), "{text}");
    assert!(!text.contains(" lang="), "{text}");
}
