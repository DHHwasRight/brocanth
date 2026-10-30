//! How many requests one caller may make in a window.
//!
//! A fixed window per key. Each [`Rule`] names a group of routes, a limit,
//! a window in seconds and what the key is. A request counts against
//! every rule that matches it. When one rule is over its limit, the
//! request gets a 429 with `Retry-After` and the `RateLimit-*` headers of
//! that rule, and the handler does not run.
//!
//! # Fixed window, not token bucket
//!
//! A fixed window is one counter and one end time per key. In Postgres
//! that is one row and one `INSERT … ON CONFLICT DO UPDATE`, with no read
//! before the write and no lock held across a round trip. A token bucket
//! needs a refill computed from the last time, so it is a read and a
//! write, or a longer statement, for a smoother curve. The cost of the
//! fixed window is the edge: a caller can spend a whole limit at the end
//! of one window and another at the start of the next. For the job this
//! does (stop a password guesser, stop a scraper) twice the limit for one
//! moment is acceptable. Halve the limit if it is not.
//!
//! # The key
//!
//! [`Key`] says what a request is counted as. A rule lists more than one,
//! and the first one the request has is used: `["user", "ip"]` counts a
//! signed-in visitor by account and everybody else by address.
//!
//! - [`Key::Ip`]: the header named by [`RateLimitConfig::ip_header`] if
//!   it is set, then [`ClientInfo`] from a trusted proxy
//!   ([`ProxyHeaders`](crate::ProxyHeaders)), then the socket's
//!   [`PeerAddress`]. `ip_header` is for an edge that sets one header
//!   and strips it from the client (`Fly-Client-IP`, `CF-Connecting-IP`).
//!   Name it only when that is true, or a client picks its own key.
//! - [`Key::Session`]: the cookie named by
//!   [`RateLimitConfig::session_cookie`]. Its SHA-256 is the key, not the
//!   value, so a session id never reaches the store.
//! - [`Key::User`]: a [`Subject`] on the request state, which the
//!   application's own authentication hook puts there. Order this hook
//!   after that one with [`RateLimitConfig::after`].
//!
//! A request that has none of a rule's keys is not counted by that rule.
//! A shared bucket for "unknown" would let one caller lock out every
//! other caller that has no key, which is worse than no limit.
//!
//! # Two plugins, one hook
//!
//! [`RateLimit`] counts in this process, in a [`MemoryStore`], with a
//! synchronous request hook. That is the default, and it works with
//! `App::run`, `Bridge` and `TestClient::send`.
//!
//! [`SharedRateLimit`] counts in any [`Store`], [`PgStore`] included, so
//! every process behind the load balancer shares one count. A store
//! that waits on the network needs an asynchronous request hook, and an
//! application with one must serve through `App::begin_async`. The core
//! makes that a compile error, not a silent skip. See
//! `docs/middleware.md` §6.
//!
//! # When the store fails
//!
//! [`RateLimitConfig::fail_open`] decides. By default a store error lets
//! the request through: a rate limiter that is down should not take the
//! site down with it. Set it to `false` for a rule that guards something
//! worth an outage, such as a login form.

mod memory;
#[cfg(feature = "postgres")]
mod postgres;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    AsyncRequestHook, BodyShape, Both, Flow, HookRegistrar, HookSource, OnAsyncRequest, OnRequest,
    OnResponse, Request, RequestHook, RequestState, Response, ResponseHead, ResponseHook,
};
use brocanth::plugin_order::Order;
use brocanth::plugin_routes::BoxFuture;
use sha2::Digest as _;

use crate::proxy_headers::PeerAddress;
use crate::ClientInfo;

pub use memory::MemoryStore;
#[cfg(feature = "postgres")]
pub use postgres::PgStore;

/// What a request is counted as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum Key {
    /// The client's address.
    Ip,
    /// The session cookie, hashed.
    Session,
    /// The [`Subject`] the application put on the request state.
    User,
}

/// The signed-in account, as the rate limiter counts it.
///
/// The application's authentication hook inserts one into the request
/// state. This crate does not know what a user is, so it reads this
/// type and nothing else.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Subject(pub String);

/// One group of routes and its limit.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(deny_unknown_fields)
)]
pub struct Rule {
    /// The rule's name. It is part of every key it counts, so two rules
    /// never share a counter, and it is in the 429's text.
    pub name: String,
    /// Path prefixes, matched by segment: `/login` covers `/login` and
    /// `/login/totp`, and does not cover `/loginhelp`. `/` covers every
    /// path.
    #[cfg_attr(feature = "config", serde(default = "everything"))]
    pub paths: Vec<String>,
    /// Verbs, upper case. Empty means every verb.
    #[cfg_attr(feature = "config", serde(default))]
    pub methods: Vec<String>,
    /// Requests allowed per window.
    pub limit: u64,
    /// The window, in whole seconds.
    pub window_secs: u64,
    /// What the request is counted as, first match wins.
    #[cfg_attr(feature = "config", serde(default = "by_ip"))]
    pub key: Vec<Key>,
}

#[cfg(feature = "config")]
fn everything() -> Vec<String> {
    vec!["/".to_string()]
}

#[cfg(feature = "config")]
fn by_ip() -> Vec<Key> {
    vec![Key::Ip]
}

impl Rule {
    /// `limit` requests per `window_secs`, on every path, by address.
    pub fn new(name: impl Into<String>, limit: u64, window_secs: u64) -> Self {
        Self {
            name: name.into(),
            paths: vec!["/".to_string()],
            methods: Vec::new(),
            limit,
            window_secs,
            key: vec![Key::Ip],
        }
    }

    /// Only under these path prefixes.
    #[must_use]
    pub fn paths(mut self, paths: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.paths = paths.into_iter().map(Into::into).collect();
        self
    }

    /// Only for these verbs.
    #[must_use]
    pub fn methods(mut self, methods: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.methods = methods.into_iter().map(Into::into).collect();
        self
    }

    /// Count by these keys, first match wins.
    #[must_use]
    pub fn key(mut self, keys: impl IntoIterator<Item = Key>) -> Self {
        self.key = keys.into_iter().collect();
        self
    }

    fn matches(&self, request: &Request) -> bool {
        let verb = request.method().as_str();
        let verb_ok =
            self.methods.is_empty() || self.methods.iter().any(|m| m.eq_ignore_ascii_case(verb));
        verb_ok && self.paths.iter().any(|p| under(request.uri().path(), p))
    }

    fn window(&self) -> Duration {
        Duration::from_secs(self.window_secs)
    }
}

/// Whether `path` is `prefix` or under it, by whole segments.
fn under(path: &str, prefix: &str) -> bool {
    let prefix = prefix.trim_end_matches('/');
    prefix.is_empty()
        || path == prefix
        || (path.starts_with(prefix) && path[prefix.len()..].starts_with('/'))
}

/// The rules, and where the keys come from.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct RateLimitConfig {
    /// Every rule. A request counts against each one that matches.
    pub rules: Vec<Rule>,
    /// A header that holds the client address, set by an edge that
    /// strips it from the client. `None` reads the address from
    /// [`ClientInfo`] or the socket.
    pub ip_header: Option<String>,
    /// The session cookie's name, for [`Key::Session`].
    pub session_cookie: Option<String>,
    /// Hook ids this hook runs after, when they are installed: the hook
    /// that puts a [`Subject`] on the state, for one.
    /// `proxy-headers.read` is always in the list.
    pub after: Vec<String>,
    /// Let a request through when the store fails.
    pub fail_open: bool,
}

impl Default for RateLimitConfig {
    /// No rules. A limit is a number about this application's traffic,
    /// and no default number is right for every application.
    fn default() -> Self {
        Self {
            rules: Vec::new(),
            ip_header: None,
            session_cookie: None,
            after: Vec::new(),
            fail_open: true,
        }
    }
}

impl RateLimitConfig {
    /// These rules, and the defaults for the rest.
    pub fn rules(rules: impl IntoIterator<Item = Rule>) -> Self {
        Self {
            rules: rules.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Read the address from this header. Only behind an edge that sets
    /// it and strips the client's copy.
    #[must_use]
    pub fn ip_header(mut self, name: impl Into<String>) -> Self {
        self.ip_header = Some(name.into());
        self
    }

    /// The session cookie, for [`Key::Session`].
    #[must_use]
    pub fn session_cookie(mut self, name: impl Into<String>) -> Self {
        self.session_cookie = Some(name.into());
        self
    }

    /// Run after this hook id, when it is installed.
    #[must_use]
    pub fn after(mut self, hook: impl Into<String>) -> Self {
        self.after.push(hook.into());
        self
    }

    /// Answer 503 when the store fails, instead of letting the request
    /// through.
    #[must_use]
    pub fn fail_closed(mut self) -> Self {
        self.fail_open = false;
        self
    }

    /// Why this configuration cannot work, if it cannot.
    ///
    /// # Errors
    ///
    /// The reason, with what to change.
    pub fn check(&self) -> Result<(), String> {
        let mut seen = std::collections::HashSet::new();
        for rule in &self.rules {
            if rule.name.is_empty() || !seen.insert(rule.name.as_str()) {
                return Err(format!(
                    "the rule name `{}` is empty or used twice. help: give each rule its own name",
                    rule.name
                ));
            }
            if rule.limit == 0 || rule.window_secs == 0 {
                return Err(format!(
                    "rule `{}` has a limit or a window of 0. help: a route nobody may call \
                     wants no route; set both to at least 1",
                    rule.name
                ));
            }
            if rule.key.is_empty() || rule.paths.is_empty() {
                return Err(format!(
                    "rule `{}` has no key or no path, so it counts nothing. help: add one of \
                     each, such as `key = [\"ip\"]` and `paths = [\"/\"]`",
                    rule.name
                ));
            }
            if rule.key.contains(&Key::Session) && self.session_cookie.is_none() {
                return Err(format!(
                    "rule `{}` counts by session, and `session_cookie` is not set. help: set \
                     `session_cookie` to the name of the session cookie",
                    rule.name
                ));
            }
        }
        if let Some(header) = &self.ip_header {
            if http::header::HeaderName::from_bytes(header.as_bytes()).is_err() {
                return Err(format!(
                    "`ip_header` is `{header}`, which is not a header name. help: use the \
                     name your edge sets, such as `fly-client-ip`"
                ));
            }
        }
        Ok(())
    }
}

/// One count from the store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Requests in this window, this one included.
    pub count: u64,
    /// Time until the window ends and the count starts again.
    pub resets_in: Duration,
}

/// Why the store did not count.
#[derive(Debug)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

/// Where the counters live.
///
/// One operation: add one to `key` in the window of length `window`
/// that holds now, and say the new count and when the window ends. The
/// store owns the clock, so every process that shares a store agrees on
/// where a window starts.
pub trait Store: Send + Sync + 'static {
    /// Count one request.
    fn hit<'a>(&'a self, key: &'a str, window: Duration) -> BoxFuture<'a, Result<Hit, StoreError>>;
}

/// The state of the rule nearest its limit, for the response headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RateLimitStatus {
    /// The rule's name.
    pub rule: String,
    /// Its limit.
    pub limit: u64,
    /// What is left in this window.
    pub remaining: u64,
    /// Seconds until the window ends.
    pub reset_secs: u64,
    /// The window, in seconds.
    pub window_secs: u64,
}

impl RateLimitStatus {
    fn apply(&self, headers: &mut http::HeaderMap) {
        let num = |n: u64| http::HeaderValue::from(n);
        headers.insert("ratelimit-limit", num(self.limit));
        headers.insert("ratelimit-remaining", num(self.remaining));
        headers.insert("ratelimit-reset", num(self.reset_secs));
        if let Ok(policy) =
            http::HeaderValue::from_str(&format!("{};w={}", self.limit, self.window_secs))
        {
            headers.insert("ratelimit-policy", policy);
        }
    }
}

/// The part of both plugins that does not wait: which rules apply, what
/// each one counts, and what the counts mean.
struct Limiter {
    config: RateLimitConfig,
}

impl Limiter {
    /// `(rule index, store key)` for every rule that counts this request.
    fn plan(&self, request: &Request, state: &RequestState) -> Vec<(usize, String)> {
        self.config
            .rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.matches(request))
            .filter_map(|(at, rule)| {
                let key = rule.key.iter().find_map(|k| self.key(*k, request, state))?;
                Some((at, format!("{}:{key}", rule.name)))
            })
            .collect()
    }

    fn key(&self, key: Key, request: &Request, state: &RequestState) -> Option<String> {
        match key {
            Key::Ip => self.address(request, state).map(|ip| format!("ip:{ip}")),
            Key::Session => {
                let name = self.config.session_cookie.as_deref()?;
                let value = cookie(request.headers(), name)?;
                let digest = sha2::Sha256::digest(value.as_bytes());
                Some(format!("session:{}", hex(&digest[..16])))
            }
            Key::User => state.get::<Subject>().map(|s| format!("user:{}", s.0)),
        }
    }

    fn address(&self, request: &Request, state: &RequestState) -> Option<IpAddr> {
        if let Some(header) = &self.config.ip_header {
            return request
                .headers()
                .get(header.as_str())
                .and_then(|v| v.to_str().ok())
                .map(crate::first)
                .and_then(|v| v.parse().ok());
        }
        state
            .get::<ClientInfo>()
            .and_then(|info| info.address)
            .or_else(|| PeerAddress::of(request))
    }

    /// What the counts mean: a 429, or the status to put on the response.
    fn decide(
        &self,
        hits: &[(usize, Result<Hit, StoreError>)],
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let mut nearest: Option<RateLimitStatus> = None;
        for (at, hit) in hits {
            let rule = &self.config.rules[*at];
            let hit = match hit {
                Ok(hit) => hit,
                Err(e) if self.config.fail_open => {
                    state.insert(StoreFailed(e.0.clone()));
                    continue;
                }
                Err(e) => return Ok(Flow::respond(unavailable(&rule.name, e))),
            };
            let status = RateLimitStatus {
                rule: rule.name.clone(),
                limit: rule.limit,
                remaining: rule.limit.saturating_sub(hit.count),
                reset_secs: ceil_secs(hit.resets_in),
                window_secs: rule.window_secs,
            };
            if hit.count > rule.limit {
                return Ok(Flow::respond(refuse(&status)));
            }
            if nearest
                .as_ref()
                .is_none_or(|n| status.remaining < n.remaining)
            {
                nearest = Some(status);
            }
        }
        if let Some(status) = nearest {
            state.insert(status);
        }
        Ok(Flow::Continue)
    }
}

/// The store failed and the request went through. On the state, so a
/// logger can say so.
#[derive(Clone, Debug)]
pub struct StoreFailed(pub String);

fn ceil_secs(d: Duration) -> u64 {
    d.as_secs() + u64::from(d.subsec_nanos() > 0)
}

fn refuse(status: &RateLimitStatus) -> Response {
    let text = format!(
        "too many requests: rule `{}` allows {} per {}s. help: retry in {}s\n",
        status.rule, status.limit, status.window_secs, status.reset_secs
    );
    let mut response = Response::new(Body::from(text.into_bytes()));
    *response.status_mut() = http::StatusCode::TOO_MANY_REQUESTS;
    let headers = response.headers_mut();
    headers.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(
        http::header::RETRY_AFTER,
        http::HeaderValue::from(status.reset_secs),
    );
    status.apply(headers);
    response
}

fn unavailable(rule: &str, e: &StoreError) -> Response {
    let text = format!("the rate limit store failed for rule `{rule}`: {e}. help: retry shortly\n");
    let mut response = Response::new(Body::from(text.into_bytes()));
    *response.status_mut() = http::StatusCode::SERVICE_UNAVAILABLE;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

fn cookie<'a>(headers: &'a http::HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(http::header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|line| line.split(';'))
        .find_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            (k.trim() == name).then(|| v.trim())
        })
        .filter(|v| !v.is_empty())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn order(config: &RateLimitConfig) -> Order {
    // After the proxy reader, so `ClientInfo` is there to read.
    let mut order = Order::new().after_if_present("proxy-headers.read");
    for hook in &config.after {
        order = order.after_if_present(hook.clone());
    }
    order
}

/// Puts the `RateLimit-*` headers on a response that went through.
struct Headers;

impl ResponseHook for Headers {
    fn on_headers(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        _body: BodyShape<'_>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        if let Some(status) = state.get::<RateLimitStatus>() {
            if !head.headers.contains_key("ratelimit-limit") {
                status.apply(&mut head.headers);
            }
        }
        Ok(())
    }
}

/// Rate limiting in this process, with a [`MemoryStore`].
///
/// The default. Each process counts alone, so behind a load balancer
/// with `n` processes a caller gets up to `n` times the limit. Use
/// [`SharedRateLimit`] with a [`PgStore`] when that matters.
pub struct RateLimit {
    config: RateLimitConfig,
    store: Arc<MemoryStore>,
}

impl RateLimit {
    /// The same plugin, counting in a store the caller holds. A test
    /// moves that store's clock.
    pub fn with_store(config: RateLimitConfig, store: Arc<MemoryStore>) -> Self {
        Self { config, store }
    }
}

impl Plugin for RateLimit {
    type Config = RateLimitConfig;
    type Provides = NoContext;
    /// Also the configuration section.
    const NAME: &'static str = "rate_limit";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self {
            config,
            store: Arc::new(MemoryStore::new()),
        }
    }
}

impl HookSource for RateLimit {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        self.config
            .check()
            .map_err(|why| PluginError::new(format!("rate_limit: {why}")))?;
        registrar.request(
            "count",
            order(&self.config),
            Local {
                limiter: Limiter {
                    config: self.config.clone(),
                },
                store: self.store.clone(),
            },
        );
        registrar.response("headers", Order::new(), Headers);
        Ok(())
    }
}

struct Local {
    limiter: Limiter,
    store: Arc<MemoryStore>,
}

impl RequestHook for Local {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let hits: Vec<_> = self
            .limiter
            .plan(request, state)
            .into_iter()
            .map(|(at, key)| {
                let window = self.limiter.config.rules[at].window();
                (at, Ok(self.store.count(&key, window)))
            })
            .collect();
        self.limiter.decide(&hits, state)
    }
}

/// What [`SharedRateLimit`] is built from.
#[derive(Clone)]
pub struct SharedConfig {
    /// The rules.
    pub config: RateLimitConfig,
    /// Where the counts live.
    pub store: Arc<dyn Store>,
}

/// Rate limiting with a store every process shares.
///
/// Installs an asynchronous request hook, because a shared store waits
/// on the network. An application with one serves through
/// `App::begin_async`, and `App::run` is a compile error for it.
pub struct SharedRateLimit {
    config: SharedConfig,
}

impl Plugin for SharedRateLimit {
    type Config = SharedConfig;
    type Provides = NoContext;
    /// The same section as [`RateLimit`]: an application registers one
    /// or the other.
    const NAME: &'static str = "rate_limit";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for SharedRateLimit {
    type Families = Both<OnAsyncRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        self.config
            .config
            .check()
            .map_err(|why| PluginError::new(format!("rate_limit: {why}")))?;
        registrar.request_async(
            "count",
            order(&self.config.config),
            Remote {
                limiter: Limiter {
                    config: self.config.config.clone(),
                },
                store: self.config.store.clone(),
            },
        );
        registrar.response("headers", Order::new(), Headers);
        Ok(())
    }
}

struct Remote {
    limiter: Limiter,
    store: Arc<dyn Store>,
}

impl AsyncRequestHook for Remote {
    fn before_dispatch<'a>(
        &'a self,
        request: &'a mut Request,
        state: &'a mut RequestState,
    ) -> BoxFuture<'a, Result<Flow, PluginError>> {
        Box::pin(async move {
            let plan = self.limiter.plan(request, state);
            let mut hits = Vec::with_capacity(plan.len());
            for (at, key) in plan {
                let window = self.limiter.config.rules[at].window();
                hits.push((at, self.store.hit(&key, window).await));
            }
            self.limiter.decide(&hits, state)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: http::Method, path: &str, headers: &[(&str, &str)]) -> Request {
        let mut builder = http::Request::builder().method(method).uri(path);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(Vec::new()).expect("a request")
    }

    fn limiter(config: RateLimitConfig) -> Limiter {
        config.check().expect("a valid configuration");
        Limiter { config }
    }

    #[test]
    fn a_rule_matches_by_segment_and_verb() {
        let rule = Rule::new("login", 5, 60)
            .paths(["/login/"])
            .methods(["post"]);
        let post = |p: &str| request(http::Method::POST, p, &[]);
        assert!(rule.matches(&post("/login")));
        assert!(rule.matches(&post("/login/totp")));
        assert!(!rule.matches(&post("/loginhelp")));
        assert!(!rule.matches(&request(http::Method::GET, "/login", &[])));
        assert!(Rule::new("all", 1, 1).matches(&post("/anything")));
    }

    #[test]
    fn the_first_key_the_request_has_is_used() {
        let l = limiter(
            RateLimitConfig::rules([Rule::new("api", 10, 60).key([Key::User, Key::Ip])])
                .ip_header("fly-client-ip"),
        );
        let anonymous = request(http::Method::GET, "/", &[("fly-client-ip", "192.0.2.7")]);
        let mut state = RequestState::new();
        assert_eq!(
            l.plan(&anonymous, &state),
            vec![(0, "api:ip:192.0.2.7".into())]
        );

        state.insert(Subject("42".into()));
        assert_eq!(l.plan(&anonymous, &state), vec![(0, "api:user:42".into())]);
    }

    #[test]
    fn a_request_with_no_key_is_not_counted() {
        let l = limiter(RateLimitConfig::rules([Rule::new("api", 10, 60)]));
        assert!(l
            .plan(&request(http::Method::GET, "/", &[]), &RequestState::new())
            .is_empty());
    }

    #[test]
    fn the_address_comes_from_the_trusted_proxy_then_the_socket() {
        let l = limiter(RateLimitConfig::rules([Rule::new("api", 10, 60)]));
        let mut req = request(
            http::Method::GET,
            "/",
            &[("x-forwarded-for", "203.0.113.9")],
        );
        req.extensions_mut()
            .insert(PeerAddress("10.0.0.1".parse().unwrap()));
        let mut state = RequestState::new();
        assert_eq!(l.plan(&req, &state)[0].1, "api:ip:10.0.0.1");

        state.insert(ClientInfo {
            address: Some("203.0.113.9".parse().unwrap()),
            scheme: None,
            host: None,
            chain: Vec::new(),
        });
        assert_eq!(l.plan(&req, &state)[0].1, "api:ip:203.0.113.9");
    }

    #[test]
    fn the_socket_address_the_server_bridge_sets_is_a_key() {
        let l = limiter(RateLimitConfig::rules([Rule::new("api", 10, 60)]));
        let mut req = request(http::Method::GET, "/", &[]);
        req.extensions_mut()
            .insert(std::net::SocketAddr::from(([192, 0, 2, 7], 4000)));
        assert_eq!(l.plan(&req, &RequestState::new())[0].1, "api:ip:192.0.2.7");
    }

    #[test]
    fn a_session_is_counted_by_its_hash_and_never_by_its_value() {
        let l = limiter(
            RateLimitConfig::rules([Rule::new("s", 10, 60).key([Key::Session])])
                .session_cookie("_app_session"),
        );
        let req = request(
            http::Method::GET,
            "/",
            &[("cookie", "theme=dark; _app_session=secret-value")],
        );
        let key = &l.plan(&req, &RequestState::new())[0].1;
        assert!(key.starts_with("s:session:"), "{key}");
        assert!(!key.contains("secret-value"), "{key}");
    }

    #[test]
    fn over_the_limit_is_a_429_with_the_headers_a_client_reads() {
        let l = limiter(RateLimitConfig::rules([Rule::new("api", 2, 60)]));
        let hit = |count| {
            vec![(
                0,
                Ok(Hit {
                    count,
                    resets_in: Duration::from_millis(41_200),
                }),
            )]
        };
        let mut state = RequestState::new();
        assert!(l.decide(&hit(2), &mut state).unwrap().is_continue());
        let status = state.get::<RateLimitStatus>().unwrap();
        assert_eq!((status.remaining, status.reset_secs), (0, 42));

        let Flow::Respond(refused) = l.decide(&hit(3), &mut RequestState::new()).unwrap() else {
            panic!("the third request went through");
        };
        assert_eq!(refused.status(), 429);
        let h = refused.headers();
        assert_eq!(h["retry-after"], "42");
        assert_eq!(h["ratelimit-limit"], "2");
        assert_eq!(h["ratelimit-remaining"], "0");
        assert_eq!(h["ratelimit-reset"], "42");
        assert_eq!(h["ratelimit-policy"], "2;w=60");
    }

    #[test]
    fn a_store_error_lets_the_request_through_unless_told_otherwise() {
        let failed = || vec![(0, Err(StoreError("connection refused".into())))];
        let open = limiter(RateLimitConfig::rules([Rule::new("api", 2, 60)]));
        let mut state = RequestState::new();
        assert!(open.decide(&failed(), &mut state).unwrap().is_continue());
        assert!(state.get::<StoreFailed>().is_some());

        let closed = limiter(RateLimitConfig::rules([Rule::new("api", 2, 60)]).fail_closed());
        let Flow::Respond(r) = closed.decide(&failed(), &mut RequestState::new()).unwrap() else {
            panic!("fail_closed let it through");
        };
        assert_eq!(r.status(), 503);
    }

    #[test]
    fn a_configuration_that_counts_nothing_is_a_startup_error() {
        let bad = [
            RateLimitConfig::rules([Rule::new("a", 0, 60)]),
            RateLimitConfig::rules([Rule::new("a", 1, 0)]),
            RateLimitConfig::rules([Rule::new("a", 1, 1), Rule::new("a", 1, 1)]),
            RateLimitConfig::rules([Rule::new("a", 1, 1).key([])]),
            RateLimitConfig::rules([Rule::new("a", 1, 1).key([Key::Session])]),
            RateLimitConfig::rules([Rule::new("a", 1, 1)]).ip_header("bad header"),
        ];
        for config in bad {
            let why = config.check().expect_err("refused");
            assert!(why.contains("help:"), "{why}");
        }
    }
}
