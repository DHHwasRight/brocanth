//! id_token verification: signature, issuer, audience, expiry, and nonce.
//!
//! `openidconnect` does the verification. This file chooses its settings,
//! keeps the provider's key set in a cache, and turns its errors into
//! [`AuthError`].
//!
//! # The three ways this is normally got wrong
//!
//! **Trusting the header's `alg`.** The classic JWT break is to take a token
//! signed `RS256`, re-sign it `HS256` using the *public* key as the HMAC
//! secret, and hand it to a verifier that reads the algorithm out of the token
//! it is verifying. The verifier here allows the public-key algorithms in
//! [`ALLOWED_ALGS`] and nothing else, and it holds no client secret, so an
//! `HS*` or `none` token cannot verify.
//!
//! **Not checking `aud`.** An id_token minted for a different client at the
//! same provider is a perfectly valid, correctly signed token. Without an
//! audience check, anybody who can get a Google token for their own
//! application can present it as yours. A token with more than one audience
//! is refused too: this client trusts no other audience.
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

use openidconnect::core::{
    CoreGenderClaim, CoreIdTokenVerifier, CoreJsonWebKeySet, CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
};
use openidconnect::{
    AdditionalClaims, ClaimsVerificationError, ClientId, IdToken, IssuerUrl, JsonWebKeySetUrl,
    Nonce, SignatureVerificationError,
};
use serde::{Deserialize, Serialize};

use crate::clock::{Clock, UnixTime};
use crate::error::AuthError;
use crate::http_client::HttpClient;
use crate::oauth::transport::Transport;
use crate::providers::IssuerCheck;
use crate::random::ct_eq_str;

/// The algorithms an id_token may be signed with.
///
/// Public-key only. `HS*` is excluded because a provider's JWKS is public and
/// an HMAC verification would therefore accept a token anybody could mint;
/// `none` is excluded because it is not a signature.
pub const ALLOWED_ALGS: &[CoreJwsSigningAlgorithm] = &[
    CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
    CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384,
    CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512,
    CoreJwsSigningAlgorithm::EcdsaP256Sha256,
    CoreJwsSigningAlgorithm::EcdsaP384Sha384,
    CoreJwsSigningAlgorithm::RsaSsaPssSha256,
    CoreJwsSigningAlgorithm::RsaSsaPssSha384,
    CoreJwsSigningAlgorithm::RsaSsaPssSha512,
    CoreJwsSigningAlgorithm::EdDsa,
];

/// How far a provider's clock may run ahead of ours, in seconds. A token
/// is accepted until `exp + LEEWAY`.
const LEEWAY: i64 = 60;

/// A JWKS, cached per URL.
///
/// A provider rotates its keys, so the cache has a lifetime; a provider also
/// rate-limits, so there is a cache at all. When no key in the cache matches
/// a token, the fetch is forced once — that is the key-rotation path, and
/// treating it as a miss rather than a failure is what stops a rotation from
/// signing everybody out.
pub struct Jwks<H: HttpClient, C: Clock> {
    transport: Transport<H>,
    clock: Arc<C>,
    ttl: i64,
    cache: Mutex<HashMap<String, (CoreJsonWebKeySet, UnixTime)>>,
}

impl<H: HttpClient, C: Clock> Jwks<H, C> {
    /// A cache with a one-hour lifetime.
    pub fn new(http: Arc<H>, clock: Arc<C>) -> Self {
        Self {
            transport: Transport(http),
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

    /// The key set at `url`, fetched when the cache is cold or stale.
    ///
    /// # Errors
    /// Returns [`AuthError::Http`] or [`AuthError::ProviderResponse`].
    pub async fn get(&self, url: &str) -> Result<CoreJsonWebKeySet, AuthError> {
        let now = self.clock.now();
        if let Some((set, fetched)) = self.cache.lock().expect("jwks lock").get(url) {
            if now - *fetched < self.ttl {
                return Ok(set.clone());
            }
        }
        self.fetch(url).await
    }

    /// The key set at `url`, fetched now whatever the cache holds.
    ///
    /// # Errors
    /// Returns [`AuthError::Http`] or [`AuthError::ProviderResponse`].
    pub async fn fetch(&self, url: &str) -> Result<CoreJsonWebKeySet, AuthError> {
        let bad = |detail: String| AuthError::ProviderResponse {
            provider: url.to_string(),
            detail,
        };
        let parsed = JsonWebKeySetUrl::new(url.to_string())
            .map_err(|e| bad(format!("the JWKS URL does not parse: {e}")))?;
        let set = CoreJsonWebKeySet::fetch_async(&parsed, &self.transport)
            .await
            .map_err(|e| match e {
                openidconnect::DiscoveryError::Request(inner) => inner,
                other => bad(format!("the JWKS endpoint gave no usable key set: {other}")),
            })?;
        self.cache
            .lock()
            .expect("jwks lock")
            .insert(url.to_string(), (set.clone(), self.clock.now()));
        Ok(set)
    }
}

/// Every claim of the id_token, kept for
/// [`crate::providers::Provider::map_profile`]. `openidconnect` reads the
/// standard ones; a provider's own (`tid`, `is_private_email`) land here.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct OtherClaims {
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl AdditionalClaims for OtherClaims {}

type Token = IdToken<
    OtherClaims,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJwsSigningAlgorithm,
>;

/// What a verified id_token said.
#[derive(Clone, Debug)]
pub struct VerifiedIdToken {
    /// Every claim, for [`crate::providers::Provider::map_profile`].
    pub claims: serde_json::Value,
}

/// Verifies an id_token and its nonce.
///
/// `openidconnect` checks, in order: the issuer, the audience, the
/// algorithm against [`ALLOWED_ALGS`], the signature against the provider's
/// key set, the expiry against `clock`, then the nonce. A tenant issuer
/// (Microsoft's `/common`) is checked here, after the signature, because it
/// is a template and not one string.
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
    let token: Token = id_token
        .parse()
        .map_err(|e| bad(format!("unreadable id_token: {e}")))?;
    let check = Check {
        issuer,
        audience,
        expected_nonce,
        now: jwks.clock.now(),
    };

    let keys = jwks.get(jwks_url).await?;
    let claims = match check.run(&token, keys) {
        // A key the cache does not hold: the provider may have rotated.
        Err(ClaimsVerificationError::SignatureVerification(
            SignatureVerificationError::NoMatchingKey,
        )) => check.run(&token, jwks.fetch(jwks_url).await?),
        other => other,
    }
    .map_err(|e| match e {
        ClaimsVerificationError::InvalidNonce(_) => AuthError::NonceMismatch,
        // Its own text is only "Signature verification failed"; the reason
        // (a refused algorithm, no matching key) is the inner error.
        ClaimsVerificationError::SignatureVerification(why) => {
            bad(format!("the id_token did not verify: {why}"))
        }
        other => bad(format!("the id_token did not verify: {other}")),
    })?;

    let iss = claims.get("iss").and_then(|v| v.as_str()).unwrap_or("");
    if !issuer.accepts(iss) {
        return Err(bad(format!(
            "the id_token was issued by `{iss}`, not `{}`",
            issuer.describe()
        )));
    }
    Ok(VerifiedIdToken { claims })
}

/// One verification of one token against one key set.
struct Check<'a> {
    issuer: &'a IssuerCheck,
    audience: &'a str,
    expected_nonce: &'a str,
    now: UnixTime,
}

impl Check<'_> {
    fn run(
        &self,
        token: &Token,
        keys: CoreJsonWebKeySet,
    ) -> Result<serde_json::Value, ClaimsVerificationError> {
        // An exact issuer is checked by `openidconnect`. A template cannot
        // be, so the crate is given a placeholder it does not compare, and
        // `verify_id_token` checks the template after the signature.
        let (expected, exact) = match self.issuer {
            IssuerCheck::Exact(iss) => (iss.to_string(), true),
            IssuerCheck::TenantTemplate { .. } => ("https://issuer.invalid".to_string(), false),
        };
        let expected = IssuerUrl::new(expected).map_err(|e| {
            ClaimsVerificationError::Other(format!("the expected issuer is not a URL: {e}"))
        })?;
        let now = self.now - LEEWAY;
        let verifier = CoreIdTokenVerifier::new_public_client(
            ClientId::new(self.audience.to_string()),
            expected,
            keys,
        )
        .set_allowed_algs(ALLOWED_ALGS.iter().cloned())
        .require_issuer_match(exact)
        .set_time_fn(move || {
            chrono::DateTime::from_timestamp(now, 0).unwrap_or(chrono::DateTime::UNIX_EPOCH)
        });
        let expected_nonce = self.expected_nonce;
        let claims = token.claims(&verifier, |nonce: Option<&Nonce>| {
            // In constant time: `==` tells a patient attacker how much of a
            // guess was right.
            match nonce {
                Some(nonce) if ct_eq_str(nonce.secret(), expected_nonce) => Ok(()),
                Some(_) => Err("the nonce is not this flow's".to_string()),
                None => Err("the id_token carries no nonce".to_string()),
            }
        })?;
        serde_json::to_value(claims)
            .map_err(|e| ClaimsVerificationError::Other(format!("the claims do not encode: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_symmetric_algorithm_and_no_none_is_allowed() {
        for alg in [
            CoreJwsSigningAlgorithm::HmacSha256,
            CoreJwsSigningAlgorithm::HmacSha384,
            CoreJwsSigningAlgorithm::HmacSha512,
            CoreJwsSigningAlgorithm::None,
        ] {
            assert!(!ALLOWED_ALGS.contains(&alg), "{alg:?}");
        }
        assert!(ALLOWED_ALGS.contains(&CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256));
        assert!(ALLOWED_ALGS.contains(&CoreJwsSigningAlgorithm::EcdsaP256Sha256));
    }
}
