//! Apple, which does not fit, and exactly how it does not.
//!
//! Every other provider in [`crate::providers::catalog`] is four URLs and a
//! field mapping. Apple needs code, in two places, and both of them bite
//! quietly rather than loudly.
//!
//! # 1. The client secret is a JWT you sign, and it expires
//!
//! Apple issues no static `client_secret`. You register a key in the developer
//! portal, download a `.p8` ES256 private key **once** — it cannot be
//! downloaded again — and mint a short-lived JWT for each token request:
//! `iss` is the team id, `sub` is the client id, `aud` is
//! `https://appleid.apple.com`, the header carries the key id, and `exp` is at
//! most six months out.
//!
//! The failure mode is a deployment that generates the secret once, puts the
//! string in an environment variable, and gets `invalid_client` six months
//! later with no deployment to blame. [`AppleKey`] therefore signs per request
//! with a short lifetime, so the secret is never a stored value at all.
//!
//! # 2. The name arrives exactly once, and then never again
//!
//! On the **first** authorization for a given Apple ID and client, Apple posts
//! a `user` form field alongside `code` and `state`, containing JSON with the
//! person's given and family name. Every later sign-in — including after you
//! delete the account at your end and they sign in again — omits it. The
//! id_token never carries a name, and Apple has no userinfo endpoint to ask.
//!
//! So the name is a one-time delivery, and the only correct handling is to
//! persist it during that callback. A product that reads the name later, or
//! that treats the first callback as a dry run and asks the user to confirm
//! before saving, loses it permanently and the user cannot get it back short
//! of revoking the application in their Apple ID settings and starting over.
//!
//! [`FirstAuthorizationUser`] parses the field and
//! [`crate::oauth::CallbackParams::apple_user`] carries it. The flow folds it
//! into [`crate::identity::ProviderIdentity::name`] when it is there, so the
//! caller gets the name in the normal place on the one callback it exists, and
//! `None` afterwards.
//!
//! # A third, smaller one
//!
//! Requesting the `name` or `email` scope forces `response_mode=form_post`, so
//! Apple's callback is a `POST` from `appleid.apple.com`. A `SameSite=Strict`
//! session cookie is not sent with it, which is why
//! [`crate::session::SessionConfig`] defaults to `Lax`. The `state` check is
//! what defends that callback, and it is not optional.

use serde::{Deserialize, Serialize};

use crate::clock::Clock;
use crate::error::AuthError;

/// The maximum lifetime Apple accepts for a client secret: six months.
pub const MAX_SECRET_LIFETIME: i64 = 15_777_000;

/// The audience of an Apple client secret.
pub const AUDIENCE: &str = "https://appleid.apple.com";

/// The ES256 key an Apple client secret is signed with.
///
/// Built from the `.p8` file the developer portal gives you once. Hold it like
/// a password: anybody with it can mint client secrets for your application
/// until you revoke the key.
#[derive(Clone)]
pub struct AppleKey {
    /// The ten-character team identifier.
    pub team_id: String,
    /// The key identifier, which goes in the JWT header as `kid`.
    pub key_id: String,
    /// The Services ID — Apple's name for the OAuth client id.
    pub client_id: String,
    /// The PKCS#8 PEM contents of the `.p8` file.
    pub private_key_pem: String,
    /// How long each minted secret lives. Defaults to one hour.
    ///
    /// Short rather than the six months Apple allows: a secret that lives for
    /// one request is not a secret that leaks from a log.
    pub lifetime: i64,
}

impl AppleKey {
    /// Builds a key with a one-hour secret lifetime.
    pub fn new(
        team_id: impl Into<String>,
        key_id: impl Into<String>,
        client_id: impl Into<String>,
        private_key_pem: impl Into<String>,
    ) -> Self {
        Self {
            team_id: team_id.into(),
            key_id: key_id.into(),
            client_id: client_id.into(),
            private_key_pem: private_key_pem.into(),
            lifetime: 60 * 60,
        }
    }

    /// Mints a client secret, valid from now.
    ///
    /// # Errors
    /// Returns [`AuthError::Misconfigured`] when the lifetime is outside
    /// Apple's range, and [`AuthError::Crypto`] when the key is not a usable
    /// ES256 private key — which in practice means the `.p8` was pasted
    /// wrongly, and is far better found at startup than at the token endpoint.
    pub fn client_secret(&self, clock: &dyn Clock) -> Result<String, AuthError> {
        if self.lifetime <= 0 || self.lifetime > MAX_SECRET_LIFETIME {
            return Err(AuthError::misconfigured(format!(
                "an Apple client secret must live between 1 and {MAX_SECRET_LIFETIME} seconds, not {}",
                self.lifetime
            )));
        }
        let now = clock.now();
        let claims = Claims {
            iss: self.team_id.clone(),
            iat: now,
            exp: now + self.lifetime,
            aud: AUDIENCE.to_string(),
            sub: self.client_id.clone(),
        };
        let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        let key = jsonwebtoken::EncodingKey::from_ec_pem(self.private_key_pem.as_bytes())
            .map_err(|e| AuthError::crypto(format!("the Apple private key is not usable: {e}")))?;
        jsonwebtoken::encode(&header, &claims, &key)
            .map_err(|e| AuthError::crypto(format!("could not sign the Apple client secret: {e}")))
    }
}

impl std::fmt::Debug for AppleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppleKey")
            .field("team_id", &self.team_id)
            .field("key_id", &self.key_id)
            .field("client_id", &self.client_id)
            .field("private_key_pem", &"<redacted>")
            .finish()
    }
}

#[derive(Serialize)]
struct Claims {
    iss: String,
    iat: i64,
    exp: i64,
    aud: String,
    sub: String,
}

/// The `user` form field Apple posts on a first authorization, and only then.
///
/// Parse it during that callback and persist what you need. There is no second
/// chance and no endpoint to ask.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct FirstAuthorizationUser {
    /// The name, split the way Apple splits it.
    #[serde(default)]
    pub name: Option<AppleName>,
    /// The address, which also appears in the id_token.
    #[serde(default)]
    pub email: Option<String>,
}

/// Apple's two-part name.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct AppleName {
    /// Given name.
    #[serde(default, rename = "firstName")]
    pub first_name: Option<String>,
    /// Family name.
    #[serde(default, rename = "lastName")]
    pub last_name: Option<String>,
}

impl FirstAuthorizationUser {
    /// Parses the raw `user` form field.
    ///
    /// # Errors
    /// Returns [`AuthError::ProviderResponse`] when the field is not the JSON
    /// Apple documents.
    pub fn parse(raw: &str) -> Result<Self, AuthError> {
        serde_json::from_str(raw).map_err(|e| AuthError::ProviderResponse {
            provider: "apple".into(),
            detail: format!("the `user` form field is not the documented JSON: {e}"),
        })
    }

    /// The two parts joined, or whichever part exists.
    pub fn display_name(&self) -> Option<String> {
        let name = self.name.as_ref()?;
        match (&name.first_name, &name.last_name) {
            (Some(f), Some(l)) => Some(format!("{f} {l}")),
            (Some(f), None) => Some(f.clone()),
            (None, Some(l)) => Some(l.clone()),
            (None, None) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_user_field_parses_and_joins() {
        let raw = r#"{"name":{"firstName":"Ada","lastName":"Lovelace"},"email":"ada@example.com"}"#;
        let user = FirstAuthorizationUser::parse(raw).unwrap();
        assert_eq!(user.display_name().as_deref(), Some("Ada Lovelace"));
        assert_eq!(user.email.as_deref(), Some("ada@example.com"));
    }

    #[test]
    fn a_later_callback_has_no_user_field_at_all_and_that_is_not_an_error() {
        // Apple simply omits it; the flow must treat the absence as normal,
        // which it does by making `CallbackParams::apple_user` an `Option`.
        let user = FirstAuthorizationUser::parse("{}").unwrap();
        assert_eq!(user.display_name(), None);
    }

    #[test]
    fn an_impossible_secret_lifetime_is_refused_before_signing() {
        let key = AppleKey {
            lifetime: MAX_SECRET_LIFETIME + 1,
            ..AppleKey::new("TEAM", "KEY", "com.example.app", "not a key")
        };
        let err = key
            .client_secret(&crate::clock::TestClock::default())
            .unwrap_err();
        assert!(matches!(err, AuthError::Misconfigured(_)), "{err:?}");
    }

    #[test]
    fn a_malformed_private_key_fails_at_the_key_rather_than_at_apple() {
        let key = AppleKey::new("TEAM", "KEY", "com.example.app", "-----BEGIN NONSENSE-----");
        let err = key
            .client_secret(&crate::clock::TestClock::default())
            .unwrap_err();
        assert!(matches!(err, AuthError::Crypto(_)), "{err:?}");
    }
}
