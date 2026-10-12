//! PKCE, and there is no way to turn it off.
//!
//! # Why it is mandatory rather than configurable
//!
//! Without PKCE, an authorization code is a bearer token in a URL. Anything
//! that can observe the redirect — a malicious app registered for the same
//! custom scheme, a logging proxy, a referrer header, a browser extension —
//! can replay it at the token endpoint and receive an access token. The
//! `client_secret` is supposed to prevent that, and a public client does not
//! have one that is actually secret.
//!
//! PKCE closes it: the client sends `SHA-256(verifier)` when it starts, and
//! the verifier itself when it redeems. A stolen code is worth nothing to
//! anybody who does not also hold the verifier, which never leaves the server.
//!
//! OAuth 2.1 makes PKCE required for *all* clients, confidential ones
//! included, and every provider in the catalogue supports it. There is no
//! remaining case for the alternative, so this crate does not implement one:
//! [`PkcePair`] is generated inside [`OauthClient::begin`], stored in
//! [`FlowState`], and sent by [`OauthClient::complete`] with no branch in
//! between.
//!
//! `plain` is not implemented either. It is in the RFC only for clients that
//! cannot compute SHA-256, which no server can honestly claim.
//!
//! [`FlowState`]: crate::oauth::FlowState
//! [`OauthClient::begin`]: crate::oauth::OauthClient::begin
//! [`OauthClient::complete`]: crate::oauth::OauthClient::complete

use crate::error::AuthError;
use crate::random::{ct_eq_str, random_secret, sha256_b64};

/// The only challenge method this crate speaks.
pub const METHOD: &str = "S256";

/// A verifier and the challenge derived from it.
#[derive(Clone)]
pub struct PkcePair {
    verifier: String,
    challenge: String,
}

impl PkcePair {
    /// Generates a pair: 256 bits of entropy, base64url, 43 characters — the
    /// shortest verifier the RFC allows and the most entropy that length can
    /// hold.
    ///
    /// # Errors
    /// See [`random_secret`].
    pub fn generate() -> Result<Self, AuthError> {
        let verifier = random_secret()?;
        let challenge = sha256_b64(verifier.as_bytes());
        Ok(Self {
            verifier,
            challenge,
        })
    }

    /// Rebuilds a pair from a stored verifier, on the callback.
    pub fn from_verifier(verifier: impl Into<String>) -> Self {
        let verifier = verifier.into();
        let challenge = sha256_b64(verifier.as_bytes());
        Self {
            verifier,
            challenge,
        }
    }

    /// The secret half. Goes to the token endpoint and nowhere else.
    pub fn verifier(&self) -> &str {
        &self.verifier
    }

    /// The public half. Goes on the authorization URL.
    pub fn challenge(&self) -> &str {
        &self.challenge
    }
}

impl std::fmt::Debug for PkcePair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PkcePair")
            .field("challenge", &self.challenge)
            .field("verifier", &"<redacted>")
            .finish()
    }
}

/// Checks a verifier against a challenge, in constant time.
///
/// This crate is the *client*, so it never has to perform this check against a
/// remote party — the authorization server does. It is here because it is the
/// only honest way to test that the challenge sent on the authorization URL
/// and the verifier sent to the token endpoint are the two halves of one pair,
/// which is the property the whole mechanism rests on.
pub fn verify(verifier: &str, challenge: &str) -> bool {
    ct_eq_str(&sha256_b64(verifier.as_bytes()), challenge)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_verifies_against_itself_and_nothing_else() {
        let a = PkcePair::generate().unwrap();
        let b = PkcePair::generate().unwrap();
        assert!(verify(a.verifier(), a.challenge()));
        assert!(!verify(a.verifier(), b.challenge()));
        assert!(!verify(b.verifier(), a.challenge()));
    }

    #[test]
    fn a_verifier_is_43_url_safe_characters() {
        let p = PkcePair::generate().unwrap();
        assert_eq!(p.verifier().len(), 43);
        assert!(p
            .verifier()
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert_eq!(p.challenge().len(), 43);
    }

    #[test]
    fn the_rfc_test_vector_matches() {
        // RFC 7636 appendix B.
        assert_eq!(
            crate::random::sha256_b64(b"dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
