//! The two plugins in an application: the context value a handler reads, and
//! the hook that stamps the response.

#[allow(dead_code)]
mod i18n {
    include!("golden/i18n.rs");
}

use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::{App, AppBuilder};
use brocanth::plugin_context::{After, Base, Context, Has};
use brocanth::plugin_hooks::{Both, HookSource, NoHooks, Request, RequestState, Response};
use brocanth_i18n::{
    Asked, Current, I18n, I18nHook, Locales, Negotiated, Negotiator, NegotiatorConfig, PathLocale,
    Source,
};
use i18n::{t, Locale};

/// What a generated project writes in `src/plugins/mod.rs`: the provider
/// list, spelled out.
type Installed = After<I18n<Locale>, Base>;

/// A handler as a generated project writes one. `Ctx::provided` is
/// `Context::try_get` behind the same `Has` bound, so this is the same check.
fn handler<E, I>(cx: &Context<E>, accept_language: Option<&str>, query: &str) -> String
where
    E: Has<Negotiator<Locale>, I>,
{
    let negotiator: &Negotiator<Locale> = cx.get();
    let locale = negotiator
        .negotiate(Asked {
            query,
            cookie: None,
            accept_language,
        })
        .locale;
    t::cart_items(locale, 2)
}

#[test]
fn a_handler_reads_the_negotiator_from_the_context() {
    let app: App<Installed> = AppBuilder::new()
        .plugin(I18n::<Locale>::new(NegotiatorConfig::default()))
        .build()
        .expect("an application");
    let cx = app.context();
    assert_eq!(handler(&cx, Some("fr-CH, en;q=0.5"), ""), "2 articles");
    assert_eq!(handler(&cx, Some("fr"), "locale=en"), "2 items");
    assert_eq!(handler(&cx, None, ""), "2 items");
}

#[test]
fn a_cookie_name_nothing_can_send_stops_the_application() {
    let built = AppBuilder::new()
        .plugin(I18n::<Locale>::new(NegotiatorConfig {
            query: Some("locale".into()),
            cookie: Some("my locale".into()),
            ..NegotiatorConfig::default()
        }))
        .build();
    assert!(built.is_err(), "a cookie name with a space was accepted");
}

type Hooks = Both<<I18nHook<Locale> as HookSource>::Families, NoHooks>;

fn hooked() -> App<Base, Hooks> {
    AppBuilder::new()
        .plugin_hooking(I18nHook::<Locale>::new(NegotiatorConfig::default()))
        .build()
        .expect("an application")
}

fn get(uri: &str, headers: &[(&str, &str)]) -> Request {
    let mut b = http::Request::builder().uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(Vec::new()).expect("a request")
}

/// Runs one request; the handler answers with what the hook negotiated.
fn run(app: &App<Base, Hooks>, mut request: Request) -> Response {
    let mut state = RequestState::new();
    app.run(&mut request, &mut state, |request, _state| {
        let negotiated = request
            .extensions()
            .get::<Negotiated<Locale>>()
            .copied()
            .expect("the hook ran first");
        Ok(http::Response::new(t::title(negotiated.locale).into()))
    })
}

fn body(response: Response) -> String {
    let bytes = response.into_body().into_buffered().expect("buffered");
    String::from_utf8(bytes).expect("utf-8")
}

#[test]
fn the_hook_negotiates_before_the_handler_and_says_so_on_the_response() {
    let app = hooked();
    let response = run(&app, get("/cart", &[("accept-language", "fr-CA,fr;q=0.8")]));
    assert_eq!(response.headers()["content-language"], "fr-CA");
    let vary: Vec<_> = response
        .headers()
        .get_all("vary")
        .iter()
        .map(|v| v.to_str().expect("ascii"))
        .collect();
    assert_eq!(vary, ["accept-language", "cookie"]);
    assert_eq!(body(response), "Votre panier d’achats");
}

#[test]
fn the_query_beats_the_cookie_which_beats_the_browser() {
    let app = hooked();
    let r = run(
        &app,
        get(
            "/cart?locale=fr",
            &[("cookie", "locale=en"), ("accept-language", "en")],
        ),
    );
    assert_eq!(body(r), "Votre panier");
    let r = run(
        &app,
        get(
            "/cart",
            &[("cookie", "a=1; locale=fr"), ("accept-language", "en")],
        ),
    );
    assert_eq!(body(r), "Votre panier");
}

#[test]
fn a_handlers_own_content_language_is_kept() {
    let app = hooked();
    let mut request = get("/legal", &[("accept-language", "fr")]);
    let mut state = RequestState::new();
    let response = app.run(&mut request, &mut state, |_, state| {
        let n = state.get::<Negotiated<Locale>>().expect("in the state too");
        assert_eq!((n.locale, n.source), (Locale::Fr, Source::AcceptLanguage));
        let mut r = http::Response::new("English only".into());
        r.headers_mut()
            .insert("content-language", http::HeaderValue::from_static("en"));
        r.headers_mut()
            .insert("vary", http::HeaderValue::from_static("Accept-Language"));
        Ok(r)
    });
    assert_eq!(response.headers()["content-language"], "en");
    let vary: Vec<_> = response.headers().get_all("vary").iter().collect();
    assert_eq!(vary, ["Accept-Language", "cookie"]);
}

#[test]
fn a_choice_made_by_link_is_remembered_in_the_cookie() {
    let app = hooked();
    let r = run(&app, get("/cart?locale=fr", &[("accept-language", "en")]));
    let cookie = r.headers()["set-cookie"].to_str().expect("ascii");
    assert!(cookie.starts_with("locale=fr; Path=/;"), "{cookie}");
    // A cookie or a browser setting is not a new choice: nothing to write.
    let r = run(&app, get("/cart", &[("cookie", "locale=fr")]));
    assert!(r.headers().get("set-cookie").is_none());
    let r = run(&app, get("/cart", &[("accept-language", "fr")]));
    assert!(r.headers().get("set-cookie").is_none());
}

#[test]
fn a_hook_that_cannot_name_the_locale_type_reads_current() {
    let app = hooked();
    let mut request = get("/", &[("accept-language", "fr-CA")]);
    let mut state = RequestState::new();
    app.run(&mut request, &mut state, |_, state| {
        let current = state.get::<Current>().copied().expect("the hook wrote it");
        assert_eq!(current.tag(), "fr-CA");
        assert_eq!(current.source(), Source::AcceptLanguage);
        // `fr-CA` has its own title; `nav.home` falls back to `fr`.
        assert_eq!(
            current.text("title").as_deref(),
            Some("Votre panier d’achats")
        );
        assert_eq!(current.text("nav.home").as_deref(), Some("Accueil"));
        // A key with an argument is a function only.
        assert_eq!(current.text("greeting"), None);
        assert_eq!(current.text("missing"), None);
        Ok(http::Response::new(Vec::new().into()))
    });
}

fn routed() -> App<Base, Hooks> {
    AppBuilder::new()
        .plugin_hooking(I18nHook::<Locale>::new(
            NegotiatorConfig::default().with_paths(true),
        ))
        .build()
        .expect("an application")
}

/// Runs one request; the handler answers with the path it was given and
/// the locale the hook chose.
fn run_routed(app: &App<Base, Hooks>, mut request: Request) -> Response {
    let mut state = RequestState::new();
    app.run(&mut request, &mut state, |request, state| {
        let negotiated = state.get::<Negotiated<Locale>>().copied().expect("ran");
        let href = state
            .get::<PathLocale>()
            .map_or_else(|| "-".to_string(), |p| p.href("/x"));
        let answer = format!(
            "{} {} {:?} {href}",
            request.uri(),
            negotiated.locale.tag(),
            negotiated.source
        );
        Ok(http::Response::new(answer.into()))
    })
}

#[test]
fn a_locale_segment_names_the_locale_and_comes_off_the_path() {
    let app = routed();
    let r = run_routed(&app, get("/fr/cart?page=2", &[("accept-language", "en")]));
    assert_eq!(r.headers()["content-language"], "fr");
    assert_eq!(body(r), "/cart?page=2 fr Path /fr/x");
    let r = run_routed(&app, get("/fr-CA", &[]));
    assert_eq!(body(r), "/ fr-CA Path /fr-CA/x");
}

#[test]
fn a_get_with_no_segment_is_sent_to_the_negotiated_locale() {
    let app = routed();
    let r = run_routed(&app, get("/cart?page=2", &[("accept-language", "fr")]));
    assert_eq!(r.status(), 307);
    assert_eq!(r.headers()["location"], "/fr/cart?page=2");
    let r = run_routed(&app, get("/", &[]));
    assert_eq!(r.headers()["location"], "/en");
}

#[test]
fn a_file_a_kept_prefix_and_a_post_are_not_redirected() {
    let app = routed();
    for uri in ["/robots.txt", "/admin", "/admin/users", "/api/posts"] {
        let r = run_routed(&app, get(uri, &[]));
        assert_eq!(r.status(), 200, "{uri}");
        assert!(body(r).ends_with("Default -"), "{uri}");
    }
    let mut post = get("/cart", &[("cookie", "locale=fr")]);
    *post.method_mut() = http::Method::POST;
    let r = run_routed(&app, post);
    assert_eq!(body(r), "/cart fr Cookie -");
    // `/administration` is not under `/admin`.
    assert_eq!(run_routed(&app, get("/administration", &[])).status(), 307);
}

#[test]
fn a_locale_from_the_path_is_remembered_once() {
    let app = routed();
    let r = run_routed(&app, get("/fr/cart", &[("cookie", "locale=en")]));
    let cookie = r.headers()["set-cookie"].to_str().expect("ascii");
    assert!(cookie.starts_with("locale=fr;"), "{cookie}");
    let r = run_routed(&app, get("/fr/cart", &[("cookie", "locale=fr")]));
    assert!(r.headers().get("set-cookie").is_none());
}

#[test]
fn a_kept_prefix_that_is_not_a_path_stops_the_application() {
    let config = NegotiatorConfig {
        unprefixed: vec!["admin".into()],
        ..NegotiatorConfig::default()
    };
    let why = config.check().expect_err("refused");
    assert!(why.contains("help:") && why.contains("\"/admin\""), "{why}");
}
