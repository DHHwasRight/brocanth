//! One host, so cookies and caches agree.
//!
//! `example.com` and `www.example.com` are two origins. A session cookie
//! set on one is not sent to the other, a cache stores both, and every
//! absolute URL in a page points at whichever one the visitor happened to
//! type. Gorilla's `CanonicalHost` redirects to the one you picked and
//! keeps the path.
//!
//! Three things this does that a hand-written redirect usually forgets:
//!
//! 1. **It keeps the query string.** A redirect that drops `?page=2` is a
//!    redirect that loses the visitor's place.
//! 2. **It leaves a safe-method request alone where the code says to.** A
//!    301 on a `POST` is allowed to turn into a `GET` by the letter of the
//!    specification, and browsers did that for years, so a form post to
//!    the wrong host would silently lose its body. Use 307 or 308 to keep
//!    the verb, and this hook says so if you do not.
//! 3. **It does nothing without a `Host`.** An HTTP/1.0 request with no
//!    host has nothing to compare, and answering a redirect to a client
//!    that cannot say where it is going is a loop.

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    Flow, HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState, Response,
};

use crate::text;

/// The host to send everybody to.
#[derive(Clone, Debug)]
pub struct CanonicalHostConfig {
    /// The scheme and host, as they should appear: `https://example.com`.
    /// A trailing slash is trimmed.
    pub to: String,
    /// The redirect status. 308 keeps the verb and the body; 301 is what
    /// Gorilla's examples use and what search engines prefer.
    pub status: http::StatusCode,
}

impl CanonicalHostConfig {
    /// A permanent redirect that keeps the verb: 308.
    ///
    /// The safe default, and not the common one. 301 is older and better
    /// understood by crawlers, and it is also the one that may turn a
    /// `POST` into a `GET`.
    pub fn to(host: impl Into<String>) -> Self {
        Self {
            to: host.into(),
            status: http::StatusCode::PERMANENT_REDIRECT,
        }
    }

    /// Answer with this status instead.
    pub fn with_status(mut self, status: http::StatusCode) -> Self {
        self.status = status;
        self
    }
}

/// Redirects every request to one host.
pub struct CanonicalHost {
    config: CanonicalHostConfig,
}

impl Plugin for CanonicalHost {
    type Config = CanonicalHostConfig;
    type Provides = NoContext;
    const NAME: &'static str = "canonical-host";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for CanonicalHost {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        let (_scheme, host) = split(&self.config.to);
        registrar.request(
            "redirect",
            brocanth::plugin_order::Order::new(),
            Hook {
                base: self.config.to.trim_end_matches('/').to_string(),
                host,
                status: self.config.status,
            },
        );
        Ok(())
    }
}

struct Hook {
    /// `https://example.com`, with no trailing slash.
    base: String,
    /// `example.com`, which is what a `Host` header holds.
    host: String,
    status: http::StatusCode,
}

impl RequestHook for Hook {
    fn before_dispatch(
        &self,
        request: &mut Request,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let Some(sent) = host_of(request) else {
            // Nothing to compare, and nowhere to send a client that could
            // not say where it was.
            return Ok(Flow::Continue);
        };
        if sent.eq_ignore_ascii_case(&self.host) {
            return Ok(Flow::Continue);
        }
        let path = request
            .uri()
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/");
        let target = format!("{}{path}", self.base);
        let mut response = Response::new(Body::from(Vec::new()));
        *response.status_mut() = self.status;
        response.headers_mut().insert(
            http::header::LOCATION,
            http::HeaderValue::from_str(&target).map_err(|_| {
                PluginError::new(format!("canonical-host: {}", "the target is not a URL"))
            })?,
        );
        Ok(Flow::respond(response))
    }
}

/// The host the client asked for: the URI's authority if it has one, the
/// `Host` header otherwise.
fn host_of(request: &Request) -> Option<String> {
    if let Some(authority) = request.uri().authority() {
        return Some(authority.host().to_ascii_lowercase());
    }
    let raw = text(request.headers(), http::header::HOST)?;
    // `example.com:8443` is the same host as `example.com`. The port is
    // not compared, because a canonical host that redirected on a port
    // would loop behind any proxy that terminates TLS.
    let host = raw.rsplit_once(':').map_or(raw, |(host, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) {
            host
        } else {
            raw
        }
    });
    Some(host.trim().to_ascii_lowercase())
}

/// Splits `https://example.com` into its scheme and its host.
fn split(base: &str) -> (String, String) {
    match base.split_once("://") {
        Some((scheme, rest)) => (
            scheme.to_ascii_lowercase(),
            rest.trim_end_matches('/')
                .split('/')
                .next()
                .unwrap_or(rest)
                .to_ascii_lowercase(),
        ),
        None => (
            String::new(),
            base.trim_end_matches('/').to_ascii_lowercase(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook() -> Hook {
        let config = CanonicalHostConfig::to("https://example.com");
        let (_scheme, host) = split(&config.to);
        Hook {
            base: config.to.clone(),
            host,
            status: config.status,
        }
    }

    fn get(uri: &str, host: Option<&str>) -> Request {
        let mut builder = http::Request::builder().uri(uri);
        if let Some(host) = host {
            builder = builder.header("host", host);
        }
        builder.body(Vec::new()).expect("a request")
    }

    fn redirect(request: &mut Request) -> Option<(http::StatusCode, String)> {
        match hook()
            .before_dispatch(request, &mut RequestState::new())
            .expect("ok")
        {
            Flow::Continue => None,
            Flow::Respond(response) => Some((
                response.status(),
                response
                    .headers()
                    .get(http::header::LOCATION)
                    .expect("a location")
                    .to_str()
                    .expect("ascii")
                    .to_string(),
            )),
        }
    }

    #[test]
    fn the_wrong_host_is_sent_to_the_right_one_with_its_path() {
        let (status, to) = redirect(&mut get("/posts/7", Some("www.example.com"))).expect("moved");
        assert_eq!(status, http::StatusCode::PERMANENT_REDIRECT);
        assert_eq!(to, "https://example.com/posts/7");
    }

    #[test]
    fn the_query_string_survives() {
        // A redirect that drops `?page=2` loses the visitor's place.
        let (_, to) =
            redirect(&mut get("/posts?page=2&q=a+b", Some("old.example"))).expect("moved");
        assert_eq!(to, "https://example.com/posts?page=2&q=a+b");
    }

    #[test]
    fn the_right_host_is_left_alone() {
        assert!(redirect(&mut get("/x", Some("example.com"))).is_none());
        assert!(
            redirect(&mut get("/x", Some("EXAMPLE.COM"))).is_none(),
            "a host is case-insensitive"
        );
    }

    #[test]
    fn a_port_is_not_part_of_the_comparison() {
        // Behind a TLS terminator the Host arrives with the inside port.
        // Comparing it would redirect every request, to itself, forever.
        assert!(redirect(&mut get("/x", Some("example.com:8443"))).is_none());
    }

    #[test]
    fn no_host_is_no_redirect() {
        assert!(redirect(&mut get("/x", None)).is_none());
    }

    #[test]
    fn an_absolute_uri_is_read_before_the_header() {
        // A proxy may pass the absolute form. The header can disagree, and
        // the request line is the request.
        assert!(redirect(&mut get("https://example.com/x", Some("www.example.com"))).is_none());
    }

    #[test]
    fn the_default_keeps_the_verb() {
        // 301 may legally turn a POST into a GET, and browsers did for
        // years, so a form posted to the wrong host would lose its body.
        assert_eq!(
            CanonicalHostConfig::to("https://example.com").status,
            http::StatusCode::PERMANENT_REDIRECT
        );
    }

    #[test]
    fn a_status_can_still_be_chosen() {
        let config = CanonicalHostConfig::to("https://example.com")
            .with_status(http::StatusCode::MOVED_PERMANENTLY);
        assert_eq!(config.status, http::StatusCode::MOVED_PERMANENTLY);
    }

    #[test]
    fn a_base_is_split_into_a_scheme_and_a_host() {
        assert_eq!(
            split("https://example.com/"),
            ("https".to_string(), "example.com".to_string())
        );
        assert_eq!(
            split("example.com"),
            (String::new(), "example.com".to_string())
        );
    }
}
