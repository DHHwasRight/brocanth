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
//! application that only wants the context value, and a generated project
//! cannot name the widened hook record yet (`docs/middleware.md` §5).

use std::marker::PhantomData;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, NoContext, ProvidesContext};
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookRegistrar, HookSource, OnRequest, OnResponse, Request, RequestHook,
    RequestState, ResponseHead, ResponseHook,
};
use brocanth::plugin_order::Order;
use http::header::{HeaderValue, CONTENT_LANGUAGE, COOKIE, VARY};

use crate::negotiate::{Locales, Negotiated, Negotiator, NegotiatorConfig};

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
    const NAME: &'static str = "i18n";

    fn new(config: NegotiatorConfig) -> Self {
        Self {
            negotiator: Negotiator::new(config),
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.negotiator.config().check().map_err(PluginError::new)
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
    const NAME: &'static str = "i18n-hook";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: NegotiatorConfig) -> Self {
        Self {
            negotiator: Negotiator::new(config),
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        self.negotiator.config().check().map_err(PluginError::new)
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
        registrar.response(
            "content-language",
            Order::new(),
            Stamp::<L> {
                vary_cookie: self.negotiator.config().cookie.is_some(),
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
        let negotiated = self.negotiator.negotiate_request(request);
        // Both places: the state for the response hook below, the
        // extensions for a handler that is given the request.
        request.extensions_mut().insert(negotiated);
        state.insert(negotiated);
        Ok(Flow::Continue)
    }
}

struct Stamp<L> {
    vary_cookie: bool,
    _locales: PhantomData<fn() -> L>,
}

impl<L: Locales> ResponseHook for Stamp<L> {
    fn on_headers(
        &self,
        _request: &Request,
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
