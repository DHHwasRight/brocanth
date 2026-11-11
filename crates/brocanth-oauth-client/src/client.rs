//! [`OauthClient`]: the connect flow, the token source, and the way to an
//! authenticated HTTP client.
//!
//! # The token source
//!
//! [`OauthClient::token`] is golang's `TokenSource`. It reads the row, opens
//! the sealed token, and returns it while it is valid. Ten seconds before
//! it runs out, it refreshes first, writes the new token, and returns that.
//!
//! **One refresh per key.** Refreshes take an async lock per
//! `(account, provider)`. The first caller refreshes; the others wait, read
//! the row again and find the new token. A provider that rotates refresh
//! tokens (each refresh returns a new one and voids the old) would
//! otherwise see the void token on the second call and answer
//! `invalid_grant`, and the person would have to connect again for nothing.
//!
//! The lock is per process. Two instances can still race. So an
//! `invalid_grant` is checked once more against the row: when another
//! instance stored a new refresh token in the meantime, its token is
//! returned instead of [`Error::Reconnect`].
//!
//! **The rotated refresh token is kept.** When the answer has a new refresh
//! token, it replaces the old one in the same write as the access token.
//! When it has none, the old one stays, as RFC 6749 §6 says.

use std::collections::{BTreeMap, HashMap};
use std::hash::Hash;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use oauth2::basic::{BasicClient, BasicErrorResponse, BasicErrorResponseType, BasicTokenType};
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, RequestTokenError,
    Scope, TokenResponse, TokenUrl,
};
use serde::{Deserialize, Serialize};

use crate::clock::{Clock, SystemClock};
use crate::http::{Http, HttpError};
use crate::key::{ct_eq, random_text, Key};
use crate::provider::{AuthStyle, Provider};
use crate::store::{Connection, StoredToken, TokenStore};
use crate::{Authorized, Error, FlowError};

/// How long a started connection may take, in seconds.
pub(crate) const FLOW_SECONDS: i64 = 600;

/// The first client installed in this process, for the admin tab.
static INSTALLED: OnceLock<OauthClient> = OnceLock::new();

/// An access token, and what came with it. Its `Debug` shows no token.
#[derive(Clone, PartialEq, Eq)]
pub struct Token {
    access_token: String,
    refresh_token: Option<String>,
    /// `Bearer`, most of the time.
    pub token_type: String,
    /// The scopes granted.
    pub scopes: Vec<String>,
    /// When the access token runs out, in seconds since 1970.
    pub expires_at: Option<i64>,
}

impl std::fmt::Debug for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("token_type", &self.token_type)
            .field("scopes", &self.scopes)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

impl Token {
    /// The access token. Send it; never log it.
    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    /// The refresh token, when the provider gave one.
    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    /// The `Authorization` header value: `Bearer <token>`.
    pub fn authorization(&self) -> String {
        format!("Bearer {}", self.access_token)
    }

    /// Whether it runs out within `delta` seconds of `now`. A token with no
    /// expiry never does.
    pub fn expires_within(&self, now: i64, delta: i64) -> bool {
        self.expires_at.is_some_and(|at| now + delta >= at)
    }

    fn from_response(
        response: &oauth2::basic::BasicTokenResponse,
        now: i64,
        previous: Option<&Token>,
        requested: &[String],
    ) -> Self {
        let scopes = response
            .scopes()
            .map(|s| s.iter().map(|s| s.to_string()).collect())
            .or_else(|| previous.map(|p| p.scopes.clone()))
            .unwrap_or_else(|| requested.to_vec());
        Self {
            access_token: response.access_token().secret().clone(),
            refresh_token: response
                .refresh_token()
                .map(|r| r.secret().clone())
                .or_else(|| previous.and_then(|p| p.refresh_token.clone())),
            token_type: match response.token_type() {
                BasicTokenType::Bearer => "Bearer".to_string(),
                BasicTokenType::Mac => "MAC".to_string(),
                BasicTokenType::Extension(other) => other.clone(),
            },
            scopes,
            expires_at: response
                .expires_in()
                .map(|d| now.saturating_add(i64::try_from(d.as_secs()).unwrap_or(i64::MAX))),
        }
    }
}

/// A started connection: where to send the browser, and the cookie value
/// that remembers the flow.
#[derive(Clone)]
pub struct Started {
    /// The provider's consent URL, with `state` and the PKCE challenge.
    pub url: String,
    /// The sealed flow: account, provider, state, PKCE verifier, return
    /// path and time. Put it in an `HttpOnly` cookie.
    pub cookie: String,
}

impl std::fmt::Debug for Started {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Started")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// A finished connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Completed {
    /// The account the token now belongs to.
    pub account: String,
    /// The provider id.
    pub provider: String,
    /// The local path to go back to, when the flow named a valid one.
    pub return_to: Option<String>,
}

/// What the flow cookie carries, sealed.
#[derive(Serialize, Deserialize)]
struct Flow {
    /// Provider id.
    p: String,
    /// Account.
    a: String,
    /// State.
    s: String,
    /// PKCE verifier.
    v: String,
    /// Return path.
    r: Option<String>,
    /// When it started.
    t: i64,
}

/// Builds an [`OauthClient`].
pub struct Builder {
    key: Key,
    store: Option<Arc<dyn TokenStore>>,
    providers: Vec<Provider>,
    callback_base: String,
    http: Option<Http>,
    api: Option<reqwest::Client>,
    clock: Arc<dyn Clock>,
    allow_http: bool,
    expiry_delta: i64,
}

impl Builder {
    /// Where the tokens live.
    #[must_use]
    pub fn store(mut self, store: Arc<dyn TokenStore>) -> Self {
        self.store = Some(store);
        self
    }

    /// Adds a provider.
    #[must_use]
    pub fn provider(mut self, provider: Provider) -> Self {
        self.providers.push(provider);
        self
    }

    /// The scheme, host and path the connect routes answer at:
    /// `https://example.com/connections`. The redirect URI of provider `id`
    /// is `<callback_base>/<id>/callback`; register it at the provider.
    #[must_use]
    pub fn callback_base(mut self, url: impl Into<String>) -> Self {
        self.callback_base = url.into().trim_end_matches('/').to_string();
        self
    }

    /// The client for token requests. The default has a 10 second timeout
    /// and follows no redirect.
    #[must_use]
    pub fn http(mut self, http: Http) -> Self {
        self.http = Some(http);
        self
    }

    /// The client for API requests through [`Authorized`]. The default has
    /// a 30 second timeout.
    #[must_use]
    pub fn api_client(mut self, client: reqwest::Client) -> Self {
        self.api = Some(client);
        self
    }

    /// The clock. A test moves it.
    #[must_use]
    pub fn clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// Accepts `http://` provider URLs. For development and for a fake
    /// provider in a test; never in production.
    #[must_use]
    pub fn allow_http(mut self, allow: bool) -> Self {
        self.allow_http = allow;
        self
    }

    /// How long before its expiry a token counts as expired. Ten seconds,
    /// as in golang.org/x/oauth2.
    #[must_use]
    pub fn expiry_delta(mut self, delta: Duration) -> Self {
        self.expiry_delta = i64::try_from(delta.as_secs()).unwrap_or(i64::MAX);
        self
    }

    /// The client.
    ///
    /// # Errors
    /// [`Error::Config`] for no store, a provider URL that is not https, two
    /// providers with one id, or a callback base that is not a URL.
    pub fn build(self) -> Result<OauthClient, Error> {
        let Some(store) = self.store else {
            return Err(Error::Config(
                "no token store\nhelp: call `.store(Arc::new(MemoryTokenStore::new()))` for a \
                 test, or `.store(Arc::new(PgTokenStore::new(db)))`"
                    .into(),
            ));
        };
        let base = url::Url::parse(&self.callback_base).map_err(|e| {
            Error::Config(format!(
                "the callback base `{}` is not a URL ({e})\n\
                 help: set it to the scheme, host and mount point: https://example.com/connections",
                self.callback_base
            ))
        })?;
        if !matches!(base.scheme(), "https" | "http") {
            return Err(Error::Config(format!(
                "the callback base `{}` is not http or https\nhelp: use https://<your host><mount_at>",
                self.callback_base
            )));
        }
        let mut providers = BTreeMap::new();
        for provider in self.providers {
            provider.check(self.allow_http)?;
            let id = provider.id.clone();
            if providers.insert(id.clone(), provider).is_some() {
                return Err(Error::Config(format!(
                    "two providers have the id `{id}`\nhelp: give each one its own id"
                )));
            }
        }
        let http = match self.http {
            Some(http) => http,
            None => Http::new(Duration::from_secs(10))?,
        };
        let api = match self.api {
            Some(api) => api,
            None => reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|e| Error::Config(format!("building the API client: {e}")))?,
        };
        Ok(OauthClient {
            inner: Arc::new(Inner {
                key: self.key,
                store,
                providers,
                callback_base: self.callback_base,
                http,
                api,
                clock: self.clock,
                expiry_delta: self.expiry_delta,
                flights: Mutex::new(HashMap::new()),
                app_tokens: Mutex::new(HashMap::new()),
                app_flights: Mutex::new(HashMap::new()),
            }),
        })
    }
}

/// The OAuth client: providers, a store and a key.
///
/// Cheap to clone: the clones share the store, the locks and the cached
/// application tokens.
#[derive(Clone)]
pub struct OauthClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for OauthClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OauthClient")
            .field(
                "providers",
                &self.inner.providers.keys().collect::<Vec<_>>(),
            )
            .finish_non_exhaustive()
    }
}

impl brocanth::plugin_context::ContextItem for OauthClient {}

type Flights<K> = Mutex<HashMap<K, Arc<tokio::sync::Mutex<()>>>>;

struct Inner {
    key: Key,
    store: Arc<dyn TokenStore>,
    providers: BTreeMap<String, Provider>,
    callback_base: String,
    http: Http,
    api: reqwest::Client,
    clock: Arc<dyn Clock>,
    expiry_delta: i64,
    flights: Flights<(String, String)>,
    app_tokens: Mutex<HashMap<String, Token>>,
    app_flights: Flights<String>,
}

type TokenClient =
    BasicClient<EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

impl OauthClient {
    /// A builder with this key and no store, no provider, and
    /// `http://localhost:3000/connections` as the callback base.
    pub fn builder(key: Key) -> Builder {
        Builder {
            key,
            store: None,
            providers: Vec::new(),
            callback_base: "http://localhost:3000/connections".to_string(),
            http: None,
            api: None,
            clock: Arc::new(SystemClock),
            allow_http: false,
            expiry_delta: 10,
        }
    }

    /// Makes this the client [`OauthClient::installed`] answers. The first
    /// call in a process wins.
    pub fn install(&self) {
        let _ = INSTALLED.set(self.clone());
    }

    /// The client the plugin installed, for code that runs outside a
    /// request: the admin tab, a job.
    pub fn installed() -> Option<OauthClient> {
        INSTALLED.get().cloned()
    }

    /// The providers, by id.
    pub fn providers(&self) -> impl Iterator<Item = &Provider> {
        self.inner.providers.values()
    }

    /// One provider.
    ///
    /// # Errors
    /// [`Error::UnknownProvider`].
    pub fn provider(&self, id: &str) -> Result<&Provider, Error> {
        self.inner
            .providers
            .get(id)
            .ok_or_else(|| Error::UnknownProvider(id.to_string()))
    }

    /// The redirect URI to register at provider `id`.
    pub fn redirect_uri(&self, id: &str) -> String {
        format!("{}/{id}/callback", self.inner.callback_base)
    }

    /// The store.
    pub fn store(&self) -> &Arc<dyn TokenStore> {
        &self.inner.store
    }

    fn now(&self) -> i64 {
        self.inner.clock.now()
    }

    // ---- the connect flow --------------------------------------------------

    /// Starts a connection for `account`: a fresh `state`, a PKCE pair, and
    /// the provider's consent URL. `return_to` is kept only when it is a
    /// path on this site.
    ///
    /// # Errors
    /// [`Error::UnknownProvider`], or [`Error::Config`] for a URL that does
    /// not parse.
    pub fn begin(
        &self,
        provider: &str,
        account: &str,
        return_to: Option<&str>,
    ) -> Result<Started, Error> {
        let p = self.provider(provider)?;
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let state = random_text(32);
        let auth_url = AuthUrl::new(p.endpoint.auth_url.clone())
            .map_err(|e| Error::Config(format!("`{}` auth_url: {e}", p.id)))?;
        let client = BasicClient::new(ClientId::new(p.client_id.clone()))
            .set_auth_uri(auth_url)
            .set_redirect_uri(self.redirect_url(p)?);
        let flow_state = state.clone();
        let mut request = client
            .authorize_url(move || CsrfToken::new(flow_state))
            .set_pkce_challenge(challenge)
            .add_scopes(p.scopes.iter().cloned().map(Scope::new));
        for (name, value) in &p.endpoint.auth_params {
            request = request.add_extra_param(name.clone(), value.clone());
        }
        let (url, _) = request.url();
        let flow = Flow {
            p: p.id.clone(),
            a: account.to_string(),
            s: state,
            v: verifier.secret().clone(),
            r: return_to.and_then(local_path),
            t: self.now(),
        };
        let json = serde_json::to_vec(&flow).expect("a flow serialises");
        Ok(Started {
            url: url.to_string(),
            cookie: self.inner.key.seal_state(&json),
        })
    }

    /// Finishes a connection: checks the flow cookie, the time and the
    /// `state` (in constant time), exchanges the code with the PKCE
    /// verifier, and stores the token, sealed, for the account that
    /// started.
    ///
    /// `query` is the callback's query pairs. `signed_in` is who is signed
    /// in now, when the callback request carries a session: it must be the
    /// account that started.
    ///
    /// # Errors
    /// [`Error::Flow`] for a refused flow, with a message a visitor may
    /// read. [`Error::Provider`] when the exchange fails. [`Error::Store`].
    pub async fn complete(
        &self,
        provider: &str,
        cookie: Option<&str>,
        query: &[(String, String)],
        signed_in: Option<&str>,
    ) -> Result<Completed, Error> {
        let p = self.provider(provider)?;
        let sealed = cookie.ok_or(FlowError::NotStarted)?;
        let flow: Flow = self
            .inner
            .key
            .open_state(sealed)
            .and_then(|json| serde_json::from_slice(&json).ok())
            .ok_or(FlowError::Tampered)?;
        let now = self.now();
        if now - flow.t > FLOW_SECONDS || flow.t - now > 60 {
            return Err(FlowError::Expired.into());
        }
        if flow.p != p.id {
            return Err(FlowError::WrongProvider.into());
        }
        let param = |name: &str| {
            query
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        if !ct_eq(param("state").unwrap_or(""), &flow.s) {
            return Err(FlowError::StateMismatch.into());
        }
        if let Some(error) = param("error") {
            return Err(FlowError::Denied(clean(error)).into());
        }
        if signed_in.is_some_and(|who| who != flow.a) {
            return Err(FlowError::WrongAccount.into());
        }
        let code = param("code")
            .filter(|c| !c.is_empty())
            .ok_or(FlowError::NoCode)?;
        let response = self
            .token_client(p)?
            .exchange_code(AuthorizationCode::new(code.to_string()))
            .set_pkce_verifier(PkceCodeVerifier::new(flow.v))
            .request_async(&self.inner.http)
            .await
            .map_err(|e| provider_error(p, &e))?;
        let token = Token::from_response(&response, now, None, &p.scopes);
        self.save(&flow.a, p, &token, now).await?;
        Ok(Completed {
            account: flow.a,
            provider: p.id.clone(),
            return_to: flow.r,
        })
    }

    // ---- the token source --------------------------------------------------

    /// The account's token for `provider`, refreshed first when it runs out
    /// within the expiry delta.
    ///
    /// # Errors
    /// [`Error::NotConnected`] with no row. [`Error::Reconnect`] when the
    /// token expired and the refresh token is gone or refused.
    /// [`Error::Provider`] when the refresh failed for another reason.
    pub async fn token(&self, account: &str, provider: &str) -> Result<Token, Error> {
        let p = self.provider(provider)?;
        let (current, _) = self.load(account, p).await?;
        if !current.expires_within(self.now(), self.inner.expiry_delta) {
            return Ok(current);
        }
        self.refresh_from(account, p, Some(current.access_token))
            .await
    }

    /// Refreshes now, unless another caller already replaced `stale`. For a
    /// 401 from the API: the token looked valid, and the provider says it
    /// is not.
    ///
    /// # Errors
    /// As [`OauthClient::token`].
    pub async fn refresh(
        &self,
        account: &str,
        provider: &str,
        stale: &Token,
    ) -> Result<Token, Error> {
        let p = self.provider(provider)?;
        self.refresh_from(account, p, Some(stale.access_token.clone()))
            .await
    }

    async fn refresh_from(
        &self,
        account: &str,
        p: &Provider,
        stale: Option<String>,
    ) -> Result<Token, Error> {
        let key = (account.to_string(), p.id.clone());
        let lock = flight(&self.inner.flights, &key);
        let result = {
            let _held = lock.lock().await;
            self.refresh_held(account, p, stale).await
        };
        land(&self.inner.flights, &key, lock);
        result
    }

    async fn refresh_held(
        &self,
        account: &str,
        p: &Provider,
        stale: Option<String>,
    ) -> Result<Token, Error> {
        let (current, connected_at) = self.load(account, p).await?;
        let now = self.now();
        let replaced = stale.is_some_and(|s| !ct_eq(&s, &current.access_token));
        if replaced && !current.expires_within(now, self.inner.expiry_delta) {
            // Another caller refreshed while this one waited.
            return Ok(current);
        }
        let Some(refresh) = current.refresh_token.clone() else {
            return Err(Error::Reconnect {
                account: account.to_string(),
                provider: p.id.clone(),
                reason: "the access token ran out and the provider gave no refresh token".into(),
            });
        };
        let answer = self
            .token_client(p)?
            .exchange_refresh_token(&RefreshToken::new(refresh.clone()))
            .request_async(&self.inner.http)
            .await;
        match answer {
            Ok(response) => {
                let token = Token::from_response(&response, now, Some(&current), &p.scopes);
                self.save(account, p, &token, connected_at).await?;
                Ok(token)
            }
            Err(RequestTokenError::ServerResponse(e))
                if *e.error() == BasicErrorResponseType::InvalidGrant =>
            {
                // Another instance may have rotated the refresh token first.
                let (again, _) = self.load(account, p).await?;
                if again.refresh_token.as_deref() != Some(refresh.as_str())
                    && !again.expires_within(self.now(), self.inner.expiry_delta)
                {
                    return Ok(again);
                }
                Err(Error::Reconnect {
                    account: account.to_string(),
                    provider: p.id.clone(),
                    reason: format!("the provider refused the refresh token ({})", describe(&e)),
                })
            }
            Err(e) => Err(provider_error(p, &e)),
        }
    }

    // ---- client credentials ------------------------------------------------

    /// The application's own token for `provider` (client credentials),
    /// cached until it runs out. Concurrent callers make one request.
    ///
    /// # Errors
    /// [`Error::Provider`] when the token request fails.
    pub async fn app_token(&self, provider: &str) -> Result<Token, Error> {
        let p = self.provider(provider)?;
        if let Some(token) = self.cached_app_token(&p.id) {
            return Ok(token);
        }
        let lock = flight(&self.inner.app_flights, &p.id);
        let result = {
            let _held = lock.lock().await;
            match self.cached_app_token(&p.id) {
                Some(token) => Ok(token),
                None => self.fetch_app_token(p).await,
            }
        };
        land(&self.inner.app_flights, &p.id, lock);
        result
    }

    /// Drops the cached application token when it is still `stale`, then
    /// fetches one. For a 401 from the API.
    ///
    /// # Errors
    /// As [`OauthClient::app_token`].
    pub async fn app_refresh(&self, provider: &str, stale: &Token) -> Result<Token, Error> {
        {
            let mut cache = lock(&self.inner.app_tokens);
            if cache
                .get(provider)
                .is_some_and(|t| ct_eq(&t.access_token, &stale.access_token))
            {
                cache.remove(provider);
            }
        }
        self.app_token(provider).await
    }

    fn cached_app_token(&self, provider: &str) -> Option<Token> {
        let now = self.now();
        lock(&self.inner.app_tokens)
            .get(provider)
            .filter(|t| !t.expires_within(now, self.inner.expiry_delta))
            .cloned()
    }

    async fn fetch_app_token(&self, p: &Provider) -> Result<Token, Error> {
        let response = self
            .token_client(p)?
            .exchange_client_credentials()
            .add_scopes(p.app_scopes.iter().cloned().map(Scope::new))
            .request_async(&self.inner.http)
            .await
            .map_err(|e| provider_error(p, &e))?;
        let token = Token::from_response(&response, self.now(), None, &p.app_scopes);
        lock(&self.inner.app_tokens).insert(p.id.clone(), token.clone());
        Ok(token)
    }

    // ---- the authenticated client ------------------------------------------

    /// An HTTP client that calls the API as `account`, like golang's
    /// `Config.Client`.
    pub fn http(&self, account: &str, provider: &str) -> Authorized {
        Authorized::for_account(self.clone(), account, provider)
    }

    /// An HTTP client that calls the API as the application.
    pub fn app_http(&self, provider: &str) -> Authorized {
        Authorized::for_app(self.clone(), provider)
    }

    pub(crate) fn api(&self) -> &reqwest::Client {
        &self.inner.api
    }

    // ---- connections -------------------------------------------------------

    /// The account's connections, without tokens.
    ///
    /// # Errors
    /// [`Error::Store`].
    pub async fn connections(&self, account: &str) -> Result<Vec<Connection>, Error> {
        Ok(self.inner.store.for_account(account).await?)
    }

    /// Every connection, without tokens, `limit` from `offset`.
    ///
    /// # Errors
    /// [`Error::Store`].
    pub async fn all_connections(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Connection>, Error> {
        Ok(self.inner.store.list(limit, offset).await?)
    }

    /// Revokes the grant at the provider when it has an RFC 7009 endpoint,
    /// then deletes the row. The revocation is best effort: a provider that
    /// is down must not keep a token here that the person asked to remove.
    /// `true` when there was a row.
    ///
    /// # Errors
    /// [`Error::Store`].
    pub async fn disconnect(&self, account: &str, provider: &str) -> Result<bool, Error> {
        let Some(row) = self.inner.store.get(account, provider).await? else {
            return Ok(false);
        };
        if let Ok(p) = self.provider(provider) {
            if let (Some(url), Ok(token)) = (&p.endpoint.revoke_url, self.open(&row)) {
                let _ = self.revoke(p, url, &token).await;
            }
        }
        Ok(self.inner.store.delete(account, provider).await?)
    }

    /// The RFC 7009 request, sent here and not through `oauth2`: `oauth2`
    /// refuses an `http` revocation URL outright, and the https rule is
    /// this crate's ([`Provider::check`]), which development may relax.
    /// One code path, so the tests run the one production runs.
    async fn revoke(&self, p: &Provider, url: &str, token: &Token) -> Result<(), String> {
        // Revoking the refresh token ends the grant, and with it every
        // access token (RFC 7009 §2.1).
        let (value, hint) = match &token.refresh_token {
            Some(refresh) => (refresh.as_str(), "refresh_token"),
            None => (token.access_token.as_str(), "access_token"),
        };
        let mut pairs = vec![("token", value), ("token_type_hint", hint)];
        let mut request = http::Request::builder()
            .method(http::Method::POST)
            .uri(url)
            .header(
                http::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .header(http::header::ACCEPT, "application/json");
        match p.endpoint.auth_style {
            AuthStyle::InParams => {
                pairs.push(("client_id", p.client_id.as_str()));
                pairs.push(("client_secret", p.client_secret.as_str()));
            }
            AuthStyle::InHeader => {
                use base64::Engine as _;
                let encode = |s: &str| -> String {
                    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
                };
                let pair = format!("{}:{}", encode(&p.client_id), encode(&p.client_secret));
                request = request.header(
                    http::header::AUTHORIZATION,
                    format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(pair)
                    ),
                );
            }
        }
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&pairs)
            .finish();
        let request = request.body(body.into_bytes()).map_err(|e| e.to_string())?;
        let response = oauth2::AsyncHttpClient::call(&self.inner.http, request)
            .await
            .map_err(|e| e.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!(
                "the revocation endpoint answered {}",
                response.status()
            ))
        }
    }

    // ---- sealing -----------------------------------------------------------

    async fn load(&self, account: &str, p: &Provider) -> Result<(Token, i64), Error> {
        let row =
            self.inner
                .store
                .get(account, &p.id)
                .await?
                .ok_or_else(|| Error::NotConnected {
                    account: account.to_string(),
                    provider: p.id.clone(),
                })?;
        Ok((self.open(&row)?, row.connected_at))
    }

    fn open(&self, row: &StoredToken) -> Result<Token, Error> {
        let key = &self.inner.key;
        let text =
            |bytes: Vec<u8>| String::from_utf8(bytes).map_err(|_| Error::Crypto("not text".into()));
        let access =
            text(key.open_token(&row.access, &aad(&row.account, &row.provider, "access"))?)?;
        let refresh = match &row.refresh {
            Some(sealed) => Some(text(
                key.open_token(sealed, &aad(&row.account, &row.provider, "refresh"))?,
            )?),
            None => None,
        };
        Ok(Token {
            access_token: access,
            refresh_token: refresh,
            token_type: row.token_type.clone(),
            scopes: row.scopes.clone(),
            expires_at: row.expires_at,
        })
    }

    async fn save(
        &self,
        account: &str,
        p: &Provider,
        token: &Token,
        connected_at: i64,
    ) -> Result<(), Error> {
        let key = &self.inner.key;
        let row = StoredToken {
            account: account.to_string(),
            provider: p.id.clone(),
            access: key.seal_token(
                token.access_token.as_bytes(),
                &aad(account, &p.id, "access"),
            ),
            refresh: token
                .refresh_token
                .as_ref()
                .map(|r| key.seal_token(r.as_bytes(), &aad(account, &p.id, "refresh"))),
            token_type: token.token_type.clone(),
            scopes: token.scopes.clone(),
            expires_at: token.expires_at,
            connected_at,
            refreshed_at: self.now(),
        };
        Ok(self.inner.store.put(row).await?)
    }

    // ---- oauth2 ------------------------------------------------------------

    fn redirect_url(&self, p: &Provider) -> Result<RedirectUrl, Error> {
        RedirectUrl::new(self.redirect_uri(&p.id))
            .map_err(|e| Error::Config(format!("`{}` redirect URI: {e}", p.id)))
    }

    fn token_client(&self, p: &Provider) -> Result<TokenClient, Error> {
        let token_url = TokenUrl::new(p.endpoint.token_url.clone())
            .map_err(|e| Error::Config(format!("`{}` token_url: {e}", p.id)))?;
        Ok(BasicClient::new(ClientId::new(p.client_id.clone()))
            .set_client_secret(ClientSecret::new(p.client_secret.clone()))
            .set_auth_type(match p.endpoint.auth_style {
                AuthStyle::InParams => AuthType::RequestBody,
                AuthStyle::InHeader => AuthType::BasicAuth,
            })
            .set_token_uri(token_url)
            .set_redirect_uri(self.redirect_url(p)?))
    }
}

/// The associated data of a sealed token: who it belongs to and which one
/// it is. Length-prefixed, so no two triples give the same bytes.
fn aad(account: &str, provider: &str, kind: &str) -> Vec<u8> {
    format!(
        "{}:{account}{}:{provider}{kind}",
        account.len(),
        provider.len()
    )
    .into_bytes()
}

/// The token error as text a log may hold: the provider's error code and
/// description, never the body of an answer that failed to parse.
fn describe(e: &BasicErrorResponse) -> String {
    let mut out = e.error().to_string();
    if let Some(text) = e.error_description() {
        out.push_str(": ");
        out.push_str(&clean(text));
    }
    out
}

fn provider_error(p: &Provider, e: &RequestTokenError<HttpError, BasicErrorResponse>) -> Error {
    let message = match e {
        RequestTokenError::ServerResponse(e) => describe(e),
        RequestTokenError::Request(e) => e.to_string(),
        // The body may hold a token: it is not repeated.
        RequestTokenError::Parse(..) => "the answer is not a token response".to_string(),
        RequestTokenError::Other(text) => clean(text),
    };
    Error::Provider {
        provider: p.id.clone(),
        message,
    }
}

/// Provider text, cut to 200 characters with control characters dropped,
/// so it cannot forge a log line.
fn clean(text: &str) -> String {
    text.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// `raw` when it is a path on this site, else `None`.
///
/// It starts with one `/`, and the second character is neither `/` nor
/// `\`: `//evil.example` and `/\evil.example` are other hosts to a browser.
/// No backslash and no control character anywhere.
pub fn local_path(raw: &str) -> Option<String> {
    let ok = raw.starts_with('/')
        && !raw.starts_with("//")
        && !raw.contains('\\')
        && !raw.chars().any(char::is_control)
        && raw.len() <= 512;
    ok.then(|| raw.to_string())
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The lock of one key, made on first use.
fn flight<K: Eq + Hash + Clone>(map: &Flights<K>, key: &K) -> Arc<tokio::sync::Mutex<()>> {
    lock(map).entry(key.clone()).or_default().clone()
}

/// Drops the key's lock from the map when nobody else holds a copy, so the
/// map does not grow with every account that ever refreshed.
fn land<K: Eq + Hash>(map: &Flights<K>, key: &K, held: Arc<tokio::sync::Mutex<()>>) {
    let mut map = lock(map);
    // Two copies: the map's and `held`. Nobody can take a third without
    // the map lock, which this holds.
    if Arc::strong_count(&held) == 2 {
        map.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_path_on_this_site_is_kept() {
        for good in ["/", "/settings", "/a/b?c=d#e"] {
            assert_eq!(local_path(good).as_deref(), Some(good));
        }
        for bad in [
            "https://evil.example/",
            "//evil.example/",
            "/\\evil.example",
            "\\\\evil",
            "javascript:alert(1)",
            "evil",
            "/a\nb",
            "",
        ] {
            assert_eq!(local_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_token_debug_shows_no_token() {
        let token = Token {
            access_token: "at-secret".into(),
            refresh_token: Some("rt-secret".into()),
            token_type: "Bearer".into(),
            scopes: vec![],
            expires_at: None,
        };
        let text = format!("{token:?}");
        assert!(!text.contains("secret"), "{text}");
        assert!(!token.expires_within(0, 10), "no expiry never runs out");
    }

    #[test]
    fn the_associated_data_does_not_collide() {
        assert_ne!(aad("ab", "c", "access"), aad("a", "bc", "access"));
    }
}
