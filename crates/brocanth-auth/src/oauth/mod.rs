//! The authorization-code flow, once, for every provider.
//!
//! Two calls. [`OauthClient::begin`] produces a URL to send the browser to and
//! a [`FlowState`] to keep; [`OauthClient::complete`] takes that state and the
//! callback parameters and produces a [`ProviderIdentity`].
//!
//! Nothing in here names a provider. Whatever a provider needs that the others
//! do not is declared on the [`Provider`] value as a [`Quirk`], so this file
//! contains the protocol and the catalogue contains the differences.
//!
//! # The state is yours to carry, not ours to store
//!
//! [`FlowState`] holds the `state`, the `nonce` and the PKCE verifier. It is
//! returned to you rather than written to a table, because requiring a
//! database row per *started* login would hand an unauthenticated visitor a
//! write on your storage. [`FlowState::seal`] signs it into a cookie value.
//!
//! **That cookie must be `HttpOnly`, `Secure` and short-lived.** Sealing is a
//! signature, not encryption: it proves the state was not tampered with, and
//! anybody who can read the cookie can read the verifier. `HttpOnly` is what
//! keeps script out of it.
//!
//! [`ProviderIdentity`]: crate::identity::ProviderIdentity

pub mod oidc;
pub mod pkce;

use std::collections::BTreeMap;
use std::sync::Arc;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::apple::{AppleKey, FirstAuthorizationUser};
use crate::clock::{Clock, UnixTime};
use crate::error::AuthError;
use crate::http_client::{HttpClient, HttpRequest, HttpResponse};
use crate::identity::ProviderIdentity;
use crate::providers::{AuthStyle, Provider, ProviderKind, Quirk};
use crate::random::{b64, ct_eq, ct_eq_str, random_secret, unb64};
use crate::session::SigningKey;

pub use oidc::Jwks;
pub use pkce::PkcePair;

/// How the client authenticates itself to the token endpoint.
#[derive(Clone)]
pub enum ClientSecret {
    /// A string the provider issued.
    Static(String),
    /// A JWT this crate signs per request. Apple, and only Apple — see
    /// [`crate::apple`].
    Apple(AppleKey),
    /// No secret. A public client, which is exactly the case PKCE exists for
    /// and exactly the case that would be indefensible without it.
    None,
}

impl std::fmt::Debug for ClientSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientSecret::Static(_) => f.write_str("ClientSecret::Static(<redacted>)"),
            ClientSecret::Apple(k) => write!(f, "ClientSecret::Apple({k:?})"),
            ClientSecret::None => f.write_str("ClientSecret::None"),
        }
    }
}

/// One application's registration at one provider.
#[derive(Clone, Debug)]
pub struct ClientConfig {
    /// The client identifier the provider issued. Apple calls it a Services ID.
    pub client_id: String,
    /// How to authenticate at the token endpoint.
    pub secret: ClientSecret,
    /// Where the provider sends the browser back.
    ///
    /// Sent again at the token endpoint, where the provider checks it matches,
    /// and recorded in the [`FlowState`] so the exchange cannot be pointed
    /// somewhere else.
    pub redirect_uri: String,
    /// Scopes to request instead of the provider's defaults.
    pub scopes: Option<Vec<String>>,
}

impl ClientConfig {
    /// A configuration with a static secret.
    pub fn new(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        redirect_uri: impl Into<String>,
    ) -> Self {
        Self {
            client_id: client_id.into(),
            secret: ClientSecret::Static(client_secret.into()),
            redirect_uri: redirect_uri.into(),
            scopes: None,
        }
    }

    /// Requests these scopes instead of the provider's defaults.
    #[must_use]
    pub fn with_scopes<I, S>(mut self, scopes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.scopes = Some(scopes.into_iter().map(Into::into).collect());
        self
    }
}

/// Extra parameters for one authorization request.
#[derive(Clone, Debug, Default)]
pub struct AuthorizeOptions {
    /// Scopes for this request only.
    pub scopes: Option<Vec<String>>,
    /// Anything else to put on the authorization URL: `prompt=consent`,
    /// `access_type=offline`, `login_hint=...`.
    pub extra_params: Vec<(String, String)>,
}

impl AuthorizeOptions {
    /// Adds a parameter.
    #[must_use]
    pub fn param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_params.push((name.into(), value.into()));
        self
    }
}

/// Everything the callback will need, and nothing the provider sees.
///
/// Serializable so it can live in a signed cookie for the duration of the
/// round trip. See the module documentation for why it is not a database row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlowState {
    /// Which provider this flow belongs to. Checked on the callback, so a
    /// state minted for one provider cannot be redeemed at another.
    pub provider: String,
    /// The CSRF value echoed by the provider.
    pub state: String,
    /// The OIDC nonce. Empty for a plain OAuth 2.0 provider, which has no
    /// id_token to bind it to.
    pub nonce: String,
    /// The PKCE verifier. The secret that makes a stolen code worthless.
    pub pkce_verifier: String,
    /// The redirect URI this flow started with.
    pub redirect_uri: String,
    /// When the flow started, for [`AuthError::FlowExpired`].
    pub started_at: UnixTime,
}

impl FlowState {
    /// Signs the state into a cookie value: `payload.signature`.
    ///
    /// # Errors
    /// Returns [`AuthError::Crypto`] if the state cannot be serialised, which
    /// it always can.
    pub fn seal(&self, key: &SigningKey) -> Result<String, AuthError> {
        let json = serde_json::to_vec(self)
            .map_err(|e| AuthError::crypto(format!("could not serialise the flow state: {e}")))?;
        let payload = b64(&json);
        Ok(format!("{payload}.{}", b64(&sign(key, payload.as_bytes()))))
    }

    /// Recovers a sealed state.
    ///
    /// # Errors
    /// Returns [`AuthError::StateMismatch`] for anything that does not verify.
    /// A tampered flow state and a tampered `state` parameter are the same
    /// attack, so they are the same error.
    pub fn unseal(key: &SigningKey, sealed: &str) -> Result<Self, AuthError> {
        let (payload, signature) = sealed.rsplit_once('.').ok_or(AuthError::StateMismatch)?;
        let presented = unb64(signature).map_err(|_| AuthError::StateMismatch)?;
        if !ct_eq(&presented, &sign(key, payload.as_bytes())) {
            return Err(AuthError::StateMismatch);
        }
        let json = unb64(payload).map_err(|_| AuthError::StateMismatch)?;
        serde_json::from_slice(&json).map_err(|_| AuthError::StateMismatch)
    }
}

fn sign(key: &SigningKey, message: &[u8]) -> Vec<u8> {
    // `SigningKey::sign` is private to the session module on purpose: one key,
    // one signer. This goes through the same code.
    key.sign_message(message)
}

/// Where to send the browser, and what to keep until it comes back.
#[derive(Clone, Debug)]
pub struct AuthorizationRequest {
    /// The URL to redirect to.
    pub url: String,
    /// The state to carry across the round trip.
    pub flow: FlowState,
}

/// What the provider put on the callback.
#[derive(Clone, Debug, Default)]
pub struct CallbackParams {
    /// The authorization code.
    pub code: String,
    /// The `state` parameter, to be checked against [`FlowState::state`].
    pub state: String,
    /// The provider's `error`, when it refused.
    pub error: Option<String>,
    /// The provider's `error_description`.
    pub error_description: Option<String>,
    /// Apple's `user` form field, present on a first authorization and never
    /// again. See [`crate::apple`].
    pub apple_user: Option<String>,
}

impl CallbackParams {
    /// Reads the parameters out of a query string or a form body.
    ///
    /// Takes an iterator of pairs rather than a string so the same function
    /// serves a `GET` query and Apple's `POST` form.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: Into<String>,
    {
        let mut out = Self::default();
        for (k, v) in pairs {
            match k.as_ref() {
                "code" => out.code = v.into(),
                "state" => out.state = v.into(),
                "error" => out.error = Some(v.into()),
                "error_description" => out.error_description = Some(v.into()),
                "user" => out.apple_user = Some(v.into()),
                _ => {}
            }
        }
        out
    }
}

/// What a token endpoint answered.
#[derive(Clone, Debug)]
pub struct TokenResponse {
    /// The access token.
    pub access_token: String,
    /// Usually `Bearer`.
    pub token_type: String,
    /// Lifetime in seconds, when the provider said.
    pub expires_in: Option<i64>,
    /// The refresh token, when the provider issued one.
    pub refresh_token: Option<String>,
    /// The id_token, for an OIDC provider.
    pub id_token: Option<String>,
    /// The scopes actually granted, which may be fewer than those requested.
    pub scope: Option<String>,
    /// The whole document, for a provider-specific field.
    pub raw: serde_json::Value,
}

impl TokenResponse {
    /// When the access token expires, given the current time.
    pub fn expires_at(&self, now: UnixTime) -> Option<UnixTime> {
        self.expires_in.map(|s| now + s)
    }
}

/// A completed sign-in.
#[derive(Clone, Debug)]
pub struct Authenticated {
    /// Who the provider says this is.
    pub identity: ProviderIdentity,
    /// The tokens, for an application that will call the provider's API.
    pub tokens: TokenResponse,
}

/// The flow, for every provider.
pub struct OauthClient<H: HttpClient, C: Clock> {
    http: Arc<H>,
    clock: Arc<C>,
    jwks: Jwks<H, C>,
    flow_ttl: i64,
}

impl<H: HttpClient, C: Clock> OauthClient<H, C> {
    /// Builds the client. A flow must complete within ten minutes.
    pub fn new(http: Arc<H>, clock: Arc<C>) -> Self {
        Self {
            jwks: Jwks::new(http.clone(), clock.clone()),
            http,
            clock,
            // Ten minutes is long enough for a password manager, a one-time
            // code and a consent screen, and short enough that a state cookie
            // left in a shared browser is not a standing invitation.
            flow_ttl: 600,
        }
    }

    /// Overrides how long a flow may take, in seconds.
    #[must_use]
    pub fn with_flow_ttl(mut self, seconds: i64) -> Self {
        self.flow_ttl = seconds;
        self
    }

    /// The JWKS cache, exposed so an application can warm it at startup.
    pub fn jwks(&self) -> &Jwks<H, C> {
        &self.jwks
    }

    /// Starts an authorization-code flow.
    ///
    /// PKCE is generated here unconditionally. There is no option and no
    /// branch: see [`pkce`].
    ///
    /// # Errors
    /// Fails when the OS has no randomness or the provider's authorize URL
    /// does not parse.
    pub fn begin(
        &self,
        provider: &Provider,
        client: &ClientConfig,
        options: &AuthorizeOptions,
    ) -> Result<AuthorizationRequest, AuthError> {
        let pkce = PkcePair::generate()?;
        let state = random_secret()?;
        // A nonce for every OIDC provider, always. A plain OAuth 2.0 provider
        // has no id_token to bind it to, so it gets an empty one rather than a
        // value nothing checks.
        let nonce = if provider.is_oidc() {
            random_secret()?
        } else {
            String::new()
        };

        let scopes = options
            .scopes
            .clone()
            .or_else(|| client.scopes.clone())
            .unwrap_or_else(|| {
                provider
                    .default_scopes
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect()
            });

        let mut url = Url::parse(&provider.authorize_url).map_err(|e| {
            AuthError::misconfigured(format!(
                "provider `{}` has an unparseable authorize URL: {e}",
                provider.id
            ))
        })?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("response_type", "code");
            q.append_pair("client_id", &client.client_id);
            q.append_pair("redirect_uri", &client.redirect_uri);
            q.append_pair("scope", &scopes.join(" "));
            q.append_pair("state", &state);
            q.append_pair("code_challenge", pkce.challenge());
            q.append_pair("code_challenge_method", pkce::METHOD);
            if provider.is_oidc() {
                q.append_pair("nonce", &nonce);
            }
            for (k, v) in provider.extra_authorize_params {
                q.append_pair(k, v);
            }
            for (k, v) in &options.extra_params {
                q.append_pair(k, v);
            }
        }

        Ok(AuthorizationRequest {
            url: url.to_string(),
            flow: FlowState {
                provider: provider.id.to_string(),
                state,
                nonce,
                pkce_verifier: pkce.verifier().to_string(),
                redirect_uri: client.redirect_uri.clone(),
                started_at: self.clock.now(),
            },
        })
    }

    /// Finishes the flow: checks `state`, exchanges the code with the PKCE
    /// verifier, verifies the id_token and its nonce, and maps the profile.
    ///
    /// The checks are in that order on purpose. `state` is free and rejects
    /// the CSRF case before a single byte leaves the process.
    ///
    /// # Errors
    /// See [`AuthError`]. The common ones here are
    /// [`AuthError::StateMismatch`], [`AuthError::NonceMismatch`],
    /// [`AuthError::FlowExpired`] and [`AuthError::ProviderRefused`].
    pub async fn complete(
        &self,
        provider: &Provider,
        client: &ClientConfig,
        flow: &FlowState,
        callback: &CallbackParams,
    ) -> Result<Authenticated, AuthError> {
        // 1. The provider may have refused outright.
        if let Some(error) = &callback.error {
            return Err(AuthError::ProviderRefused {
                error: error.clone(),
                description: callback.error_description.clone(),
            });
        }

        // 2. CSRF, in constant time, before anything else. A `!=` here returns
        //    at the first differing byte and tells a patient attacker how much
        //    of their guess was right.
        if !ct_eq_str(&callback.state, &flow.state) {
            return Err(AuthError::StateMismatch);
        }
        // A flow state is bound to one provider. Without this, a state sealed
        // for a provider that asserts nothing could be redeemed at one that
        // does.
        if flow.provider != provider.id {
            return Err(AuthError::StateMismatch);
        }
        if self.clock.now() - flow.started_at > self.flow_ttl {
            return Err(AuthError::FlowExpired);
        }
        if callback.code.is_empty() {
            return Err(AuthError::ProviderRefused {
                error: "invalid_request".into(),
                description: Some("the callback carried no authorization code".into()),
            });
        }

        // 3. Exchange, always with the verifier.
        let form = vec![
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), callback.code.clone()),
            ("redirect_uri".to_string(), flow.redirect_uri.clone()),
            ("code_verifier".to_string(), flow.pkce_verifier.clone()),
        ];
        let tokens = self.token_request(provider, client, form).await?;

        // 4. Identity.
        let mut identity = self.identify(provider, client, flow, &tokens).await?;

        // 5. Apple's one-time name. The id_token never carries it and there is
        //    no endpoint to ask, so this callback is the only chance.
        if provider.has(Quirk::NameOnFirstAuthorizationOnly) {
            if let Some(raw) = &callback.apple_user {
                let user = FirstAuthorizationUser::parse(raw)?;
                if let Some(name) = user.display_name() {
                    identity.name = Some(name);
                }
            }
        }

        Ok(Authenticated { identity, tokens })
    }

    /// Exchanges a refresh token for a new access token.
    ///
    /// Note what is *not* returned to you automatically: many providers rotate
    /// the refresh token and return a new one, and some return none, meaning
    /// keep the old. [`TokenResponse::refresh_token`] is therefore an
    /// `Option`, and the caller stores the new one when there is one.
    ///
    /// # Errors
    /// See [`AuthError`].
    pub async fn refresh(
        &self,
        provider: &Provider,
        client: &ClientConfig,
        refresh_token: &str,
    ) -> Result<TokenResponse, AuthError> {
        let form = vec![
            ("grant_type".to_string(), "refresh_token".to_string()),
            ("refresh_token".to_string(), refresh_token.to_string()),
        ];
        self.token_request(provider, client, form).await
    }

    async fn token_request(
        &self,
        provider: &Provider,
        client: &ClientConfig,
        mut form: Vec<(String, String)>,
    ) -> Result<TokenResponse, AuthError> {
        form.push(("client_id".to_string(), client.client_id.clone()));

        let mut request = HttpRequest::post_form(provider.token_url.as_ref(), vec![])
            // GitHub answers a token request with `application/x-www-form-
            // urlencoded` unless asked otherwise, and every other provider
            // ignores this header. One line here replaces a quirk.
            .header("Accept", "application/json");

        let secret = match (&provider.auth_style, &client.secret) {
            (AuthStyle::PrivateKeyJwt, ClientSecret::Apple(key)) => {
                Some(key.client_secret(self.clock.as_ref())?)
            }
            (AuthStyle::PrivateKeyJwt, _) => {
                return Err(AuthError::misconfigured(format!(
                    "provider `{}` signs its client secret with a private key; \
                     configure `ClientSecret::Apple`",
                    provider.id
                )))
            }
            (_, ClientSecret::Static(s)) => Some(s.clone()),
            (_, ClientSecret::Apple(_)) => {
                return Err(AuthError::misconfigured(format!(
                    "provider `{}` takes a static client secret, not an Apple key",
                    provider.id
                )))
            }
            (_, ClientSecret::None) => None,
        };

        if let Some(secret) = secret {
            match provider.auth_style {
                AuthStyle::ClientSecretBasic => {
                    let raw = format!("{}:{}", urlencode(&client.client_id), urlencode(&secret));
                    request = request.header(
                        "Authorization",
                        format!("Basic {}", BASE64_STANDARD.encode(raw)),
                    );
                }
                AuthStyle::ClientSecretPost | AuthStyle::PrivateKeyJwt => {
                    form.push(("client_secret".to_string(), secret));
                }
            }
        }

        request.form = Some(form);
        let response = self.http.send(request).await?;
        let body = self.read_json(provider, &response)?;

        let access_token = body
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| AuthError::ProviderResponse {
                provider: provider.id.to_string(),
                detail: "the token endpoint returned no `access_token`".into(),
            })?
            .to_string();

        Ok(TokenResponse {
            access_token,
            token_type: body
                .get("token_type")
                .and_then(|v| v.as_str())
                .unwrap_or("Bearer")
                .to_string(),
            expires_in: body.get("expires_in").and_then(|v| v.as_i64()),
            refresh_token: body
                .get("refresh_token")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            id_token: body
                .get("id_token")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            scope: body
                .get("scope")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            raw: body,
        })
    }

    /// Reads a provider's JSON body and turns every shape of failure into an
    /// error, including the one that arrives as `200 OK`.
    fn read_json(
        &self,
        provider: &Provider,
        response: &HttpResponse,
    ) -> Result<serde_json::Value, AuthError> {
        let body: serde_json::Value =
            serde_json::from_slice(&response.body).unwrap_or(serde_json::Value::Null);

        // Slack answers 200 with `{"ok": false, "error": "..."}`. Reading only
        // the status turns a refusal into a success with every field missing.
        if provider.has(Quirk::OkEnvelope)
            && body.get("ok").and_then(|v| v.as_bool()) == Some(false)
        {
            return Err(AuthError::ProviderRefused {
                error: body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                description: None,
            });
        }

        if !response.status.is_success() || body.get("error").is_some() {
            if let Some(error) = body.get("error") {
                return Err(AuthError::ProviderRefused {
                    // Some providers answer `error` as an object rather than a
                    // string. Either way the caller gets something printable.
                    error: error
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| error.to_string()),
                    description: body
                        .get("error_description")
                        .and_then(|v| v.as_str())
                        .map(str::to_string),
                });
            }
            return Err(AuthError::ProviderResponse {
                provider: provider.id.to_string(),
                detail: format!(
                    "{} with a body this crate cannot read: {}",
                    response.status,
                    truncate(&response.text(), 200)
                ),
            });
        }

        if body.is_null() {
            return Err(AuthError::ProviderResponse {
                provider: provider.id.to_string(),
                detail: "the response body is not JSON".into(),
            });
        }
        Ok(body)
    }

    async fn identify(
        &self,
        provider: &Provider,
        client: &ClientConfig,
        flow: &FlowState,
        tokens: &TokenResponse,
    ) -> Result<ProviderIdentity, AuthError> {
        match &provider.kind {
            // For an OIDC provider the identity is the id_token's claims, and
            // nothing else. They are signed by the provider, bound to this
            // client by `aud` and to this flow by `nonce`. A userinfo call
            // would add a second, unsigned source of truth for no gain: every
            // OIDC provider in the catalogue puts the subject, the address and
            // the verification flag in the token.
            ProviderKind::Oidc { issuer, jwks_url } => {
                let id_token =
                    tokens
                        .id_token
                        .as_deref()
                        .ok_or_else(|| AuthError::ProviderResponse {
                            provider: provider.id.to_string(),
                            detail: "an OIDC provider returned no `id_token`".into(),
                        })?;
                let verified = oidc::verify_id_token(
                    &self.jwks,
                    jwks_url,
                    id_token,
                    issuer,
                    &client.client_id,
                    &flow.nonce,
                    &provider.id,
                )
                .await?;
                provider.map_profile(&verified.claims)
            }
            ProviderKind::OAuth2 => {
                let url = provider.userinfo_url.as_deref().ok_or_else(|| {
                    AuthError::misconfigured(format!(
                        "provider `{}` is plain OAuth 2.0 and has no userinfo URL",
                        provider.id
                    ))
                })?;
                let response = self
                    .http
                    .send(
                        HttpRequest::get(url)
                            .header("Authorization", format!("Bearer {}", tokens.access_token))
                            .header("Accept", "application/json")
                            // GitHub requires a User-Agent and answers 403
                            // without one.
                            .header("User-Agent", "brocanth-auth"),
                    )
                    .await?;
                let mut profile = self.read_json(provider, &response)?;

                if provider.has(Quirk::GithubVerifiedEmails) {
                    self.add_github_verified_email(tokens, &mut profile).await?;
                }
                provider.map_profile(&profile)
            }
        }
    }

    /// GitHub's `/user` gives the public profile email, which nobody checked.
    /// The verified one is at `/user/emails`, and this folds it into the
    /// profile under the key the mapping expects.
    ///
    /// When there is no primary verified address the field stays absent and
    /// the identity simply has no email — which is correct, and which
    /// [`crate::linking`] then handles by creating an account without one.
    async fn add_github_verified_email(
        &self,
        tokens: &TokenResponse,
        profile: &mut serde_json::Value,
    ) -> Result<(), AuthError> {
        let response = self
            .http
            .send(
                HttpRequest::get("https://api.github.com/user/emails")
                    .header("Authorization", format!("Bearer {}", tokens.access_token))
                    .header("Accept", "application/json")
                    .header("User-Agent", "brocanth-auth"),
            )
            .await?;
        // A token without `user:email` gets a 403 here. That is not a failed
        // sign-in: it is a sign-in without an address.
        if !response.status.is_success() {
            return Ok(());
        }
        let list: serde_json::Value =
            serde_json::from_slice(&response.body).unwrap_or(serde_json::Value::Null);
        let found = list.as_array().and_then(|entries| {
            entries
                .iter()
                .find(|e| {
                    e.get("primary").and_then(|v| v.as_bool()) == Some(true)
                        && e.get("verified").and_then(|v| v.as_bool()) == Some(true)
                })
                .and_then(|e| e.get("email"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
        if let (Some(email), Some(object)) = (found, profile.as_object_mut()) {
            object.insert(
                "verified_primary_email".to_string(),
                serde_json::Value::String(email),
            );
        }
        Ok(())
    }
}

fn urlencode(text: &str) -> String {
    // RFC 6749 §2.3.1 wants the client id and secret percent-encoded before
    // they go into HTTP Basic. `url` has no standalone encoder for a form
    // component, so this is the smallest correct one.
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn truncate(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Parses a query string or form body into pairs, for
/// [`CallbackParams::from_pairs`].
pub fn parse_pairs(body: &str) -> BTreeMap<String, String> {
    url::form_urlencoded::parse(body.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}
