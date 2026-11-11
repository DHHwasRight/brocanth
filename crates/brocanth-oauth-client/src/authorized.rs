//! [`Authorized`]: an HTTP client that sends the token, like golang's
//! `Config.Client`.
//!
//! Each request gets `Authorization: Bearer <token>` from the token source,
//! so an expired token is refreshed before it is sent. When the API still
//! answers 401, the token was revoked or rotated early: the client forces
//! one refresh and sends the request once more. A second 401 goes back to
//! the caller. A request whose body is a stream cannot be sent twice, so it
//! gets no retry.

use http::header::{HeaderValue, AUTHORIZATION};

use crate::{Error, OauthClient, Token};

#[derive(Clone, Debug)]
enum Who {
    Account { account: String, provider: String },
    App { provider: String },
}

/// An HTTP client that calls an API as an account or as the application.
#[derive(Clone, Debug)]
pub struct Authorized {
    client: OauthClient,
    who: Who,
}

impl Authorized {
    pub(crate) fn for_account(client: OauthClient, account: &str, provider: &str) -> Self {
        Self {
            client,
            who: Who::Account {
                account: account.to_string(),
                provider: provider.to_string(),
            },
        }
    }

    pub(crate) fn for_app(client: OauthClient, provider: &str) -> Self {
        Self {
            client,
            who: Who::App {
                provider: provider.to_string(),
            },
        }
    }

    /// A request to build on. Send it with [`Authorized::send`].
    pub fn request(&self, method: reqwest::Method, url: &str) -> reqwest::RequestBuilder {
        self.client.api().request(method, url)
    }

    /// A GET to build on.
    pub fn get(&self, url: &str) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::GET, url)
    }

    /// A POST to build on.
    pub fn post(&self, url: &str) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::POST, url)
    }

    /// Sends the request with the token, and once more after a 401 with a
    /// refreshed one.
    ///
    /// # Errors
    /// What [`OauthClient::token`] or [`OauthClient::app_token`] returns,
    /// or [`Error::Http`] when the request does not build or send. A 4xx or
    /// 5xx answer is not an error: it is the response.
    pub async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response, Error> {
        let request = request
            .build()
            .map_err(|e| Error::Http(e.without_url().to_string()))?;
        let again = request.try_clone();
        let token = self.token().await?;
        let response = self.execute(request, &token).await?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let Some(again) = again else {
            return Ok(response);
        };
        let fresh = self.refreshed(&token).await?;
        self.execute(again, &fresh).await
    }

    async fn execute(
        &self,
        mut request: reqwest::Request,
        token: &Token,
    ) -> Result<reqwest::Response, Error> {
        let mut value = HeaderValue::from_str(&token.authorization())
            .map_err(|_| Error::Http("the access token is not a header value".into()))?;
        // Marked sensitive: an HTTP log layer prints it as `Sensitive`.
        value.set_sensitive(true);
        request.headers_mut().insert(AUTHORIZATION, value);
        self.client
            .api()
            .execute(request)
            .await
            .map_err(|e| Error::Http(e.without_url().to_string()))
    }

    async fn token(&self) -> Result<Token, Error> {
        match &self.who {
            Who::Account { account, provider } => self.client.token(account, provider).await,
            Who::App { provider } => self.client.app_token(provider).await,
        }
    }

    async fn refreshed(&self, stale: &Token) -> Result<Token, Error> {
        match &self.who {
            Who::Account { account, provider } => {
                self.client.refresh(account, provider, stale).await
            }
            Who::App { provider } => self.client.app_refresh(provider, stale).await,
        }
    }
}
