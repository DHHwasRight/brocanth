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
use brocanth_i18n::{Asked, I18n, I18nHook, Negotiated, Negotiator, NegotiatorConfig, Source};
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
