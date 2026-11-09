//! The two plugins.
//!
//! Two and not one, because they need two different registrations and an
//! application may want only the first:
//!
//! - [`I18n`] puts a [`Negotiator`] on the context. A handler reads it with
//!   `ctx.provided::<Negotiator<Locale>, _>()` and negotiates from the
//!   headers and query the context already carries. Registered with
//!   `.plugin(..)`.
//! - [`I18nHook`] negotiates once per request in a request hook, puts the
//!   [`Negotiated`] in the request's extensions and in the request state, and
//!   writes `Content-Language` and `Vary` on the way out. Registered with
//!   `.plugin_hooking(..)`, which widens the application's hook record.
//!
//! A single plugin that did both would force `.plugin_hooking` on an
//! application that only wants the context value. A generated project
//! registers [`I18nHook`]: it runs the hook pipeline, and a hook in another
//! crate (the `gtag` banner) needs the locale before the page is rendered.

use std::marker::PhantomData;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, NoContext, ProvidesContext};
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookRegistrar, HookSource, OnRequest, OnResponse, Request, RequestHook,
    RequestState, ResponseHead, ResponseHook,
};
use brocanth::plugin_order::Order;
use http::header::{HeaderValue, CONTENT_LANGUAGE, COOKIE, LOCATION, SET_COOKIE, VARY};
use http::Method;

use crate::negotiate::{
    prefixed, Current, Locales, Negotiated, Negotiator, NegotiatorConfig, Source,
};

impl<L: Locales> ContextItem for Negotiator<L> {}

impl<L: Locales> Negotiator<L> {
    /// The locale for an `http` request: its query string, every `Cookie`
    /// header and every `Accept-Language` header.
    pub fn negotiate_request<B>(&self, request: &http::Request<B>) -> Negotiated<L> {
        let headers = request.headers();
        self.negotiate_all(
            request.uri().query().unwrap_or(""),
            headers.get_all(COOKIE).iter().filter_map(text),
            headers
                .get_all(http::header::ACCEPT_LANGUAGE)
                .iter()
                .filter_map(text),
        )
    }
}

fn text(value: &HeaderValue) -> Option<&str> {
    value.to_str().ok()
}

/// Puts a [`Negotiator`] on the per-request context.
pub struct I18n<L> {
    negotiator: Negotiator<L>,
}

impl<L: Locales> Plugin for I18n<L> {
    type Config = NegotiatorConfig;
    type Provides = ProvidesContext<Negotiator<L>>;
    /// Not `i18n`: that is the hook's name, the one a generated project
    /// registers and the `[i18n]` section it reads.
    const NAME: &'static str = "i18n-context";

    fn new(config: NegotiatorConfig) -> Self {
        Self {
            negotiator: Negotiator::new(config),
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.negotiator.check().map_err(PluginError::new)
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        // An `Arc` clone: the configuration is shared, not copied.
        cx.insert(self.negotiator.clone());
    }
}

/// Negotiates in a request hook, and says which language the response is in.
pub struct I18nHook<L> {
    negotiator: Negotiator<L>,
}

impl<L: Locales> Plugin for I18nHook<L> {
    type Config = NegotiatorConfig;
    type Provides = NoContext;
    /// Also the configuration section and the flag prefix:
    /// `--i18n.default=fr`. Another crate's hook that reads [`Current`]
    /// orders itself after `i18n.negotiate`.
    const NAME: &'static str = "i18n";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: NegotiatorConfig) -> Self {
        Self {
            negotiator: Negotiator::new(config),
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.negotiator.check().map_err(PluginError::new)
    }
}

impl<L: Locales> HookSource for I18nHook<L> {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request(
            "negotiate",
            Order::new(),
            Negotiate {
                negotiator: self.negotiator.clone(),
            },
        );
        let config = self.negotiator.config();
        // Only a choice the query or the path made is written: a cookie the
        // request already carried needs no answer, and `Accept-Language` is
        // not a choice the visitor made on this site.
        let remember = match (&config.query, &config.cookie) {
            (Some(_), Some(cookie)) if config.remember => Some(cookie.clone()),
            _ => None,
        };
        registrar.response(
            "content-language",
            Order::new(),
            Stamp::<L> {
                vary_cookie: config.cookie.is_some(),
                remember,
                _locales: PhantomData,
            },
        );
        Ok(())
    }
}

struct Negotiate<L> {
    negotiator: Negotiator<L>,
}

impl<L: Locales> RequestHook for Negotiate<L> {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if self.negotiator.config().paths {
            return self.locale_from_path(request, state);
        }
        let negotiated = self.negotiator.negotiate_request(request);
        record(request, state, negotiated);
        Ok(Flow::Continue)
    }
}

impl<L: Locales> Negotiate<L> {
    /// With `paths`: the locale segment decides, and comes off the path. A
    /// GET with no segment is sent to the negotiated locale's, with a 307,
    /// because which locale it gets depends on who asks.
    fn locale_from_path(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let path = request.uri().path().to_string();
        if let Some((locale, rest)) = self.negotiator.split_path(&path) {
            let mut target = rest.to_string();
            if let Some(query) = request.uri().query() {
                target.push('?');
                target.push_str(query);
            }
            *request.uri_mut() = target.parse().map_err(|_| {
                PluginError::new(format!(
                    "i18n: `{target}`, the path without its locale, is not a URI"
                ))
            })?;
            record(
                request,
                state,
                Negotiated {
                    locale,
                    source: Source::Path,
                },
            );
            state.insert(self.negotiator.path_locale(locale));
            return Ok(Flow::Continue);
        }
        let negotiated = self.negotiator.negotiate_request(request);
        record(request, state, negotiated);
        let read = matches!(*request.method(), Method::GET | Method::HEAD);
        if !(read && self.negotiator.redirects(&path)) {
            // A form posts where it was told to: with no segment, the
            // query, the cookie or the browser chooses, as without `paths`.
            return Ok(Flow::Continue);
        }
        let mut location = prefixed(negotiated.locale.tag(), &path);
        if let Some(query) = request.uri().query() {
            location.push('?');
            location.push_str(query);
        }
        let response = http::Response::builder()
            .status(307)
            .header(LOCATION, location)
            .body(Vec::new().into())
            .map_err(|e| PluginError::new(format!("i18n: the redirect is not a response: {e}")))?;
        Ok(Flow::respond(response))
    }
}

/// Puts the locale where the pages and the other hooks read it.
///
/// Both places: the state for the response hook below and for a page
/// (`ctx.hooked`), the extensions for a handler that is given the request.
/// `Current` beside it, for a hook in a crate that cannot name `L`.
fn record<L: Locales>(request: &mut Request, state: &mut RequestState, negotiated: Negotiated<L>) {
    request.extensions_mut().insert(negotiated);
    state.insert(negotiated);
    state.insert(Current::of(negotiated));
}

struct Stamp<L> {
    vary_cookie: bool,
    /// The cookie a `?locale=` choice is written to, when `remember` is on.
    remember: Option<String>,
    _locales: PhantomData<fn() -> L>,
}

impl<L: Locales> ResponseHook for Stamp<L> {
    fn on_headers(
        &self,
        request: &Request,
        head: &mut ResponseHead,
        _body: BodyShape<'_>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let Some(negotiated) = state.get::<Negotiated<L>>() else {
            return Ok(());
        };
        // A handler that set its own language knows better: an English
        // legal notice served on a French page stays English.
        if !head.headers.contains_key(CONTENT_LANGUAGE) {
            let tag = HeaderValue::from_str(negotiated.locale.tag()).map_err(|_| {
                PluginError::new(format!(
                    "i18n: the locale tag `{}` is not a header value",
                    negotiated.locale.tag()
                ))
            })?;
            head.headers.insert(CONTENT_LANGUAGE, tag);
        }
        // The answer depends on these headers, so a shared cache must key on
        // them. Appended, never inserted, so a handler's own `Vary` stays.
        // The query string needs no `Vary`: it is part of the URL already.
        vary(head, "accept-language");
        if self.vary_cookie {
            vary(head, "cookie");
        }
        let chosen = match negotiated.source {
            Source::Query => true,
            // A path names the locale on every page, so write the cookie
            // only when it says something else: a visitor who follows a
            // link to `/fr/` is sent there again from `/`.
            Source::Path => self.remember.as_deref().is_some_and(|name| {
                !request
                    .headers()
                    .get_all(COOKIE)
                    .iter()
                    .filter_map(text)
                    .flat_map(|header| header.split(';'))
                    .filter_map(|pair| pair.trim().split_once('='))
                    .any(|(key, value)| {
                        key.trim() == name && value.trim() == negotiated.locale.tag()
                    })
            }),
            _ => false,
        };
        if let (Some(name), true) = (&self.remember, chosen) {
            // A year, the whole site, and no script access: a language
            // switcher is a link, and the server reads the answer.
            let cookie = format!(
                "{name}={}; Path=/; Max-Age=31536000; SameSite=Lax; HttpOnly",
                negotiated.locale.tag()
            );
            if let Ok(value) = HeaderValue::from_str(&cookie) {
                head.headers.append(SET_COOKIE, value);
            }
        }
        Ok(())
    }
}

fn vary(head: &mut ResponseHead, name: &'static str) {
    let already = head
        .headers
        .get_all(VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|token| {
            let token = token.trim();
            token == "*" || token.eq_ignore_ascii_case(name)
        });
    if !already {
        head.headers.append(VARY, HeaderValue::from_static(name));
    }
}
