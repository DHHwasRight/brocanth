//! The route cache: whole GET responses, served from the cache without
//! running the handler.
//!
//! A plugin with two hooks. The request hook looks the response up and
//! answers with it on a hit. The response hook stores what the handler
//! produced, when it is safe to store.
//!
//! ```no_run
//! # use std::sync::Arc;
//! # use std::time::Duration;
//! # use brocanth_cache::{PgCache, Tag};
//! use brocanth::plugin_api::Plugin;
//! use brocanth::plugin_app::AppBuilder;
//! use brocanth_cache::route::{RouteCache, RouteCacheConfig, RouteRule};
//!
//! # async fn example(cache: PgCache) -> Result<(), Box<dyn std::error::Error>> {
//! let routes = RouteCacheConfig::new(Arc::new(cache))
//!     .rule(RouteRule::path("/", Duration::from_secs(60)))
//!     .rule(
//!         RouteRule::prefix("/posts/", Duration::from_secs(300))
//!             .tags([Tag::table("posts")?])
//!             .vary([http::header::ACCEPT_LANGUAGE]),
//!     );
//! let app = AppBuilder::new()
//!     .plugin_hooking(RouteCache::new(routes))
//!     .build()?;
//! # let _ = app;
//! # Ok(())
//! # }
//! ```
//!
//! # What is never stored
//!
//! The rules are deny-first. A response is stored only when **all** of these
//! hold:
//!
//! * the request is a `GET` and a [`RouteRule`] matches its path;
//! * the request carries no `Authorization` header and no session cookie,
//!   unless the rule says [`RouteRule::allow_authenticated`];
//! * the status is `200`;
//! * the body is buffered. A stream is never stored: there is no body in hand
//!   to store;
//! * the response sets no cookie;
//! * the response's `Cache-Control` does not say `no-store` or `private`;
//! * the response's `Vary` names only headers that are in the key: the host
//!   and the rule's [`vary`](RouteRule::vary) headers. `Vary: *` and, for
//!   example, `Vary: Cookie` (the CSRF middleware sends it) are never stored:
//!   the key cannot tell those requests apart;
//! * the stored form fits under [`Cache::max_value_bytes`].
//!
//! A handler opts one response out with `Cache-Control: private`, and adds
//! tags at run time with the [`TAGS_HEADER`] header. The hook removes that
//! header before the response leaves.
//!
//! # The key
//!
//! The method, the path, the query string (unless
//! [`RouteRule::ignore_query`], or only some parameters with
//! [`RouteRule::query_params`]), the host the request names, and the value of
//! each header the rule [varies](RouteRule::vary) on. Every part is escaped,
//! so a header value cannot impersonate another part. A key over
//! [`MAX_KEY_BYTES`] is not cached.
//!
//! The host is in the key, so an application that serves several hosts
//! (tenant subdomains, one domain per locale) never serves one host's page
//! on another. Put the proxy-headers middleware before this hook when a
//! proxy rewrites `Host`.
//!
//! Every distinct query string is a distinct entry, so a loop over
//! `/?x=<random>` writes one entry per request until the TTLs run out. A
//! rule whose page reads one or two parameters names them with
//! [`RouteRule::query_params`], and every other parameter then shares one
//! entry. The default keeps the whole query string, because a page that
//! paginates with `?page=2` must not serve page 1 from the cache.
//!
//! # Order
//!
//! Response hooks run in reverse declared order. Declare this plugin so its
//! response hook runs **before** any hook that rewrites the body
//! (compression, `ETag`). On a hit those hooks run again over the stored
//! body, so the store must hold the handler's own bytes.
//!
//! # What it does about failure
//!
//! A cache error on lookup is a miss that is not stored: the handler serves
//! the page and the hook writes one warning to standard error. A route cache
//! is an optimisation, and a Redis outage must not turn every cached page
//! into a 500. ([`crate::fetch`] fails instead, because its caller can decide
//! what to do; a page cannot.)
//!
//! The store runs after the response is final, on a spawned task, and a
//! failed store is a later miss. It needs a Tokio runtime; without one the
//! hook stores nothing. At most [`DEFAULT_MAX_PENDING_STORES`] stores run at
//! once (see [`RouteCacheConfig::max_pending_stores`]); a response that finds
//! them all busy is not stored.
//!
//! # What it does not do
//!
//! No single flight: a thousand concurrent misses on one path run the handler
//! a thousand times. A page that cannot survive that wants
//! [`CacheExt::fetch`](crate::CacheExt::fetch) inside its handler.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    AsyncRequestHook, BodyShape, Both, Flow, HookRegistrar, HookSource, OnAsyncRequest, OnResponse,
    Request, RequestState, Response, ResponseHead, ResponseHook,
};
use brocanth::plugin_order::Order;
use brocanth::plugin_routes::BoxFuture;
use http::header::{HeaderMap, HeaderName, HeaderValue};

use crate::cache::Cache;
use crate::key::{Key, Namespace, MAX_KEY_BYTES};
use crate::tag::Tag;

/// The response header a handler sets to add tags to a cached response:
/// `brocanth-cache-tags: posts, posts:42`. Comma-separated. The hook removes
/// it from every response, cached or not.
pub const TAGS_HEADER: &str = "brocanth-cache-tags";

/// The response header that says what the route cache did: `hit` or `miss`.
pub const STATUS_HEADER: &str = "brocanth-cache";

/// The default namespace for stored responses.
pub const DEFAULT_NAMESPACE: &str = "routes";

/// How many stores may run at once by default. A store holds a copy of the
/// response, so this bounds the memory a burst of misses can pin.
pub const DEFAULT_MAX_PENDING_STORES: usize = 64;

/// The tag every stored response carries: `route:{path}`.
///
/// Invalidate it to drop every cached variant of one path, whatever its query
/// string and vary headers.
///
/// # Errors
/// [`CacheError::InvalidTag`](crate::CacheError::InvalidTag) for a path over
/// [`MAX_TAG_BYTES`](crate::MAX_TAG_BYTES).
pub fn path_tag(path: &str) -> crate::Result<Tag> {
    Tag::new(format!("route:{path}"))
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum PathMatch {
    Exact(String),
    Prefix(String),
}

impl PathMatch {
    fn matches(&self, path: &str) -> bool {
        match self {
            Self::Exact(p) => path == p,
            Self::Prefix(p) => path.starts_with(p.as_str()),
        }
    }
}

/// Which paths are cached, for how long, and under which tags.
#[derive(Clone, Debug)]
pub struct RouteRule {
    path: PathMatch,
    ttl: Duration,
    tags: Vec<Tag>,
    vary: Vec<HeaderName>,
    authenticated: bool,
    query: Query,
}

/// What of the query string goes into the key.
#[derive(Clone, Debug)]
enum Query {
    All,
    None,
    Only(Vec<String>),
}

impl RouteRule {
    fn with(path: PathMatch, ttl: Duration) -> Self {
        Self {
            path,
            ttl,
            tags: Vec::new(),
            vary: Vec::new(),
            authenticated: false,
            query: Query::All,
        }
    }

    /// One path, exactly.
    pub fn path(path: impl Into<String>, ttl: Duration) -> Self {
        Self::with(PathMatch::Exact(path.into()), ttl)
    }

    /// Every path that starts with `prefix`.
    pub fn prefix(prefix: impl Into<String>, ttl: Duration) -> Self {
        Self::with(PathMatch::Prefix(prefix.into()), ttl)
    }

    /// Stores every response under this rule with these tags.
    #[must_use]
    pub fn tags(mut self, tags: impl IntoIterator<Item = Tag>) -> Self {
        self.tags.extend(tags);
        self
    }

    /// Keys the response on the values of these request headers as well.
    ///
    /// `Accept-Language` for a translated page, `Accept-Encoding` when a
    /// hook before this one compresses.
    #[must_use]
    pub fn vary(mut self, headers: impl IntoIterator<Item = HeaderName>) -> Self {
        self.vary.extend(headers);
        self
    }

    /// Caches the response for a request with credentials too.
    ///
    /// Off by default. Turn it on only for a route whose response does not
    /// depend on who asks: every user is then served the first user's page.
    #[must_use]
    pub fn allow_authenticated(mut self) -> Self {
        self.authenticated = true;
        self
    }

    /// Leaves the query string out of the key, so `/?utm_source=x` and `/`
    /// share one entry.
    #[must_use]
    pub fn ignore_query(mut self) -> Self {
        self.query = Query::None;
        self
    }

    /// Keys on these query parameters only, in the order the request gives
    /// them. Every other parameter is left out, so `/?page=2&utm_source=x`
    /// and `/?page=2` share one entry, and a random parameter cannot make a
    /// new one.
    #[must_use]
    pub fn query_params(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.query = Query::Only(names.into_iter().map(Into::into).collect());
        self
    }
}

/// The route cache's configuration: the cache, the rules, and the order of
/// the two hooks.
pub struct RouteCacheConfig {
    cache: Arc<dyn Cache>,
    namespace: Namespace,
    rules: Vec<RouteRule>,
    session_cookies: Option<Vec<String>>,
    max_pending_stores: usize,
    request_order: Order,
    response_order: Order,
}

impl RouteCacheConfig {
    /// No rules, the [`DEFAULT_NAMESPACE`], and any cookie counted as a
    /// session.
    pub fn new(cache: Arc<dyn Cache>) -> Self {
        Self {
            cache,
            namespace: Namespace::new(DEFAULT_NAMESPACE).expect("a valid namespace literal"),
            rules: Vec::new(),
            session_cookies: None,
            max_pending_stores: DEFAULT_MAX_PENDING_STORES,
            request_order: Order::new(),
            response_order: Order::new(),
        }
    }

    /// Stores responses in this namespace instead.
    #[must_use]
    pub fn namespace(mut self, namespace: Namespace) -> Self {
        self.namespace = namespace;
        self
    }

    /// Adds a rule. The first rule that matches a path wins.
    #[must_use]
    pub fn rule(mut self, rule: RouteRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Counts only these cookies as credentials.
    ///
    /// By default any `Cookie` header makes a request authenticated, which
    /// also stops caching for a visitor who carries an analytics cookie.
    /// Naming the session cookie lets those visitors share the cache.
    #[must_use]
    pub fn session_cookies(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.session_cookies = Some(names.into_iter().map(Into::into).collect());
        self
    }

    /// How many stores may run at once. A response that finds them all busy
    /// is served and not stored. [`DEFAULT_MAX_PENDING_STORES`] by default.
    #[must_use]
    pub fn max_pending_stores(mut self, max: usize) -> Self {
        self.max_pending_stores = max;
        self
    }

    /// Where the lookup hook sits in the request chain.
    #[must_use]
    pub fn request_order(mut self, order: Order) -> Self {
        self.request_order = order;
        self
    }

    /// Where the store hook sits in the response chain. See the module
    /// documentation on order.
    #[must_use]
    pub fn response_order(mut self, order: Order) -> Self {
        self.response_order = order;
        self
    }
}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// The route cache as a plugin. Register it with
/// `AppBuilder::plugin_hooking`.
pub struct RouteCache {
    inner: Arc<Inner>,
    request_order: Order,
    response_order: Order,
}

struct Inner {
    cache: Arc<dyn Cache>,
    namespace: Namespace,
    rules: Vec<RouteRule>,
    session_cookies: Option<Vec<String>>,
    max_pending_stores: usize,
    pending_stores: Arc<AtomicUsize>,
}

impl Plugin for RouteCache {
    type Config = RouteCacheConfig;
    type Provides = NoContext;
    const NAME: &'static str = "route_cache";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: RouteCacheConfig) -> Self {
        Self {
            inner: Arc::new(Inner {
                cache: config.cache,
                namespace: config.namespace,
                rules: config.rules,
                session_cookies: config.session_cookies,
                max_pending_stores: config.max_pending_stores,
                pending_stores: Arc::new(AtomicUsize::new(0)),
            }),
            request_order: config.request_order,
            response_order: config.response_order,
        }
    }
}

impl HookSource for RouteCache {
    type Families = Both<OnAsyncRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request_async(
            "lookup",
            self.request_order.clone(),
            Lookup(self.inner.clone()),
        );
        registrar.response(
            "store",
            self.response_order.clone(),
            Store(self.inner.clone()),
        );
        Ok(())
    }
}

/// What the lookup leaves for the store, on a miss that may be stored.
struct Pending {
    key: Key,
    ttl: Duration,
    tags: Vec<Tag>,
    vary: Vec<HeaderName>,
}

/// One running store. Gives its place back when the store ends.
struct StoreSlot(Arc<AtomicUsize>);

impl StoreSlot {
    /// A place among `max` running stores, or `None` when all are taken.
    fn take(count: &Arc<AtomicUsize>, max: usize) -> Option<Self> {
        count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < max).then_some(n + 1)
            })
            .ok()
            .map(|_| Self(count.clone()))
    }
}

impl Drop for StoreSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Inner {
    fn rule_for(&self, path: &str) -> Option<&RouteRule> {
        self.rules.iter().find(|rule| rule.path.matches(path))
    }

    /// Whether the request carries credentials.
    fn is_authenticated(&self, headers: &HeaderMap) -> bool {
        if headers.contains_key(http::header::AUTHORIZATION) {
            return true;
        }
        let cookies = headers.get_all(http::header::COOKIE);
        match &self.session_cookies {
            None => cookies
                .iter()
                .any(|v| !v.as_bytes().trim_ascii().is_empty()),
            Some(names) => cookies.iter().any(|value| {
                value
                    .as_bytes()
                    .split(|b| *b == b';')
                    .filter_map(|pair| pair.split(|b| *b == b'=').next())
                    .any(|name| names.iter().any(|n| n.as_bytes() == name.trim_ascii()))
            }),
        }
    }
}

/// The key text for a request under a rule, before it becomes a [`Key`].
fn key_text(rule: &RouteRule, request: &Request) -> String {
    let mut out = String::from("GET ");
    escape_into(&mut out, request.uri().path().as_bytes());
    if let Some(query) = request.uri().query() {
        match &rule.query {
            Query::All => {
                out.push('?');
                escape_into(&mut out, query.as_bytes());
            }
            Query::None => {}
            Query::Only(names) => {
                let kept: Vec<&str> = query
                    .split('&')
                    .filter(|pair| {
                        let name = pair.split('=').next().unwrap_or("");
                        names.iter().any(|n| n == name)
                    })
                    .collect();
                if !kept.is_empty() {
                    out.push('?');
                    escape_into(&mut out, kept.join("&").as_bytes());
                }
            }
        }
    }
    // The host, always: one host's page must not answer for another.
    out.push_str(" | host=");
    if let Some(host) = request_host(request) {
        escape_into(&mut out, host.to_ascii_lowercase().as_bytes());
    }
    for name in &rule.vary {
        out.push_str(" | ");
        out.push_str(name.as_str());
        out.push('=');
        for (i, value) in request.headers().get_all(name).iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            escape_into(&mut out, value.as_bytes());
        }
    }
    out
}

/// The host the request names: the URI's authority (HTTP/2), else `Host`.
fn request_host(request: &Request) -> Option<&str> {
    request
        .uri()
        .authority()
        .map(http::uri::Authority::as_str)
        .or_else(|| {
            request
                .headers()
                .get(http::header::HOST)
                .and_then(|v| v.to_str().ok())
        })
}

/// Appends `bytes`, with `%`, `|` and every byte that is not printable ASCII
/// written as `%XX`. So the separators in [`key_text`] cannot appear inside a
/// part, and two different requests cannot spell one key.
fn escape_into(out: &mut String, bytes: &[u8]) {
    for &b in bytes {
        if b == b'%' || b == b'|' || !(0x20..0x7f).contains(&b) {
            out.push_str(&format!("%{b:02X}"));
        } else {
            out.push(char::from(b));
        }
    }
}

// ---------------------------------------------------------------------------
// The lookup
// ---------------------------------------------------------------------------

struct Lookup(Arc<Inner>);

impl AsyncRequestHook for Lookup {
    fn before_dispatch<'a>(
        &'a self,
        request: &'a mut Request,
        state: &'a mut RequestState,
    ) -> BoxFuture<'a, Result<Flow, PluginError>> {
        Box::pin(async move {
            let inner = &self.0;
            if request.method() != http::Method::GET {
                return Ok(Flow::Continue);
            }
            let Some(rule) = inner.rule_for(request.uri().path()) else {
                return Ok(Flow::Continue);
            };
            if !rule.authenticated && inner.is_authenticated(request.headers()) {
                return Ok(Flow::Continue);
            }
            let text = key_text(rule, request);
            if text.len() > MAX_KEY_BYTES {
                return Ok(Flow::Continue);
            }
            let key = inner
                .namespace
                .key(text)
                .map_err(|e| {
                    PluginError::new(format!(
                        "route cache: {e}\n\
                         help: shorten the key the rule builds from the request, or leave the route out of the rule"
                    ))
                })?;

            let stored = match inner.cache.get(&key).await {
                Ok(stored) => stored,
                Err(e) => {
                    // A miss that is not stored: the handler serves the page.
                    eprintln!(
                        "warning: route cache: {e}; serving {} without the cache\n\
                         help: check that the cache backend answers; its readiness check shows it",
                        request.uri().path()
                    );
                    return Ok(Flow::Continue);
                }
            };
            if let Some(response) = stored.as_deref().and_then(decode) {
                return Ok(Flow::respond(response));
            }

            let mut tags = rule.tags.clone();
            if let Ok(tag) = path_tag(request.uri().path()) {
                tags.push(tag);
            }
            state.insert(Pending {
                key,
                ttl: rule.ttl,
                tags,
                vary: rule.vary.clone(),
            });
            Ok(Flow::Continue)
        })
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

struct Store(Arc<Inner>);

/// Whether a response may be stored, judged on its head alone. `vary` is
/// the rule's: the request headers the key holds besides the host.
fn storable(head: &ResponseHead, vary: &[HeaderName]) -> bool {
    if head.status != http::StatusCode::OK {
        return false;
    }
    if head.headers.contains_key(http::header::SET_COOKIE) {
        return false;
    }
    if !vary_is_in_the_key(&head.headers, vary) {
        return false;
    }
    !head
        .headers
        .get_all(http::header::CACHE_CONTROL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|directive| directive.split('=').next().unwrap_or("").trim())
        .any(|d| d.eq_ignore_ascii_case("no-store") || d.eq_ignore_ascii_case("private"))
}

/// Whether every header the response's `Vary` names is in the key. A
/// response that varies on anything else (`*`, `Cookie`) differs between
/// requests that share one key, so it cannot be stored.
fn vary_is_in_the_key(headers: &HeaderMap, vary: &[HeaderName]) -> bool {
    headers.get_all(http::header::VARY).iter().all(|value| {
        let Ok(text) = value.to_str() else {
            return false;
        };
        text.split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .all(|name| match HeaderName::from_bytes(name.as_bytes()) {
                Ok(name) => name == http::header::HOST || vary.contains(&name),
                Err(_) => false,
            })
    })
}

/// Removes [`TAGS_HEADER`] and answers the tags it named.
fn take_header_tags(headers: &mut HeaderMap) -> Vec<Tag> {
    let name = HeaderName::from_static(TAGS_HEADER);
    let mut tags = Vec::new();
    // `remove` would drop every value after the first.
    if let http::header::Entry::Occupied(entry) = headers.entry(&name) {
        for value in entry.remove_entry_mult().1 {
            let Ok(text) = value.to_str() else { continue };
            tags.extend(
                text.split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .filter_map(|t| Tag::new(t).ok()),
            );
        }
    }
    tags
}

impl ResponseHook for Store {
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        body: BodyShape<'_>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let header_tags = take_header_tags(&mut head.headers);
        if state.get::<Pending>().is_none() {
            // A hit, an uncached route, or a request this cache refused.
            return Ok(());
        }
        // A stream is never stored, and neither is anything the head rules
        // out. `on_body` finds nothing pending and does nothing.
        let head_ok = state
            .get::<Pending>()
            .is_some_and(|pending| storable(head, &pending.vary));
        if body.is_streaming() || !head_ok {
            state.take::<Pending>();
            return Ok(());
        }
        if let Some(pending) = state.get_mut::<Pending>() {
            pending.tags.extend(header_tags);
        }
        head.headers
            .insert(STATUS_HEADER, HeaderValue::from_static("miss"));
        Ok(())
    }

    fn on_body(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        body: &mut Vec<u8>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let Some(pending) = state.take::<Pending>() else {
            return Ok(());
        };
        let value = encode(&head.headers, body);
        if value.len() > self.0.cache.max_value_bytes() {
            return Ok(());
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return Ok(());
        };
        // Every place taken: this response is not stored, rather than one
        // more task holding one more copy of a response.
        let Some(slot) = StoreSlot::take(&self.0.pending_stores, self.0.max_pending_stores) else {
            return Ok(());
        };
        let cache = self.0.cache.clone();
        runtime.spawn(async move {
            let _slot = slot;
            // Best effort: the response has already gone, and a failed store
            // is a later miss.
            let _ = cache
                .set_tagged(&pending.key, &value, pending.ttl, &pending.tags)
                .await;
        });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The stored form
// ---------------------------------------------------------------------------

const MAGIC: &[u8; 4] = b"BRC1";

/// Headers that describe one transfer, not the response. The transport sets
/// the framing ones again.
fn is_per_transfer(name: &HeaderName) -> bool {
    name == http::header::CONTENT_LENGTH
        || name == http::header::TRANSFER_ENCODING
        || name == http::header::CONNECTION
        || name == http::header::DATE
        || name.as_str() == STATUS_HEADER
}

/// `BRC1`, a header count, each header as a length-prefixed name and value,
/// then the body. The status is always 200, so it is not stored.
fn encode(headers: &HeaderMap, body: &[u8]) -> Vec<u8> {
    let kept: Vec<(&HeaderName, &HeaderValue)> = headers
        .iter()
        .filter(|(n, _)| !is_per_transfer(n))
        .collect();
    let mut out = Vec::with_capacity(body.len() + 64 * kept.len() + 8);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(kept.len() as u32).to_be_bytes());
    for (name, value) in kept {
        out.extend_from_slice(&(name.as_str().len() as u32).to_be_bytes());
        out.extend_from_slice(name.as_str().as_bytes());
        out.extend_from_slice(&(value.as_bytes().len() as u32).to_be_bytes());
        out.extend_from_slice(value.as_bytes());
    }
    out.extend_from_slice(body);
    out
}

/// The response [`encode`] wrote, or `None` for bytes it did not write. A
/// value this cannot read is a miss, and the next store overwrites it.
fn decode(bytes: &[u8]) -> Option<Response> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (count, mut rest) = take_u32(rest)?;
    let mut headers = HeaderMap::new();
    for _ in 0..count {
        let (len, r) = take_u32(rest)?;
        let (name, r) = r.split_at_checked(len as usize)?;
        let (len, r) = take_u32(r)?;
        let (value, r) = r.split_at_checked(len as usize)?;
        headers.append(
            HeaderName::from_bytes(name).ok()?,
            HeaderValue::from_bytes(value).ok()?,
        );
        rest = r;
    }
    let mut response = Response::new(Body::from(rest.to_vec()));
    *response.headers_mut() = headers;
    response
        .headers_mut()
        .insert(STATUS_HEADER, HeaderValue::from_static("hit"));
    Some(response)
}

fn take_u32(bytes: &[u8]) -> Option<(u32, &[u8])> {
    let (head, rest) = bytes.split_at_checked(4)?;
    Some((u32::from_be_bytes(head.try_into().ok()?), rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(uri: &str, headers: &[(&str, &str)]) -> Request {
        let mut builder = http::Request::builder().uri(uri);
        for (n, v) in headers {
            builder = builder.header(*n, *v);
        }
        builder.body(Vec::new()).unwrap()
    }

    fn head(status: u16, headers: &[(&str, &str)]) -> ResponseHead {
        let mut builder = http::Response::builder().status(status);
        for (n, v) in headers {
            builder = builder.header(*n, *v);
        }
        builder.body(()).unwrap().into_parts().0
    }

    fn inner(session_cookies: Option<Vec<String>>) -> Inner {
        struct Never;
        #[async_trait::async_trait]
        impl Cache for Never {
            fn max_value_bytes(&self) -> usize {
                0
            }
            async fn migrate(&self) -> crate::Result<()> {
                unreachable!()
            }
            async fn get(&self, _: &Key) -> crate::Result<Option<Vec<u8>>> {
                unreachable!()
            }
            async fn set(&self, _: &Key, _: &[u8], _: Duration) -> crate::Result<()> {
                unreachable!()
            }
            async fn set_tagged(
                &self,
                _: &Key,
                _: &[u8],
                _: Duration,
                _: &[Tag],
            ) -> crate::Result<()> {
                unreachable!()
            }
            async fn invalidate_tags(&self, _: &[Tag]) -> crate::Result<u64> {
                unreachable!()
            }
            async fn add(&self, _: &Key, _: &[u8], _: Duration) -> crate::Result<bool> {
                unreachable!()
            }
            async fn delete(&self, _: &Key) -> crate::Result<bool> {
                unreachable!()
            }
            async fn clear(&self, _: &Namespace) -> crate::Result<u64> {
                unreachable!()
            }
            async fn lease(&self, _: &Key, _: Duration) -> crate::Result<bool> {
                unreachable!()
            }
            async fn release_lease(&self, _: &Key) -> crate::Result<()> {
                unreachable!()
            }
            async fn purge_expired(&self, _: u64) -> crate::Result<u64> {
                unreachable!()
            }
        }
        Inner {
            cache: Arc::new(Never),
            namespace: Namespace::new("routes").unwrap(),
            rules: vec![
                RouteRule::path("/", Duration::from_secs(1)),
                RouteRule::prefix("/posts/", Duration::from_secs(1)),
            ],
            session_cookies,
            max_pending_stores: 2,
            pending_stores: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[test]
    fn the_first_matching_rule_wins_and_a_path_outside_every_rule_has_none() {
        let inner = inner(None);
        assert!(matches!(
            inner.rule_for("/").map(|r| &r.path),
            Some(PathMatch::Exact(_))
        ));
        assert!(matches!(
            inner.rule_for("/posts/1").map(|r| &r.path),
            Some(PathMatch::Prefix(_))
        ));
        assert!(inner.rule_for("/about").is_none());
    }

    #[test]
    fn a_header_value_cannot_impersonate_another_part_of_the_key() {
        let rule = RouteRule::path("/", Duration::from_secs(1))
            .vary([HeaderName::from_static("a"), HeaderName::from_static("b")]);
        let one = key_text(&rule, &request("/", &[("a", "x | b=y"), ("b", "")]));
        let two = key_text(&rule, &request("/", &[("a", "x"), ("b", "y | b=")]));
        assert_ne!(one, two);
        assert!(one.contains("%7C"), "{one}");
    }

    #[test]
    fn the_query_is_in_the_key_unless_the_rule_ignores_it() {
        let keep = RouteRule::path("/", Duration::from_secs(1));
        let drop = keep.clone().ignore_query();
        let r = request("/?page=2", &[]);
        assert_eq!(key_text(&keep, &r), "GET /?page=2 | host=");
        assert_eq!(key_text(&drop, &r), "GET / | host=");
    }

    #[test]
    fn only_the_named_query_params_are_in_the_key() {
        let rule = RouteRule::path("/", Duration::from_secs(1)).query_params(["page"]);
        assert_eq!(
            key_text(&rule, &request("/?utm=1&page=2&x=9", &[])),
            "GET /?page=2 | host="
        );
        assert_eq!(key_text(&rule, &request("/?x=9", &[])), "GET / | host=");
    }

    #[test]
    fn the_host_is_in_the_key() {
        let rule = RouteRule::path("/", Duration::from_secs(1));
        let a = key_text(&rule, &request("/", &[("host", "a.example")]));
        let b = key_text(&rule, &request("/", &[("host", "B.example")]));
        assert_eq!(a, "GET / | host=a.example");
        assert_eq!(b, "GET / | host=b.example");
        // A host cannot borrow part of the path: the separator is escaped.
        let c = key_text(&rule, &request("/x", &[("host", "a.example | host=")]));
        assert!(c.contains("%7C"), "{c}");
    }

    #[test]
    fn stores_beyond_the_bound_are_refused_until_one_ends() {
        let count = Arc::new(AtomicUsize::new(0));
        let one = StoreSlot::take(&count, 2).expect("a first place");
        let _two = StoreSlot::take(&count, 2).expect("a second place");
        assert!(StoreSlot::take(&count, 2).is_none());
        drop(one);
        assert!(StoreSlot::take(&count, 2).is_some());
    }

    #[test]
    fn authorization_or_any_cookie_counts_as_credentials_by_default() {
        let inner = inner(None);
        assert!(inner.is_authenticated(request("/", &[("authorization", "Bearer x")]).headers()));
        assert!(inner.is_authenticated(request("/", &[("cookie", "_ga=1")]).headers()));
        assert!(!inner.is_authenticated(request("/", &[]).headers()));
    }

    #[test]
    fn named_session_cookies_let_other_cookies_share_the_cache() {
        let inner = inner(Some(vec!["sid".into()]));
        assert!(!inner.is_authenticated(request("/", &[("cookie", "_ga=1; theme=dark")]).headers()));
        assert!(inner.is_authenticated(request("/", &[("cookie", "_ga=1; sid=abc")]).headers()));
    }

    #[test]
    fn only_a_plain_200_without_cookies_or_private_directives_is_storable() {
        assert!(storable(&head(200, &[("content-type", "text/html")]), &[]));
        assert!(storable(
            &head(200, &[("cache-control", "public, max-age=60")]),
            &[]
        ));
        assert!(!storable(&head(404, &[]), &[]));
        assert!(!storable(&head(302, &[]), &[]));
        assert!(!storable(&head(200, &[("set-cookie", "a=b")]), &[]));
        assert!(!storable(&head(200, &[("cache-control", "no-store")]), &[]));
        assert!(!storable(
            &head(200, &[("cache-control", "max-age=0, Private")]),
            &[]
        ));
    }

    #[test]
    fn a_response_that_varies_on_a_header_outside_the_key_is_not_storable() {
        let lang = [http::header::ACCEPT_LANGUAGE];
        let varies = head(200, &[("vary", "Accept-Language")]);
        assert!(!storable(&varies, &[]), "the rule has no vary");
        assert!(storable(&varies, &lang), "the rule varies on it");
        assert!(!storable(&head(200, &[("vary", "Cookie")]), &lang));
        assert!(!storable(&head(200, &[("vary", "*")]), &lang));
        assert!(!storable(
            &head(200, &[("vary", "accept-language, cookie")]),
            &lang
        ));
        assert!(storable(&head(200, &[("vary", "Host")]), &[]));
    }

    #[test]
    fn a_stored_response_comes_back_with_its_headers_and_body_and_says_hit() {
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("text/html"));
        headers.append("link", HeaderValue::from_static("</a.css>"));
        headers.append("link", HeaderValue::from_static("</b.css>"));
        headers.insert("content-length", HeaderValue::from_static("5"));
        let bytes = encode(&headers, b"hello");

        let response = decode(&bytes).expect("decodes");
        assert_eq!(response.status(), http::StatusCode::OK);
        assert_eq!(response.headers()["content-type"], "text/html");
        assert_eq!(response.headers().get_all("link").iter().count(), 2);
        assert!(!response.headers().contains_key("content-length"));
        assert_eq!(response.headers()[STATUS_HEADER], "hit");
        assert_eq!(response.body().buffered(), Some(&b"hello"[..]));
    }

    #[test]
    fn bytes_the_encoder_did_not_write_are_a_miss() {
        assert!(decode(b"").is_none());
        assert!(decode(b"BRC1\x00\x00\x00\x01\x00").is_none());
        assert!(decode(b"<html>").is_none());
    }

    #[test]
    fn the_tags_header_is_read_and_removed() {
        let mut h = head(
            200,
            &[(TAGS_HEADER, "posts, posts:1"), (TAGS_HEADER, "users:2")],
        );
        let tags = take_header_tags(&mut h.headers);
        let names: Vec<&str> = tags.iter().map(Tag::as_str).collect();
        assert_eq!(names, ["posts", "posts:1", "users:2"]);
        assert!(!h.headers.contains_key(TAGS_HEADER));
    }
}
