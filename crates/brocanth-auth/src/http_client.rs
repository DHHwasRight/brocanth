//! The HTTP seam.
//!
//! Talking to a provider needs a client, and a plugin that hard-wires one
//! forces its choice of TLS stack, runtime and proxy behaviour onto every
//! application that wants OAuth. So the contract is a trait with two methods,
//! and the client is a feature.
//!
//! `reqwest` is the default because it is already in most trees, it is
//! maintained, and `rustls` keeps it off the system TLS stack — a plugin
//! should not make an application's build depend on OpenSSL headers. It is
//! still only a default: `default-features = false` removes it from the lock
//! file entirely and the application implements [`HttpClient`] over whatever
//! it already holds.

use std::collections::BTreeMap;

use async_trait::async_trait;

use crate::error::AuthError;

/// A request to a provider.
///
/// Deliberately not `http::Request`: the body is always either absent or a
/// form encoding, and a type that can express a streaming body would be a type
/// every implementation has to handle and no caller here produces.
#[derive(Clone, Debug)]
pub struct HttpRequest {
    /// `GET` or `POST`. Nothing here uses another verb.
    pub method: http::Method,
    /// Absolute URL.
    pub url: String,
    /// Headers, in insertion-independent order so a test can assert on them.
    pub headers: BTreeMap<String, String>,
    /// `application/x-www-form-urlencoded` pairs, when there is a body.
    pub form: Option<Vec<(String, String)>>,
}

impl HttpRequest {
    /// A `GET`.
    pub fn get(url: impl Into<String>) -> Self {
        Self {
            method: http::Method::GET,
            url: url.into(),
            headers: BTreeMap::new(),
            form: None,
        }
    }

    /// A `POST` with a form body.
    pub fn post_form(url: impl Into<String>, form: Vec<(String, String)>) -> Self {
        Self {
            method: http::Method::POST,
            url: url.into(),
            headers: BTreeMap::new(),
            form: Some(form),
        }
    }

    /// Adds a header.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }
}

/// What a provider answered.
#[derive(Clone, Debug)]
pub struct HttpResponse {
    /// Status.
    pub status: http::StatusCode,
    /// Body, as received.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Builds a response, for a test double.
    pub fn new(status: http::StatusCode, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }

    /// Builds a `200 OK` with a JSON body, for a test double.
    pub fn json(body: impl Into<Vec<u8>>) -> Self {
        Self::new(http::StatusCode::OK, body)
    }

    /// The body as UTF-8, lossily.
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }
}

/// Whatever performs the request.
#[async_trait]
pub trait HttpClient: Send + Sync + 'static {
    /// Performs one request and returns the whole response.
    ///
    /// Redirects are the implementation's business. Nothing in this crate
    /// expects to follow one: every endpoint here is a direct API call, and an
    /// OAuth token endpoint that redirects is a sign something is wrong rather
    /// than something to chase.
    ///
    /// # Errors
    /// Returns [`AuthError::Http`] for anything the transport could not do.
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, AuthError>;
}

/// A client that answers from a table, for tests.
///
/// It is not behind `#[cfg(test)]` because the applications that implement
/// [`AuthStore`] adapters need it too, and a test double that only exists
/// inside this crate's own test build is a test double nobody downstream can
/// use.
///
/// [`AuthStore`]: crate::storage::AuthStore
#[derive(Default)]
pub struct StubHttp {
    routes: std::sync::Mutex<Vec<(http::Method, String, HttpResponse)>>,
    seen: std::sync::Mutex<Vec<HttpRequest>>,
}

impl StubHttp {
    /// An empty stub.
    pub fn new() -> Self {
        Self::default()
    }

    /// Answers `response` for the next request whose method and URL prefix
    /// match.
    ///
    /// Matching on a prefix rather than the whole URL is deliberate: an
    /// authorization URL carries a fresh `state` and a fresh PKCE challenge
    /// every run, and a test that pinned the whole string would be asserting
    /// on randomness.
    #[must_use]
    pub fn on(
        self,
        method: http::Method,
        url_prefix: impl Into<String>,
        response: HttpResponse,
    ) -> Self {
        self.routes
            .lock()
            .expect("stub lock")
            .push((method, url_prefix.into(), response));
        self
    }

    /// Every request the stub was asked to perform, in order.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().expect("stub lock").clone()
    }
}

#[async_trait]
impl HttpClient for StubHttp {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, AuthError> {
        self.seen.lock().expect("stub lock").push(request.clone());
        let routes = self.routes.lock().expect("stub lock");
        routes
            .iter()
            .find(|(m, prefix, _)| *m == request.method && request.url.starts_with(prefix.as_str()))
            .map(|(_, _, r)| r.clone())
            .ok_or_else(|| {
                AuthError::Http(format!(
                    "no stubbed route for {} {}",
                    request.method, request.url
                ))
            })
    }
}

#[cfg(feature = "reqwest-client")]
mod reqwest_impl {
    use super::{HttpClient, HttpRequest, HttpResponse};
    use crate::error::AuthError;
    use async_trait::async_trait;

    /// [`HttpClient`] over `reqwest`.
    ///
    /// Behind the `reqwest-client` feature, which is on by default and which
    /// an application that already owns a client turns off.
    #[derive(Clone, Debug)]
    pub struct ReqwestClient(reqwest::Client);

    impl ReqwestClient {
        /// Wraps an existing client, so connection pooling, proxies and
        /// timeouts stay the application's decisions.
        pub fn new(client: reqwest::Client) -> Self {
            Self(client)
        }

        /// A client with a ten-second timeout.
        ///
        /// A provider that stops answering must not hold a request handler
        /// open: `reqwest`'s own default is no timeout at all, and inheriting
        /// that into an auth path is how one slow provider takes down a pool.
        ///
        /// # Errors
        /// Returns [`AuthError::Http`] when the client cannot be built.
        pub fn with_default_timeout() -> Result<Self, AuthError> {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .map(Self)
                .map_err(|e| AuthError::Http(e.to_string()))
        }
    }

    #[async_trait]
    impl HttpClient for ReqwestClient {
        async fn send(&self, request: HttpRequest) -> Result<HttpResponse, AuthError> {
            let mut builder = self.0.request(request.method, &request.url);
            for (name, value) in &request.headers {
                builder = builder.header(name, value);
            }
            if let Some(form) = &request.form {
                builder = builder.form(form);
            }
            let response = builder
                .send()
                .await
                .map_err(|e| AuthError::Http(e.to_string()))?;
            let status = response.status();
            let body = response
                .bytes()
                .await
                .map_err(|e| AuthError::Http(e.to_string()))?
                .to_vec();
            Ok(HttpResponse { status, body })
        }
    }
}

#[cfg(feature = "reqwest-client")]
pub use reqwest_impl::ReqwestClient;
