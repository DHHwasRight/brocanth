//! A browser on another origin.
//!
//! Two halves, and they are different enough that conflating them is the
//! usual bug:
//!
//! **The preflight.** An `OPTIONS` carrying `Access-Control-Request-Method`
//! is the browser asking permission. It never reaches a handler; this hook
//! answers it with `Flow::Respond`.
//!
//! **The real request.** A `GET` or `POST` with an `Origin` goes through to
//! the handler as normal, and the response needs
//! `Access-Control-Allow-Origin` on it or the browser throws the body away
//! after receiving it. So the answer is a [`ResponseHook`], not something
//! the preflight can have set.
//!
//! # The two rules that are not negotiable
//!
//! 1. **`*` and credentials cannot both be true.** The specification says a
//!    wildcard origin with `Access-Control-Allow-Credentials: true` is an
//!    error, and browsers refuse it. [`CorsConfig::check`] refuses it here
//!    instead, at startup, so it is a line in a log rather than a bug that
//!    reproduces only in a browser.
//! 2. **An allowed origin is echoed, never reflected blindly.** When the
//!    configuration names origins, the response carries the request's own
//!    origin — but only after it matched the list — and a `Vary: Origin`,
//!    so a cache cannot serve one origin's response to another.
//!
//! # The verbs of the route
//!
//! Gorilla's `CORSMethodMiddleware`: with [`CorsConfig::route_verbs`], a
//! preflight answers the verbs the path's route has, inside
//! [`CorsConfig::methods`]. The configuration stays the ceiling, so a new
//! `DELETE` page is not open to other origins by accident. A preflight for
//! a verb the route lacks, or for a path with no route, gets no
//! `Access-Control-Allow-*` header, and the browser refuses the real
//! request, as it does for a verb outside the list. Without the function,
//! a preflight answers [`CorsConfig::methods`] for every path.
//!
//! [`ResponseHook`]: brocanth::plugin_hooks::ResponseHook

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookRegistrar, HookSource, OnRequest, OnResponse, Request, RequestHook,
    RequestState, Response, ResponseHead, ResponseHook,
};
use brocanth::routing::{Allow, Verb};

use std::sync::Arc;

use crate::{add_vary, text};

/// The wildcard, as a borrow that outlives the call.
const ANY: &str = "*";

/// Who may ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origins {
    /// Every origin, answered with `*`. Cannot be combined with
    /// credentials, and [`CorsConfig::check`] says so.
    Any,
    /// These origins, compared exactly and case-sensitively, because an
    /// origin is a scheme, a host and a port and not a name.
    These(Vec<String>),
}

/// The verbs of a path's route, or `None` when no route has that path.
///
/// `Cors` runs before dispatch and cannot name the project's route table,
/// so the project hands it this function. A generated project passes one
/// over `pages::router()`: `move |path| router.allow_for(path)`.
#[derive(Clone)]
pub struct RouteVerbs(Arc<VerbsOf>);

/// The function inside [`RouteVerbs`].
type VerbsOf = dyn Fn(&str) -> Option<Allow> + Send + Sync;

impl RouteVerbs {
    pub fn new(verbs: impl Fn(&str) -> Option<Allow> + Send + Sync + 'static) -> Self {
        Self(Arc::new(verbs))
    }

    /// The verbs of `path`'s route, or `None` for a path with no route.
    pub fn of(&self, path: &str) -> Option<Allow> {
        (self.0)(path)
    }
}

impl std::fmt::Debug for RouteVerbs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RouteVerbs(..)")
    }
}

/// What to allow.
#[derive(Clone, Debug)]
pub struct CorsConfig {
    pub origins: Origins,
    /// The verbs a cross-origin request may use. With
    /// [`CorsConfig::route_verbs`], the ceiling: a preflight answers the
    /// verbs of this list that the route also has.
    pub methods: Vec<http::Method>,
    /// The verbs of each path's route. `None`: every path answers
    /// [`CorsConfig::methods`].
    pub route_verbs: Option<RouteVerbs>,
    /// The request headers a cross-origin request may send.
    pub headers: Vec<String>,
    /// The response headers the page's script may read. The browser hides
    /// every other one, which is why a missing entry here reads as "the
    /// header is not being sent".
    pub expose: Vec<String>,
    /// How long a browser may cache the preflight answer.
    pub max_age: Option<std::time::Duration>,
    /// Whether cookies and `Authorization` may be sent.
    pub credentials: bool,
    /// What to answer a preflight with. 204 is the usual choice; Gorilla
    /// defaults to 200 because some old clients choke on a 204.
    pub preflight_status: http::StatusCode,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            origins: Origins::Any,
            methods: vec![http::Method::GET, http::Method::HEAD, http::Method::POST],
            route_verbs: None,
            headers: Vec::new(),
            expose: Vec::new(),
            max_age: None,
            credentials: false,
            preflight_status: http::StatusCode::NO_CONTENT,
        }
    }
}

impl CorsConfig {
    /// Allow these origins and nothing else.
    pub fn from(origins: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            origins: Origins::These(origins.into_iter().map(Into::into).collect()),
            ..Self::default()
        }
    }

    pub fn methods(mut self, methods: impl IntoIterator<Item = http::Method>) -> Self {
        self.methods = methods.into_iter().collect();
        self
    }

    /// Answer a preflight with the verbs of the path's route, inside
    /// [`CorsConfig::methods`]. A preflight for a verb the route lacks, or
    /// for a path with no route, gets no `Access-Control-Allow-*` header.
    ///
    /// ```
    /// use brocanth::routing::{Allow, Verb};
    /// use brocanth_middleware::CorsConfig;
    ///
    /// // A generated project: `move |path| router.allow_for(path)`.
    /// let config = CorsConfig::from(["https://app.example.com"])
    ///     .route_verbs(|path| (path == "/items").then(|| Allow::empty().with(Verb::Get)));
    /// # let _ = config;
    /// ```
    pub fn route_verbs(
        mut self,
        verbs: impl Fn(&str) -> Option<Allow> + Send + Sync + 'static,
    ) -> Self {
        self.route_verbs = Some(RouteVerbs::new(verbs));
        self
    }

    pub fn headers(mut self, headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.headers = headers.into_iter().map(Into::into).collect();
        self
    }

    pub fn expose(mut self, headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.expose = headers.into_iter().map(Into::into).collect();
        self
    }

    pub fn max_age(mut self, age: std::time::Duration) -> Self {
        self.max_age = Some(age);
        self
    }

    /// Allow cookies and `Authorization`. Refuses to go with [`Origins::Any`].
    pub fn with_credentials(mut self) -> Self {
        self.credentials = true;
        self
    }

    /// Why this configuration cannot work, if it cannot.
    ///
    /// Checked when the plugin is built, so a combination browsers refuse
    /// is a startup error here rather than a silent failure in one.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.credentials && self.origins == Origins::Any {
            return Err(
                "credentials cannot be allowed for every origin: name the origins instead \
                 of `Origins::Any`, because a browser refuses `*` with credentials\n\
                 help: name the origins with `Origins::These`, or set `credentials` to false",
            );
        }
        if let Origins::These(list) = &self.origins {
            if list.is_empty() {
                return Err(
                    "no origin is allowed, so every cross-origin request is refused\n\
                     help: name at least one origin in `Origins::These`, or leave the plugin out",
                );
            }
        }
        Ok(())
    }
}

/// Answers preflights, and marks the real responses.
pub struct Cors {
    config: CorsConfig,
}

impl Plugin for Cors {
    type Config = CorsConfig;
    type Provides = NoContext;
    const NAME: &'static str = "cors";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for Cors {
    type Families = Both<OnRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        self.config
            .check()
            .map_err(|why| PluginError::new(format!("cors: {}", why)))?;
        registrar.request(
            "preflight",
            brocanth::plugin_order::Phase::Guard,
            brocanth::plugin_order::Order::new(),
            Hook {
                config: self.config.clone(),
            },
        );
        registrar.response(
            "mark",
            brocanth::plugin_order::Phase::Guard,
            brocanth::plugin_order::Order::new(),
            Hook {
                config: self.config.clone(),
            },
        );
        Ok(())
    }
}

struct Hook {
    config: CorsConfig,
}

impl Hook {
    /// The value for `Access-Control-Allow-Origin`, or `None` when this
    /// origin is not allowed.
    fn allow<'a>(&self, origin: &'a str) -> Option<&'a str> {
        match &self.config.origins {
            Origins::Any if self.config.credentials => None,
            Origins::Any => Some(ANY),
            Origins::These(list) => list.iter().any(|o| o == origin).then_some(origin),
        }
    }
}

impl Hook {
    /// The verbs a preflight for `path` may name, or `None` when the
    /// preflight is refused.
    ///
    /// Without [`CorsConfig::route_verbs`], the configured list for every
    /// path, whatever verb the preflight asks about: the browser compares.
    /// With it, the configured verbs the route also has. A preflight that
    /// asks for a verb outside that set is refused here, so the browser
    /// sees no permission at all.
    fn methods(&self, path: &str, headers: &http::HeaderMap) -> Option<Vec<&str>> {
        let configured = self.config.methods.iter();
        let Some(route) = &self.config.route_verbs else {
            return Some(configured.map(http::Method::as_str).collect());
        };
        let has = route.of(path).unwrap_or(Allow::empty());
        let methods: Vec<&str> = configured
            .filter(|m| Verb::from_http(m).is_some_and(|verb| has.contains(verb)))
            .map(http::Method::as_str)
            .collect();
        let asked = text(headers, http::header::ACCESS_CONTROL_REQUEST_METHOD)?;
        methods.contains(&asked).then_some(methods)
    }
}

impl RequestHook for Hook {
    fn before_dispatch(
        &self,
        request: &mut Request,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        // A preflight is an OPTIONS that names the method it is asking
        // about. An OPTIONS without one is an ordinary OPTIONS, and this
        // hook must not eat it.
        if !is_preflight(request) {
            return Ok(Flow::Continue);
        }
        let headers = request.headers().clone();
        let Some(origin) = text(&headers, http::header::ORIGIN) else {
            return Ok(Flow::Continue);
        };

        let mut response = Response::new(Body::from(Vec::new()));
        *response.status_mut() = self.config.preflight_status;
        let out = response.headers_mut();
        // `Vary: Origin` even when the origin was refused, because the
        // refusal itself depends on the origin and a cache must not reuse
        // it for another.
        add_vary(out, "Origin");

        let Some(allow) = self.allow(origin) else {
            // The specification's way to refuse is to answer without the
            // header. The browser then blocks the real request and says
            // why, which is a better message than anything we could put
            // in a body nobody reads.
            return Ok(Flow::respond(response));
        };
        let Some(methods) = self.methods(request.uri().path(), &headers) else {
            // A verb the route lacks, or no route: refused the same way.
            return Ok(Flow::respond(response));
        };
        put(out, "access-control-allow-origin", allow)?;
        put(out, "access-control-allow-methods", &methods.join(", "))?;
        if !self.config.headers.is_empty() {
            put(
                out,
                "access-control-allow-headers",
                &self.config.headers.join(", "),
            )?;
        }
        if self.config.credentials {
            put(out, "access-control-allow-credentials", "true")?;
        }
        if let Some(age) = self.config.max_age {
            put(out, "access-control-max-age", &age.as_secs().to_string())?;
        }
        Ok(Flow::respond(response))
    }
}

impl ResponseHook for Hook {
    fn on_headers(
        &self,
        request: &Request,
        head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let Some(origin) = text(request.headers(), http::header::ORIGIN) else {
            // Not a cross-origin request. Adding the headers anyway would
            // make every response vary by a header that was never sent.
            return Ok(());
        };
        // Merged rather than inserted: a handler may already vary by
        // something, and replacing that would poison its cache. Merged
        // rather than appended: a preflight already carries `Origin`, and
        // a second line would name it twice.
        add_vary(&mut head.headers, "Origin");
        if is_preflight(request) {
            // The preflight hook wrote every header this answer carries.
            // Marking it again would put `Access-Control-Allow-Origin` on a
            // preflight that hook refused by leaving it out.
            return Ok(());
        }
        let Some(allow) = self.allow(origin) else {
            return Ok(());
        };
        put(&mut head.headers, "access-control-allow-origin", allow)?;
        if self.config.credentials {
            put(
                &mut head.headers,
                "access-control-allow-credentials",
                "true",
            )?;
        }
        if !self.config.expose.is_empty() {
            put(
                &mut head.headers,
                "access-control-expose-headers",
                &self.config.expose.join(", "),
            )?;
        }
        Ok(())
    }
}

/// An `OPTIONS` that names the method it asks about.
fn is_preflight(request: &Request) -> bool {
    request.method() == http::Method::OPTIONS
        && request
            .headers()
            .contains_key(http::header::ACCESS_CONTROL_REQUEST_METHOD)
}

fn put(headers: &mut http::HeaderMap, name: &'static str, value: &str) -> Result<(), PluginError> {
    let value = http::HeaderValue::from_str(value).map_err(|_| {
        PluginError::new(
            "cors: a configured value is not a header value\n\
             help: remove control characters and non-ASCII text from the origins, methods and headers",
        )
    })?;
    headers.insert(http::header::HeaderName::from_static(name), value);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook(config: CorsConfig) -> Hook {
        Hook { config }
    }

    fn preflight(origin: &str, asking: &str) -> Request {
        http::Request::builder()
            .method(http::Method::OPTIONS)
            .header("origin", origin)
            .header("access-control-request-method", asking)
            .body(Vec::new())
            .expect("a request")
    }

    fn answered(hook: &Hook, mut request: Request) -> Option<Response> {
        match hook
            .before_dispatch(&mut request, &mut RequestState::new())
            .expect("ok")
        {
            Flow::Continue => None,
            Flow::Respond(response) => Some(*response),
        }
    }

    fn marked(hook: &Hook, request: &Request) -> http::HeaderMap {
        let (mut head, _) = Response::new(Body::from(Vec::new())).into_parts();
        hook.on_headers(
            request,
            &mut head,
            BodyShape::Buffered { len: 0 },
            &mut RequestState::new(),
        )
        .expect("ok");
        head.headers
    }

    #[test]
    fn a_preflight_is_answered_and_never_reaches_a_handler() {
        let response = answered(
            &hook(CorsConfig::default()),
            preflight("https://a.example", "POST"),
        )
        .expect("answered");
        assert_eq!(response.status(), http::StatusCode::NO_CONTENT);
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "*"
        );
        assert!(response
            .headers()
            .get("access-control-allow-methods")
            .unwrap()
            .to_str()
            .unwrap()
            .contains("POST"));
    }

    #[test]
    fn an_ordinary_options_is_not_eaten() {
        // An OPTIONS with no `Access-Control-Request-Method` is a client
        // asking what a resource supports, and it belongs to the handler.
        let request = http::Request::builder()
            .method(http::Method::OPTIONS)
            .header("origin", "https://a.example")
            .body(Vec::new())
            .expect("a request");
        assert!(answered(&hook(CorsConfig::default()), request).is_none());
    }

    #[test]
    fn an_origin_not_on_the_list_is_refused_by_omission() {
        let hook = hook(CorsConfig::from(["https://ok.example"]));
        let response =
            answered(&hook, preflight("https://evil.example", "POST")).expect("answered");
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_none(),
            "the browser must see no permission"
        );
        assert_eq!(
            response.headers().get(http::header::VARY).unwrap(),
            "Origin",
            "a refusal depends on the origin too, so a cache must not reuse it"
        );
    }

    #[test]
    fn an_allowed_origin_is_echoed_and_varied() {
        let hook = hook(CorsConfig::from(["https://ok.example"]));
        let request = http::Request::builder()
            .header("origin", "https://ok.example")
            .body(Vec::new())
            .expect("a request");
        let headers = marked(&hook, &request);
        assert_eq!(
            headers.get("access-control-allow-origin").unwrap(),
            "https://ok.example"
        );
        assert_eq!(headers.get(http::header::VARY).unwrap(), "Origin");
    }

    #[test]
    fn a_same_origin_response_is_left_alone() {
        // No `Origin`, no headers: otherwise every response in the
        // application varies by a header nobody sent.
        let request = http::Request::builder()
            .body(Vec::new())
            .expect("a request");
        let headers = marked(&hook(CorsConfig::default()), &request);
        assert!(headers.is_empty(), "{headers:?}");
    }

    #[test]
    fn a_wildcard_with_credentials_is_refused_at_startup() {
        // The browser refuses this combination. Refusing it here makes it
        // a line in a log instead of a bug that reproduces in Chrome.
        let bad = CorsConfig {
            credentials: true,
            ..CorsConfig::default()
        };
        let why = bad.check().expect_err("a browser refuses this");
        assert!(why.contains("credentials"), "{why}");
    }

    #[test]
    fn named_origins_with_credentials_are_fine() {
        let good = CorsConfig::from(["https://ok.example"]).with_credentials();
        good.check().expect("a named origin may carry credentials");
        let request = http::Request::builder()
            .header("origin", "https://ok.example")
            .body(Vec::new())
            .expect("a request");
        let headers = marked(&hook(good), &request);
        assert_eq!(
            headers.get("access-control-allow-credentials").unwrap(),
            "true"
        );
    }

    #[test]
    fn an_empty_origin_list_is_refused_at_startup() {
        // It allows nothing, which is almost never what somebody wrote by
        // hand and always what a mis-read configuration file produces.
        assert!(CorsConfig::from(Vec::<String>::new()).check().is_err());
    }

    #[test]
    fn exposed_headers_reach_the_real_response_and_not_the_preflight() {
        // `Access-Control-Expose-Headers` is about the response the script
        // reads, so putting it on the preflight does nothing.
        let config = CorsConfig::from(["https://ok.example"]).expose(["x-total-count"]);
        let hook = hook(config);
        let response = answered(&hook, preflight("https://ok.example", "GET")).expect("answered");
        assert!(response
            .headers()
            .get("access-control-expose-headers")
            .is_none());
        let request = http::Request::builder()
            .header("origin", "https://ok.example")
            .body(Vec::new())
            .expect("a request");
        assert_eq!(
            marked(&hook, &request)
                .get("access-control-expose-headers")
                .unwrap(),
            "x-total-count"
        );
    }

    #[test]
    fn max_age_is_seconds() {
        let config = CorsConfig::default().max_age(std::time::Duration::from_secs(600));
        let response =
            answered(&hook(config), preflight("https://a.example", "POST")).expect("answered");
        assert_eq!(
            response.headers().get("access-control-max-age").unwrap(),
            "600"
        );
    }

    #[test]
    fn a_handlers_own_vary_is_kept() {
        let hook = hook(CorsConfig::from(["https://ok.example"]));
        let request = http::Request::builder()
            .header("origin", "https://ok.example")
            .body(Vec::new())
            .expect("a request");
        let (mut head, _) = Response::new(Body::from(Vec::new())).into_parts();
        head.headers.insert(
            http::header::VARY,
            http::HeaderValue::from_static("Accept-Encoding"),
        );
        hook.on_headers(
            &request,
            &mut head,
            BodyShape::Buffered { len: 0 },
            &mut RequestState::new(),
        )
        .expect("ok");
        let varies: Vec<&str> = head
            .headers
            .get_all(http::header::VARY)
            .iter()
            .map(|v| v.to_str().expect("ascii"))
            .collect();
        assert_eq!(varies, ["Accept-Encoding, Origin"]);
    }

    #[test]
    fn a_preflight_names_origin_in_one_vary() {
        // The preflight sets `Vary: Origin`, and the response hook then
        // runs on the same response. It used to append a second line.
        let hook = hook(CorsConfig::from(["https://ok.example"]));
        let request = preflight("https://ok.example", "POST");
        let response = answered(&hook, preflight("https://ok.example", "POST")).expect("answered");
        let (mut head, _) = response.into_parts();
        hook.on_headers(
            &request,
            &mut head,
            BodyShape::Buffered { len: 0 },
            &mut RequestState::new(),
        )
        .expect("ok");
        let varies: Vec<&str> = head
            .headers
            .get_all(http::header::VARY)
            .iter()
            .map(|v| v.to_str().expect("ascii"))
            .collect();
        assert_eq!(varies, ["Origin"]);
    }

    /// A table of two pages: `/notes` answers GET, `/notes/{id}` GET and
    /// DELETE. Any other path has no route.
    fn routed(config: CorsConfig) -> Hook {
        hook(config.route_verbs(|path| {
            match path {
                "/notes" => Some(Allow::empty().with(Verb::Get).with(Verb::Head)),
                "/notes/7" => Some(
                    Allow::empty()
                        .with(Verb::Get)
                        .with(Verb::Head)
                        .with(Verb::Delete),
                ),
                _ => None,
            }
        }))
    }

    fn preflight_to(path: &str, asking: &str) -> Request {
        http::Request::builder()
            .method(http::Method::OPTIONS)
            .uri(path)
            .header("origin", "https://ok.example")
            .header("access-control-request-method", asking)
            .body(Vec::new())
            .expect("a request")
    }

    fn allowed(response: &Response) -> Option<&str> {
        response
            .headers()
            .get("access-control-allow-methods")
            .map(|v| v.to_str().expect("ascii"))
    }

    fn refused(response: &Response) -> bool {
        !response
            .headers()
            .keys()
            .any(|name| name.as_str().starts_with("access-control-allow-"))
    }

    fn wide() -> CorsConfig {
        CorsConfig::from(["https://ok.example"]).methods([
            http::Method::GET,
            http::Method::HEAD,
            http::Method::POST,
            http::Method::DELETE,
        ])
    }

    #[test]
    fn the_allowed_verbs_are_the_configured_ones_the_route_has() {
        let hook = routed(wide());
        let response = answered(&hook, preflight_to("/notes/7", "DELETE")).expect("answered");
        // POST is configured and the route lacks it; the route's OPTIONS
        // is not configured. Neither is listed.
        assert_eq!(allowed(&response), Some("GET, HEAD, DELETE"));
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .unwrap(),
            "https://ok.example"
        );
    }

    #[test]
    fn a_verb_the_route_lacks_gets_no_permission() {
        let hook = routed(wide());
        let response = answered(&hook, preflight_to("/notes", "DELETE")).expect("answered");
        assert!(refused(&response), "{:?}", response.headers());
        assert_eq!(response.status(), http::StatusCode::NO_CONTENT);
        assert_eq!(
            response.headers().get(http::header::VARY).unwrap(),
            "Origin",
            "a refusal still varies by origin"
        );
    }

    #[test]
    fn the_configuration_stays_the_ceiling() {
        // The route has DELETE, and the configuration does not allow it.
        let hook = routed(CorsConfig::from(["https://ok.example"]));
        let response = answered(&hook, preflight_to("/notes/7", "DELETE")).expect("answered");
        assert!(refused(&response), "{:?}", response.headers());
        let response = answered(&hook, preflight_to("/notes/7", "GET")).expect("answered");
        assert_eq!(allowed(&response), Some("GET, HEAD"));
    }

    #[test]
    fn the_response_hook_leaves_a_refused_preflight_without_permission() {
        // Both hooks run on a preflight. The response hook used to put
        // `Access-Control-Allow-Origin` back on the answer the preflight
        // hook refused.
        let hook = routed(wide());
        let request = preflight_to("/notes", "DELETE");
        let response = answered(&hook, preflight_to("/notes", "DELETE")).expect("answered");
        let (mut head, _) = response.into_parts();
        hook.on_headers(
            &request,
            &mut head,
            BodyShape::Buffered { len: 0 },
            &mut RequestState::new(),
        )
        .expect("ok");
        assert!(
            !head
                .headers
                .keys()
                .any(|name| name.as_str().starts_with("access-control-allow-")),
            "{:?}",
            head.headers
        );
    }

    #[test]
    fn a_path_with_no_route_gets_no_permission() {
        let hook = routed(wide());
        let response = answered(&hook, preflight_to("/nowhere", "GET")).expect("answered");
        assert!(refused(&response), "{:?}", response.headers());
    }

    #[test]
    fn without_the_function_every_path_answers_the_configured_list() {
        // The behaviour before `route_verbs`: the list, whatever the path
        // and whatever verb the preflight asks about.
        let hook = hook(wide());
        let response = answered(&hook, preflight_to("/nowhere", "PATCH")).expect("answered");
        assert_eq!(allowed(&response), Some("GET, HEAD, POST, DELETE"));
    }

    #[test]
    fn a_route_does_not_change_the_real_response() {
        // The decision covers the preflight. A real request from an allowed
        // origin is marked as before; the router answers its verb.
        let hook = routed(wide());
        let request = http::Request::builder()
            .method(http::Method::DELETE)
            .uri("/notes")
            .header("origin", "https://ok.example")
            .body(Vec::new())
            .expect("a request");
        assert_eq!(
            marked(&hook, &request)
                .get("access-control-allow-origin")
                .unwrap(),
            "https://ok.example"
        );
    }

    #[test]
    fn vary_is_merged_case_blind_and_a_star_stays_alone() {
        let mut headers = http::HeaderMap::new();
        headers.append(http::header::VARY, http::HeaderValue::from_static("origin"));
        headers.append(http::header::VARY, http::HeaderValue::from_static("Cookie"));
        add_vary(&mut headers, "Origin, DP-Request");
        let varies: Vec<_> = headers.get_all(http::header::VARY).iter().collect();
        assert_eq!(varies, ["origin, Cookie, DP-Request"]);

        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::VARY, http::HeaderValue::from_static("*"));
        add_vary(&mut headers, "Origin");
        assert_eq!(headers.get(http::header::VARY).unwrap(), "*");
    }
}
