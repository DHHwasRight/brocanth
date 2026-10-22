//! id_token verification: signature, issuer, audience, expiry, and nonce.
//!
//! # The three ways this is normally got wrong
//!
//! **Trusting the header's `alg`.** The classic JWT break is to take a token
//! signed `RS256`, re-sign it `HS256` using the *public* key as the HMAC
//! secret, and hand it to a verifier that reads the algorithm out of the token
//! it is verifying. [`verify_id_token`] refuses every symmetric algorithm and
//! `none` before it looks at a key: see [`asymmetric`].
//!
//! **Not checking `aud`.** An id_token minted for a different client at the
//! same provider is a perfectly valid, correctly signed token. Without an
//! audience check, anybody who can get a Google token for their own
//! application can present it as yours.
//!
//! **Not checking `nonce`.** Without it an id_token captured anywhere can be
//! replayed into your callback. This crate makes `nonce` mandatory for every
//! OIDC provider — [`OauthClient::begin`] always generates one and
//! [`OauthClient::complete`] always checks it.
//!
//! [`OauthClient::begin`]: crate::oauth::OauthClient::begin
//! [`OauthClient::complete`]: crate::oauth::OauthClient::complete

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};

use crate::clock::{Clock, UnixTime};
use crate::error::AuthError;
use crate::http_client::{HttpClient, HttpRequest};
use crate::providers::IssuerCheck;
use crate::random::ct_eq_str;

/// The algorithms an id_token may be signed with.
///
/// Public-key only. `HS*` is excluded because a provider's JWKS is public and
/// an HMAC verification would therefore accept a token anybody could mint;
/// `none` is excluded because it is not a signature.
pub fn asymmetric(alg: Algorithm) -> bool {
    matches!(
        alg,
        Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::ES256
            | Algorithm::ES384
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512
            | Algorithm::EdDSA
    )
}

/// A JWKS, cached per URL.
///
/// A provider rotates its keys, so the cache has a lifetime; a provider also
/// rate-limits, so there is a cache at all. On a `kid` the cache does not know,
/// the fetch is forced once — that is the key-rotation path, and treating it
/// as a miss rather than a failure is what stops a rotation from signing
/// everybody out.
pub struct Jwks<H: HttpClient, C: Clock> {
    http: Arc<H>,
    clock: Arc<C>,
    ttl: i64,
    cache: Mutex<HashMap<String, (JwkSet, UnixTime)>>,
}

impl<H: HttpClient, C: Clock> Jwks<H, C> {
    /// A cache with a one-hour lifetime.
    pub fn new(http: Arc<H>, clock: Arc<C>) -> Self {
        Self {
            http,
            clock,
            ttl: 60 * 60,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Overrides the cache lifetime, in seconds.
    #[must_use]
    pub fn with_ttl(mut self, ttl: i64) -> Self {
        self.ttl = ttl;
        self
    }

    /// The key set at `url`, fetching if the cache is cold, stale, or does not
    /// contain `kid`.
    ///
    /// # Errors
    /// Returns [`AuthError::Http`] or [`AuthError::ProviderResponse`].
    pub async fn get(&self, url: &str, kid: Option<&str>) -> Result<JwkSet, AuthError> {
        let now = self.clock.now();
        if let Some((set, fetched)) = self.cache.lock().expect("jwks lock").get(url) {
            let fresh = now - *fetched < self.ttl;
            let has_key = kid.is_none_or(|k| set.find(k).is_some());
            if fresh && has_key {
                return Ok(set.clone());
            }
        }
        let response = self.http.send(HttpRequest::get(url)).await?;
        if !response.status.is_success() {
            return Err(AuthError::ProviderResponse {
                provider: url.to_string(),
                detail: format!("the JWKS endpoint answered {}", response.status),
            });
        }
        let set: JwkSet =
            serde_json::from_slice(&response.body).map_err(|e| AuthError::ProviderResponse {
                provider: url.to_string(),
                detail: format!("the JWKS endpoint is not a JWK set: {e}"),
            })?;
        self.cache
            .lock()
            .expect("jwks lock")
            .insert(url.to_string(), (set.clone(), now));
        Ok(set)
    }
}

/// What a verified id_token said.
#[derive(Clone, Debug)]
pub struct VerifiedIdToken {
    /// Every claim, for [`crate::providers::Provider::map_profile`].
    pub claims: serde_json::Value,
}

/// Verifies an id_token and its nonce.
///
/// The order is: reject a symmetric or absent algorithm, find the key by
/// `kid`, verify the signature and `exp` and `aud`, then check `iss`, then
/// check `nonce`. Everything cheap that can reject comes first, and nothing
/// reads a claim before the signature is proved.
///
/// # Errors
/// * [`AuthError::NonceMismatch`] when the `nonce` claim is absent or wrong.
/// * [`AuthError::ProviderResponse`] for a bad signature, a wrong audience, an
///   expired token, an unacceptable issuer or an unusable algorithm.
pub async fn verify_id_token<H: HttpClient, C: Clock>(
    jwks: &Jwks<H, C>,
    jwks_url: &str,
    id_token: &str,
    issuer: &IssuerCheck,
    audience: &str,
    expected_nonce: &str,
    provider_id: &str,
) -> Result<VerifiedIdToken, AuthError> {
    let bad = |detail: String| AuthError::ProviderResponse {
        provider: provider_id.to_string(),
        detail,
    };

    let header = decode_header(id_token).map_err(|e| bad(format!("unreadable id_token: {e}")))?;
    if !asymmetric(header.alg) {
        // Not "unsupported": refused. A provider offering to let us verify
        // with a symmetric algorithm is a provider we are being lied to by.
        return Err(bad(format!(
            "the id_token is signed with `{:?}`, which is not a public-key algorithm",
            header.alg
        )));
    }

    let set = jwks.get(jwks_url, header.kid.as_deref()).await?;
    let jwk = match &header.kid {
        Some(kid) => set
            .find(kid)
            .ok_or_else(|| bad(format!("the provider's JWKS has no key `{kid}`")))?,
        // A provider with a single unnamed key is legal, if unusual.
        None => set
            .keys
            .first()
            .ok_or_else(|| bad("the provider's JWKS is empty".into()))?,
    };
    let key =
        DecodingKey::from_jwk(jwk).map_err(|e| bad(format!("unusable key in the JWKS: {e}")))?;

    let mut validation = Validation::new(header.alg);
    validation.set_audience(&[audience]);
    validation.validate_exp = true;
    // `iss` is checked below rather than here, because a template issuer —
    // Microsoft's — cannot be expressed as a set of exact strings.
    validation.iss = None;

    let data = decode::<serde_json::Value>(id_token, &key, &validation)
        .map_err(|e| bad(format!("the id_token did not verify: {e}")))?;
    let claims = data.claims;

    let iss = claims
        .get("iss")
        .and_then(|v| v.as_str())
        .ok_or_else(|| bad("the id_token has no `iss`".into()))?;
    if !issuer.accepts(iss) {
        return Err(bad(format!(
            "the id_token was issued by `{iss}`, not `{}`",
            issuer.describe()
        )));
    }

    let nonce = claims.get("nonce").and_then(|v| v.as_str()).unwrap_or("");
    if !ct_eq_str(nonce, expected_nonce) {
        return Err(AuthError::NonceMismatch);
    }

    Ok(VerifiedIdToken { claims })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symmetric_and_none_are_refused_before_any_key_is_looked_at() {
        assert!(!asymmetric(Algorithm::HS256));
        assert!(!asymmetric(Algorithm::HS384));
        assert!(!asymmetric(Algorithm::HS512));
        assert!(asymmetric(Algorithm::RS256));
        assert!(asymmetric(Algorithm::ES256));
    }
}
