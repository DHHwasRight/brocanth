//! The providers people actually use, as `const` values.
//!
//! Adding one is a struct literal below and an entry in [`all`]. There is no
//! trait to implement and nothing to register, which is the whole claim this
//! module exists to make good on.
//!
//! Endpoint URLs are the ones each provider documents at the time of writing.
//! A provider that moves an endpoint moves a string here.

use std::borrow::Cow;

use super::{AuthStyle, IssuerCheck, ProfileMap, Provider, ProviderKind, Quirk, VerifiedEmail};

const fn s(text: &'static str) -> Cow<'static, str> {
    Cow::Borrowed(text)
}

// ---------------------------------------------------------------------------
// Google
// ---------------------------------------------------------------------------

/// Google, over OIDC.
///
/// `access_type=offline` is *not* among the default parameters: it is what
/// makes Google return a refresh token, and it also makes Google stop
/// returning one on every later authorization unless `prompt=consent` is also
/// sent. An application that wants offline access asks for it explicitly
/// rather than discovering the second half of that rule in production.
pub const GOOGLE: Provider = Provider {
    id: s("google"),
    display_name: s("Google"),
    kind: ProviderKind::Oidc {
        issuer: IssuerCheck::Exact(s("https://accounts.google.com")),
        jwks_url: s("https://www.googleapis.com/oauth2/v3/certs"),
    },
    authorize_url: s("https://accounts.google.com/o/oauth2/v2/auth"),
    token_url: s("https://oauth2.googleapis.com/token"),
    userinfo_url: Some(s("https://openidconnect.googleapis.com/v1/userinfo")),
    default_scopes: &["openid", "email", "profile"],
    auth_style: AuthStyle::ClientSecretPost,
    extra_authorize_params: &[],
    profile: ProfileMap {
        subject: &["sub"],
        email: &["email"],
        email_verified: VerifiedEmail::Flag("email_verified"),
        name: &["name", "given_name"],
        avatar: &["picture"],
    },
    quirks: &[],
};

// ---------------------------------------------------------------------------
// GitHub
// ---------------------------------------------------------------------------

/// GitHub, over plain OAuth 2.0.
///
/// GitHub has no OIDC for user sign-in, so there is no id_token and no nonce;
/// the `state` check and PKCE are the whole defence, which is why neither is
/// optional here.
///
/// The profile mapping deliberately does **not** list `email`. `/user` returns
/// the *public profile* email, which the user typed into a form and nobody
/// checked, and `null` when they kept it private. Taking it would mean
/// accepting an attacker-chosen address. [`Quirk::GithubVerifiedEmails`] makes
/// the flow call `/user/emails` and use the entry that is both `primary` and
/// `verified`, which is why `email_verified` is [`VerifiedEmail::Always`]: by
/// the time the mapping runs, the only address in the document is one GitHub
/// confirmed.
pub const GITHUB: Provider = Provider {
    id: s("github"),
    display_name: s("GitHub"),
    kind: ProviderKind::OAuth2,
    authorize_url: s("https://github.com/login/oauth/authorize"),
    token_url: s("https://github.com/login/oauth/access_token"),
    userinfo_url: Some(s("https://api.github.com/user")),
    default_scopes: &["read:user", "user:email"],
    auth_style: AuthStyle::ClientSecretPost,
    extra_authorize_params: &[],
    profile: ProfileMap {
        subject: &["id"],
        // Filled in by the `/user/emails` call, not read from `/user`.
        email: &["verified_primary_email"],
        email_verified: VerifiedEmail::Always,
        name: &["name", "login"],
        avatar: &["avatar_url"],
    },
    quirks: &[Quirk::GithubVerifiedEmails],
};

// ---------------------------------------------------------------------------
// GitLab
// ---------------------------------------------------------------------------

/// GitLab.com, over OIDC.
///
/// For a self-hosted instance use [`gitlab_self_hosted`]: the paths are the
/// same, only the host changes, which is the clearest demonstration in this
/// file that a provider is data.
pub const GITLAB: Provider = Provider {
    id: s("gitlab"),
    display_name: s("GitLab"),
    kind: ProviderKind::Oidc {
        issuer: IssuerCheck::Exact(s("https://gitlab.com")),
        jwks_url: s("https://gitlab.com/oauth/discovery/keys"),
    },
    authorize_url: s("https://gitlab.com/oauth/authorize"),
    token_url: s("https://gitlab.com/oauth/token"),
    userinfo_url: Some(s("https://gitlab.com/oauth/userinfo")),
    default_scopes: &["openid", "email", "profile"],
    auth_style: AuthStyle::ClientSecretPost,
    extra_authorize_params: &[],
    profile: ProfileMap {
        subject: &["sub"],
        email: &["email"],
        email_verified: VerifiedEmail::Flag("email_verified"),
        name: &["name", "nickname"],
        avatar: &["picture"],
    },
    quirks: &[],
};

/// A self-hosted GitLab at `base_url` (no trailing slash).
///
/// The returned provider keeps the id `gitlab`, so links made against
/// gitlab.com and links made against an internal instance are the same
/// provider as far as storage is concerned. Pass a different `id` by editing
/// the returned value if an application needs both at once.
pub fn gitlab_self_hosted(base_url: &str) -> Provider {
    let base = base_url.trim_end_matches('/');
    Provider {
        kind: ProviderKind::Oidc {
            issuer: IssuerCheck::Exact(Cow::Owned(base.to_string())),
            jwks_url: Cow::Owned(format!("{base}/oauth/discovery/keys")),
        },
        authorize_url: Cow::Owned(format!("{base}/oauth/authorize")),
        token_url: Cow::Owned(format!("{base}/oauth/token")),
        userinfo_url: Some(Cow::Owned(format!("{base}/oauth/userinfo"))),
        ..GITLAB
    }
}

// ---------------------------------------------------------------------------
// Microsoft
// ---------------------------------------------------------------------------

/// Microsoft identity platform, multi-tenant, over OIDC.
///
/// Two things about this entry are not like the others.
///
/// The issuer is a template. A `/common` application receives id_tokens whose
/// `iss` names the user's own directory, so there is no single string to pin —
/// see [`IssuerCheck::TenantTemplate`]. For a single-tenant application use
/// [`microsoft_tenant`], which pins the exact issuer and is strictly safer.
///
/// `email_verified` is [`VerifiedEmail::Never`] because Microsoft does not
/// emit that claim. The `email` claim on a work or school account is whatever
/// the directory administrator put there, and on a personal account it is the
/// sign-in name. Neither is a proof this crate may act on, so Microsoft
/// sign-in never auto-creates an account against an existing address — which
/// is exactly what [`crate::linking`] enforces anyway.
pub const MICROSOFT: Provider = Provider {
    id: s("microsoft"),
    display_name: s("Microsoft"),
    kind: ProviderKind::Oidc {
        issuer: IssuerCheck::TenantTemplate {
            prefix: s("https://login.microsoftonline.com/"),
            suffix: s("/v2.0"),
        },
        jwks_url: s("https://login.microsoftonline.com/common/discovery/v2.0/keys"),
    },
    authorize_url: s("https://login.microsoftonline.com/common/oauth2/v2.0/authorize"),
    token_url: s("https://login.microsoftonline.com/common/oauth2/v2.0/token"),
    userinfo_url: Some(s("https://graph.microsoft.com/oidc/userinfo")),
    default_scopes: &["openid", "email", "profile", "offline_access"],
    auth_style: AuthStyle::ClientSecretPost,
    extra_authorize_params: &[],
    profile: ProfileMap {
        subject: &["sub", "oid"],
        email: &["email", "preferred_username", "upn"],
        email_verified: VerifiedEmail::Never,
        name: &["name"],
        avatar: &[],
    },
    quirks: &[],
};

/// Microsoft, pinned to one tenant.
///
/// Prefer this whenever the application serves one directory: it replaces the
/// tenant template with an exact issuer, so a token minted by a different
/// directory is rejected rather than accepted as a stranger from somewhere.
pub fn microsoft_tenant(tenant: &str) -> Provider {
    Provider {
        kind: ProviderKind::Oidc {
            issuer: IssuerCheck::Exact(Cow::Owned(format!(
                "https://login.microsoftonline.com/{tenant}/v2.0"
            ))),
            jwks_url: Cow::Owned(format!(
                "https://login.microsoftonline.com/{tenant}/discovery/v2.0/keys"
            )),
        },
        authorize_url: Cow::Owned(format!(
            "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"
        )),
        token_url: Cow::Owned(format!(
            "https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"
        )),
        ..MICROSOFT
    }
}

// ---------------------------------------------------------------------------
// Apple — the one that does not fit. See `crate::apple`.
// ---------------------------------------------------------------------------

/// Sign in with Apple, over OIDC, and with three quirks nobody else has.
///
/// 1. **The client secret is a JWT you sign.** Apple issues no static secret.
///    You register a key, download a `.p8` ES256 private key once, and mint a
///    short-lived JWT per token request. See [`crate::apple::AppleKey`].
/// 2. **There is no userinfo endpoint.** `userinfo_url` is `None` and the
///    whole profile is the id_token's claims.
/// 3. **The name arrives exactly once.** Apple posts a `user` form field on
///    the *first* authorization only. See
///    [`crate::apple::FirstAuthorizationUser`], and store it then or lose it.
///
/// `response_mode=form_post` is mandatory once `name` or `email` is in the
/// scopes, which also means the callback is a `POST` and a `SameSite=Strict`
/// session cookie would not be sent with it. [`crate::session::SessionConfig`]
/// defaults to `Lax` for this reason.
pub const APPLE: Provider = Provider {
    id: s("apple"),
    display_name: s("Apple"),
    kind: ProviderKind::Oidc {
        issuer: IssuerCheck::Exact(s("https://appleid.apple.com")),
        jwks_url: s("https://appleid.apple.com/auth/keys"),
    },
    authorize_url: s("https://appleid.apple.com/auth/authorize"),
    token_url: s("https://appleid.apple.com/auth/token"),
    userinfo_url: None,
    default_scopes: &["name", "email"],
    auth_style: AuthStyle::PrivateKeyJwt,
    extra_authorize_params: &[("response_mode", "form_post")],
    profile: ProfileMap {
        subject: &["sub"],
        email: &["email"],
        // A JSON string, not a boolean. See `Provider::verified_flag`.
        email_verified: VerifiedEmail::Flag("email_verified"),
        // Never present in the id_token. The flow fills it from the `user`
        // form field on a first authorization, and from nothing afterwards.
        name: &["name"],
        avatar: &[],
    },
    quirks: &[
        Quirk::AppleClientSecretJwt,
        Quirk::IdentityFromIdTokenOnly,
        Quirk::NameOnFirstAuthorizationOnly,
    ],
};

// ---------------------------------------------------------------------------
// Discord
// ---------------------------------------------------------------------------

/// Discord, over plain OAuth 2.0.
///
/// `avatar` is not mapped: Discord returns an image *hash*, not a URL, and the
/// CDN path an application builds from it depends on whether the user has an
/// animated avatar. Mapping it would put a broken URL in
/// [`crate::identity::ProviderIdentity::avatar_url`]; the hash is in
/// [`crate::identity::ProviderIdentity::raw`] for an application that wants it.
pub const DISCORD: Provider = Provider {
    id: s("discord"),
    display_name: s("Discord"),
    kind: ProviderKind::OAuth2,
    authorize_url: s("https://discord.com/oauth2/authorize"),
    token_url: s("https://discord.com/api/oauth2/token"),
    userinfo_url: Some(s("https://discord.com/api/users/@me")),
    default_scopes: &["identify", "email"],
    auth_style: AuthStyle::ClientSecretPost,
    extra_authorize_params: &[],
    profile: ProfileMap {
        subject: &["id"],
        email: &["email"],
        email_verified: VerifiedEmail::Flag("verified"),
        name: &["global_name", "username"],
        avatar: &[],
    },
    quirks: &[],
};

// ---------------------------------------------------------------------------
// Slack
// ---------------------------------------------------------------------------

/// Sign in with Slack, over OIDC.
///
/// [`Quirk::OkEnvelope`]: Slack's Web API answers `200 OK` and puts the
/// failure in the body as `{"ok": false, "error": "..."}`. Reading only the
/// status turns a refused exchange into a successful one with no fields,
/// which then fails much later with a confusing message about a missing
/// subject.
pub const SLACK: Provider = Provider {
    id: s("slack"),
    display_name: s("Slack"),
    kind: ProviderKind::Oidc {
        issuer: IssuerCheck::Exact(s("https://slack.com")),
        jwks_url: s("https://slack.com/openid/connect/keys"),
    },
    authorize_url: s("https://slack.com/openid/connect/authorize"),
    token_url: s("https://slack.com/api/openid.connect.token"),
    userinfo_url: Some(s("https://slack.com/api/openid.connect.userInfo")),
    default_scopes: &["openid", "email", "profile"],
    auth_style: AuthStyle::ClientSecretPost,
    extra_authorize_params: &[],
    profile: ProfileMap {
        subject: &["sub"],
        email: &["email"],
        email_verified: VerifiedEmail::Flag("email_verified"),
        name: &["name", "https://slack.com/user_name"],
        avatar: &["picture", "https://slack.com/user_image_192"],
    },
    quirks: &[Quirk::OkEnvelope],
};

// ---------------------------------------------------------------------------
// Facebook
// ---------------------------------------------------------------------------

/// Facebook Login, over plain OAuth 2.0.
///
/// `email_verified` is [`VerifiedEmail::Never`]: Graph returns an address but
/// asserts nothing about it, and an address a user can change without
/// re-proving is not a proof. The version number is in the URL because Graph
/// versions expire; moving to the next one is a string change here.
pub const FACEBOOK: Provider = Provider {
    id: s("facebook"),
    display_name: s("Facebook"),
    kind: ProviderKind::OAuth2,
    authorize_url: s("https://www.facebook.com/v21.0/dialog/oauth"),
    token_url: s("https://graph.facebook.com/v21.0/oauth/access_token"),
    userinfo_url: Some(s(
        "https://graph.facebook.com/v21.0/me?fields=id,name,email,picture",
    )),
    default_scopes: &["public_profile", "email"],
    auth_style: AuthStyle::ClientSecretPost,
    extra_authorize_params: &[],
    profile: ProfileMap {
        subject: &["id"],
        email: &["email"],
        email_verified: VerifiedEmail::Never,
        name: &["name"],
        avatar: &["picture.data.url"],
    },
    quirks: &[],
};

// ---------------------------------------------------------------------------

/// Every provider shipped with this crate.
pub fn all() -> &'static [Provider] {
    // A `static` rather than a `const`, so the slice has one address and
    // `by_id` can hand out `&'static Provider`.
    static ALL: std::sync::OnceLock<Vec<Provider>> = std::sync::OnceLock::new();
    ALL.get_or_init(|| {
        vec![
            GOOGLE, GITHUB, GITLAB, MICROSOFT, APPLE, DISCORD, SLACK, FACEBOOK,
        ]
    })
}

/// Looks a shipped provider up by id.
///
/// An application with its own provider does not need this: it holds the
/// [`Provider`] value directly, which is the point of providers being data.
pub fn by_id(id: &str) -> Option<&'static Provider> {
    all().iter().find(|p| p.id == id)
}
