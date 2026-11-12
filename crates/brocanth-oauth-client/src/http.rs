//! The HTTP client the token requests go through.
//!
//! `oauth2` sends a request through any type that implements its
//! [`oauth2::AsyncHttpClient`]. Its own implementation is for reqwest 0.12;
//! the workspace links reqwest 0.13. So [`Http`] implements the trait over
//! 0.13, in forty lines, and the tree keeps one reqwest.
//!
//! The client follows no redirect: a token endpoint that answers 3xx is an
//! error, not a hop that carries the client secret elsewhere. A response
//! larger than one mebibyte is refused before it is all read.
//!
//! `brocanth-auth`'s `ReqwestClient` sends the sign-in requests through
//! this type too, so both crates share one reqwest and one setup.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use oauth2::{HttpRequest, HttpResponse};

use crate::Error;

/// The largest token response read, in bytes.
const MAX_BODY: usize = 1 << 20;

/// A reqwest client for token requests.
#[derive(Clone, Debug)]
pub struct Http {
    client: reqwest::Client,
}

impl Http {
    /// A client with this timeout, no redirects and no system TLS stack.
    ///
    /// # Errors
    /// [`Error::Config`] when the TLS backend does not start.
    pub fn new(timeout: Duration) -> Result<Self, Error> {
        reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("brocanth-oauth-client/", env!("CARGO_PKG_VERSION")))
            .build()
            .map(|client| Self { client })
            .map_err(|e| Error::Config(format!("building the HTTP client: {e}")))
    }

    /// A client over one the application built. Turn its redirects off.
    pub fn from_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

/// A token request that did not complete. It never holds the request
/// body, which carries the secret.
#[derive(Debug)]
pub struct HttpError(pub String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HttpError {}

impl<'c> oauth2::AsyncHttpClient<'c> for Http {
    type Error = HttpError;
    type Future = Pin<Box<dyn Future<Output = Result<HttpResponse, HttpError>> + Send + 'c>>;

    fn call(&'c self, request: HttpRequest) -> Self::Future {
        Box::pin(async move {
            let request = reqwest::Request::try_from(request)
                .map_err(|e| HttpError(format!("building the request: {}", e.without_url())))?;
            let mut response = self
                .client
                .execute(request)
                .await
                .map_err(|e| HttpError(e.without_url().to_string()))?;
            let mut builder = http::Response::builder().status(response.status());
            for (name, value) in response.headers() {
                builder = builder.header(name, value);
            }
            let mut body = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|e| HttpError(e.without_url().to_string()))?
            {
                if body.len() + chunk.len() > MAX_BODY {
                    return Err(HttpError(format!(
                        "the answer is larger than {MAX_BODY} bytes"
                    )));
                }
                body.extend_from_slice(&chunk);
            }
            builder.body(body).map_err(|e| HttpError(e.to_string()))
        })
    }
}
