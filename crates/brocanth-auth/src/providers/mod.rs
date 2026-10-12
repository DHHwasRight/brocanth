//! Providers as data.
//!
//! `markbates/goth` has one interface and roughly forty implementations of it,
//! and most of those implementations differ only in four URLs and the names of
//! three JSON fields. That is a table pretending to be a package. So here a
//! [`Provider`] is a value: endpoints, scopes, how the client authenticates to
//! the token endpoint, and a [`ProfileMap`] from the provider's own JSON to the
//! one [`ProviderIdentity`] the rest of the crate deals in. Adding a provider
//! is a `const` struct literal in [`catalog`] — no trait to implement, no
//! module, nothing for the flow code to dispatch on.
//!
//! The flow code in [`crate::oauth`] never names a provider. Whatever it has
//! to do differently is declared on the value, as a [`Quirk`], so the list of
//! special cases in this crate is a list you can read in one screen instead of
//! a behaviour spread across forty files.
//!
//! [`catalog`]: crate::providers::catalog

pub mod catalog;

use std::borrow::Cow;

use crate::error::AuthError;
use crate::identity::{EmailAddress, ProviderIdentity};

pub use catalog::{all, by_id, gitlab_self_hosted, microsoft_tenant};

/// Whether identity comes from a signed id_token or from an API call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    /// Plain OAuth 2.0. There is no id_token; the profile comes from an
    /// endpoint, and the access token is the only thing the provider signed.
    OAuth2,
    /// OpenID Connect. The id_token is a JWT this crate verifies against the
    /// provider's JWKS, and `nonce` is mandatory.
    Oidc {
        /// How to check the `iss` claim.
        issuer: IssuerCheck,
        /// Where the signing keys live.
        jwks_url: Cow<'static, str>,
    },
}

/// How to validate an id_token's `iss` claim.
///
/// Most providers have one issuer string. Microsoft does not: a multi-tenant
/// application authorises at `/common`, and the id_token comes back with
/// `iss = https://login.microsoftonline.com/{tenant}/v2.0` for whichever
/// tenant the user belongs to. Pinning the literal `/common/v2.0` rejects
/// every real token; not checking `iss` at all accepts a token minted for
/// somebody else's directory. The template is the only correct option, so it
/// is in the type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssuerCheck {
    /// The `iss` claim must equal this exactly.
    Exact(Cow<'static, str>),
    /// The `iss` claim must start with `prefix`, end with `suffix`, and have a
    /// non-empty tenant identifier between them.
    TenantTemplate {
        /// What comes before the tenant identifier.
        prefix: Cow<'static, str>,
        /// What comes after it.
        suffix: Cow<'static, str>,
    },
}

impl IssuerCheck {
    /// Whether `iss` is acceptable.
    pub fn accepts(&self, iss: &str) -> bool {
        match self {
            IssuerCheck::Exact(expected) => iss == expected.as_ref(),
            IssuerCheck::TenantTemplate { prefix, suffix } => {
                iss.len() > prefix.len() + suffix.len()
                    && iss.starts_with(prefix.as_ref())
                    && iss.ends_with(suffix.as_ref())
            }
        }
    }

    /// A representative issuer string, for an error message.
    pub fn describe(&self) -> String {
        match self {
            IssuerCheck::Exact(e) => e.to_string(),
            IssuerCheck::TenantTemplate { prefix, suffix } => format!("{prefix}<tenant>{suffix}"),
        }
    }
}

/// How the client proves who it is at the token endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthStyle {
    /// `client_id` and `client_secret` in the form body. What most providers
    /// want, and what some of them accept *only* in this form.
    ClientSecretPost,
    /// HTTP Basic, which is what RFC 6749 actually specifies.
    ClientSecretBasic,
    /// The `client_secret` is a JWT the client signs with its own private key.
    /// Apple, and nobody else in this catalogue. See [`crate::apple`].
    PrivateKeyJwt,
}

/// Whether, and how, the provider asserts that an address is verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VerifiedEmail {
    /// The provider never says. The identity is recorded as **unverified**,
    /// not as "probably fine" — [`crate::linking`] refuses to act on it, which
    /// is the entire point of recording it honestly.
    Never,
    /// Every address this provider returns is one it verified, and this crate
    /// takes its word for it. Only used where the provider documents it *and*
    /// the call that produced the address is the verified-addresses call.
    Always,
    /// A field on the profile. Accepts a JSON boolean or the strings `"true"`
    /// and `"false"`, because Apple sends the latter.
    Flag(&'static str),
}

/// Where each common field lives in this provider's profile JSON.
///
/// Each entry is a list of dotted paths, first match wins, so a provider that
/// moved a field or has two spellings is a second string rather than a code
/// change. `picture.data.url` works: a path segment indexes an object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProfileMap {
    /// The provider's stable identifier for the person. Required.
    pub subject: &'static [&'static str],
    /// The address, if any.
    pub email: &'static [&'static str],
    /// How the provider reports verification.
    pub email_verified: VerifiedEmail,
    /// A display name, if any.
    pub name: &'static [&'static str],
    /// An avatar URL, if any. Empty when the provider returns something that
    /// is not a URL.
    pub avatar: &'static [&'static str],
}

/// Something one provider does that the others do not.
///
/// Every special case in this crate is in this enum. That is the containment
/// strategy: a reader who wants to know where the exceptions are reads one
/// list, instead of grepping for provider names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Quirk {
    /// GitHub's `/user` returns the *public profile* email, which the user
    /// typed and nobody checked, and returns `null` when the profile email is
    /// private. The verified address lives at `/user/emails`, so this quirk
    /// makes the flow call it and take the entry that is both `primary` and
    /// `verified`. Without this, GitHub sign-in hands [`crate::linking`] an
    /// attacker-chosen address.
    GithubVerifiedEmails,

    /// The `client_secret` is an ES256 JWT this crate signs from a private
    /// key, and it expires. Apple. See [`crate::apple`].
    AppleClientSecretJwt,

    /// There is no userinfo endpoint: the profile is whatever the id_token
    /// carries. Apple.
    IdentityFromIdTokenOnly,

    /// The provider returns the user's name exactly once, in a form field on
    /// the first authorization callback, and never again. Apple. See
    /// [`crate::apple::FirstAuthorizationUser`].
    NameOnFirstAuthorizationOnly,

    /// The provider answers `200 OK` with `{"ok": false, "error": "..."}`
    /// instead of an HTTP error status. Slack. Without this, a failed token
    /// exchange is read as a successful one with every field missing.
    OkEnvelope,
}

/// Everything this crate needs to know about one provider.
///
/// `Cow` rather than `&'static str` so the catalogue entries stay `const`
/// while a self-hosted GitLab or a single-tenant Microsoft application can be
/// built at run time — see [`gitlab_self_hosted`] and [`microsoft_tenant`].
#[derive(Clone, Debug, PartialEq)]
pub struct Provider {
    /// Stable identifier, stored on every [`crate::identity::ProviderLink`].
    pub id: Cow<'static, str>,
    /// What to call it on a button.
    pub display_name: Cow<'static, str>,
    /// OAuth 2.0 or OIDC.
    pub kind: ProviderKind,
    /// Where to send the browser.
    pub authorize_url: Cow<'static, str>,
    /// Where to exchange the code and refresh the token.
    pub token_url: Cow<'static, str>,
    /// Where to read the profile, when there is such a place.
    pub userinfo_url: Option<Cow<'static, str>>,
    /// Scopes requested when the application does not ask for others.
    pub default_scopes: &'static [&'static str],
    /// How the client authenticates at the token endpoint.
    pub auth_style: AuthStyle,
    /// Parameters this provider requires on the authorization URL.
    pub extra_authorize_params: &'static [(&'static str, &'static str)],
    /// Where the common fields live in the profile JSON.
    pub profile: ProfileMap,
    /// What this provider does differently.
    pub quirks: &'static [Quirk],
}

impl Provider {
    /// Whether this provider has a given quirk.
    pub fn has(&self, quirk: Quirk) -> bool {
        self.quirks.contains(&quirk)
    }

    /// Whether identity arrives in a signed id_token.
    pub fn is_oidc(&self) -> bool {
        matches!(self.kind, ProviderKind::Oidc { .. })
    }

    /// Turns a profile document into the common identity.
    ///
    /// # Errors
    /// Returns [`AuthError::ProfileFieldMissing`] when the subject is absent,
    /// which means the mapping is wrong or the provider changed its response.
    /// Every other field is optional.
    pub fn map_profile(&self, raw: &serde_json::Value) -> Result<ProviderIdentity, AuthError> {
        let subject = first_string(raw, self.profile.subject).ok_or_else(|| {
            AuthError::ProfileFieldMissing {
                provider: self.id.to_string(),
                field: "subject",
            }
        })?;

        // An address the provider sent but that is not a usable address is
        // dropped rather than fatal: the sign-in still works, the account just
        // has no email, and that is a better outcome than refusing to let
        // somebody in because their directory has a malformed entry.
        let email = first_string(raw, self.profile.email)
            .and_then(|e| EmailAddress::parse(&e).ok());

        let email_verified = email.is_some() && self.verified_flag(raw);

        Ok(ProviderIdentity {
            provider: self.id.to_string(),
            subject,
            email,
            email_verified,
            name: first_string(raw, self.profile.name),
            avatar_url: first_string(raw, self.profile.avatar),
            raw: raw.clone(),
        })
    }

    fn verified_flag(&self, raw: &serde_json::Value) -> bool {
        match self.profile.email_verified {
            VerifiedEmail::Never => false,
            VerifiedEmail::Always => true,
            VerifiedEmail::Flag(path) => match lookup(raw, path) {
                Some(serde_json::Value::Bool(b)) => *b,
                // Apple sends `"email_verified": "true"`. A string is not a
                // boolean and a careless `is_truthy` would read `"false"` as
                // verified, so the two literals are spelled out.
                Some(serde_json::Value::String(s)) => s == "true",
                _ => false,
            },
        }
    }
}

/// Resolves a dotted path in a JSON document.
pub fn lookup<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn first_string(value: &serde_json::Value, paths: &[&str]) -> Option<String> {
    for path in paths {
        match lookup(value, path) {
            Some(serde_json::Value::String(s)) if !s.is_empty() => return Some(s.clone()),
            // GitHub, Discord and Facebook all number their users, and a
            // numeric `id` is still the subject.
            Some(serde_json::Value::Number(n)) => return Some(n.to_string()),
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_nested_path_resolves() {
        let doc = json!({"picture": {"data": {"url": "https://example.test/a.png"}}});
        assert_eq!(
            first_string(&doc, &["picture.data.url"]).as_deref(),
            Some("https://example.test/a.png")
        );
    }

    #[test]
    fn a_numeric_id_is_still_a_subject() {
        let p = catalog::GITHUB;
        let id = p.map_profile(&json!({"id": 4242, "login": "octocat"})).unwrap();
        assert_eq!(id.subject, "4242");
    }

    #[test]
    fn apples_string_valued_email_verified_is_read_correctly() {
        let p = catalog::APPLE;
        let yes = p
            .map_profile(&json!({"sub": "1", "email": "a@b.com", "email_verified": "true"}))
            .unwrap();
        let no = p
            .map_profile(&json!({"sub": "1", "email": "a@b.com", "email_verified": "false"}))
            .unwrap();
        assert!(yes.email_verified);
        assert!(!no.email_verified, "the string `false` is not verified");
    }

    #[test]
    fn a_provider_that_never_asserts_verification_reports_unverified() {
        let id = catalog::MICROSOFT
            .map_profile(&json!({"sub": "x", "email": "a@b.com", "name": "A"}))
            .unwrap();
        assert!(!id.email_verified);
    }

    #[test]
    fn microsofts_tenant_issuer_template_accepts_a_tenant_and_refuses_a_stranger() {
        let ProviderKind::Oidc { issuer, .. } = &catalog::MICROSOFT.kind else {
            panic!("microsoft is OIDC");
        };
        assert!(issuer.accepts("https://login.microsoftonline.com/9188040d-abc/v2.0"));
        assert!(!issuer.accepts("https://login.microsoftonline.com/v2.0"));
        assert!(!issuer.accepts("https://evil.test/9188040d-abc/v2.0"));
    }

    #[test]
    fn every_catalogue_entry_has_a_subject_mapping_and_a_unique_id() {
        let mut ids: Vec<&str> = all().iter().map(|p| p.id.as_ref()).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "two providers share an id");
        for p in all() {
            assert!(!p.profile.subject.is_empty(), "{} has no subject path", p.id);
            assert!(p.authorize_url.starts_with("https://"), "{}", p.id);
            assert!(p.token_url.starts_with("https://"), "{}", p.id);
        }
    }

    #[test]
    fn every_oidc_provider_can_fetch_keys_and_every_oauth2_provider_has_a_userinfo() {
        for p in all() {
            match &p.kind {
                ProviderKind::Oidc { jwks_url, .. } => {
                    assert!(jwks_url.starts_with("https://"), "{}", p.id)
                }
                ProviderKind::OAuth2 => assert!(
                    p.userinfo_url.is_some(),
                    "{} is plain OAuth2 and has nowhere to read a profile",
                    p.id
                ),
            }
        }
    }
}
