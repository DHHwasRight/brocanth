//! Providers as data: three URLs, how the client authenticates, and the
//! registration (client id, secret, scopes).
//!
//! [`endpoints`] is the small table golang.org/x/oauth2 keeps per
//! provider: GitHub, Google, Microsoft and GitLab. A custom provider is an
//! [`Endpoint`] written by hand.

/// How the client id and secret reach the token endpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AuthStyle {
    /// In the form body: `client_id` and `client_secret`. Every built-in
    /// provider accepts it.
    #[default]
    InParams,
    /// In an `Authorization: Basic` header (RFC 6749 §2.3.1).
    InHeader,
}

/// Where a provider answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    /// The page the browser goes to: the consent screen.
    pub auth_url: String,
    /// Where the code, a refresh token or the client credentials become a
    /// token.
    pub token_url: String,
    /// The RFC 7009 revocation endpoint, when the provider has one.
    pub revoke_url: Option<String>,
    /// How the client authenticates at `token_url`.
    pub auth_style: AuthStyle,
    /// Parameters the consent URL always carries.
    pub auth_params: Vec<(String, String)>,
}

impl Endpoint {
    /// An endpoint with an authorization and a token URL.
    pub fn new(auth_url: impl Into<String>, token_url: impl Into<String>) -> Self {
        Self {
            auth_url: auth_url.into(),
            token_url: token_url.into(),
            revoke_url: None,
            auth_style: AuthStyle::InParams,
            auth_params: Vec::new(),
        }
    }

    /// Sets the revocation URL.
    #[must_use]
    pub fn revoke_url(mut self, url: impl Into<String>) -> Self {
        self.revoke_url = Some(url.into());
        self
    }

    /// Sets how the client authenticates.
    #[must_use]
    pub fn auth_style(mut self, style: AuthStyle) -> Self {
        self.auth_style = style;
        self
    }

    /// Adds a parameter to every consent URL.
    #[must_use]
    pub fn auth_param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.auth_params.push((name.into(), value.into()));
        self
    }
}

/// The endpoints of well-known providers, like `golang.org/x/oauth2/<provider>`.
pub mod endpoints {
    use super::Endpoint;

    /// GitHub OAuth apps. GitHub has no RFC 7009 endpoint, so a disconnect
    /// only deletes the row; revoke the grant in GitHub's settings.
    pub fn github() -> Endpoint {
        Endpoint::new(
            "https://github.com/login/oauth/authorize",
            "https://github.com/login/oauth/access_token",
        )
    }

    /// Google. `access_type=offline` asks for a refresh token, and
    /// `prompt=consent` asks for one again on a second connect: without it,
    /// Google gives a refresh token only the first time.
    pub fn google() -> Endpoint {
        Endpoint::new(
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
        )
        .revoke_url("https://oauth2.googleapis.com/revoke")
        .auth_param("access_type", "offline")
        .auth_param("prompt", "consent")
    }

    /// Microsoft identity platform, for the `common` tenant. Ask for the
    /// `offline_access` scope to get a refresh token.
    pub fn microsoft() -> Endpoint {
        microsoft_tenant("common")
    }

    /// Microsoft identity platform, for one tenant: `common`,
    /// `organizations`, `consumers` or a tenant id.
    pub fn microsoft_tenant(tenant: &str) -> Endpoint {
        Endpoint::new(
            format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"),
            format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
        )
    }

    /// GitLab.com.
    pub fn gitlab() -> Endpoint {
        gitlab_at("https://gitlab.com")
    }

    /// A self-managed GitLab, by its base URL.
    pub fn gitlab_at(base: &str) -> Endpoint {
        let base = base.trim_end_matches('/');
        Endpoint::new(
            format!("{base}/oauth/authorize"),
            format!("{base}/oauth/token"),
        )
        .revoke_url(format!("{base}/oauth/revoke"))
    }
}

/// One provider as this application registered it.
#[derive(Clone)]
pub struct Provider {
    /// The id in URLs and in the store: `github`.
    pub id: String,
    /// The name people read: `GitHub`.
    pub name: String,
    /// Where it answers.
    pub endpoint: Endpoint,
    /// The client id at the provider.
    pub client_id: String,
    pub(crate) client_secret: String,
    /// The scopes a person grants when connecting.
    pub scopes: Vec<String>,
    /// The scopes of the application's own token (client credentials).
    pub app_scopes: Vec<String>,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("endpoint", &self.endpoint)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("scopes", &self.scopes)
            .field("app_scopes", &self.app_scopes)
            .finish()
    }
}

impl Provider {
    /// A provider: an id, a name, an endpoint and the registration.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        endpoint: Endpoint,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            endpoint,
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            scopes: Vec::new(),
            app_scopes: Vec::new(),
        }
    }

    /// GitHub, with id `github`.
    pub fn github(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self::new(
            "github",
            "GitHub",
            endpoints::github(),
            client_id,
            client_secret,
        )
    }

    /// Google, with id `google`.
    pub fn google(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self::new(
            "google",
            "Google",
            endpoints::google(),
            client_id,
            client_secret,
        )
    }

    /// Microsoft (`common` tenant), with id `microsoft`.
    pub fn microsoft(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self::new(
            "microsoft",
            "Microsoft",
            endpoints::microsoft(),
            client_id,
            client_secret,
        )
    }

    /// GitLab.com, with id `gitlab`.
    pub fn gitlab(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self::new(
            "gitlab",
            "GitLab",
            endpoints::gitlab(),
            client_id,
            client_secret,
        )
    }

    /// The built-in provider with this id, or `None`.
    pub fn builtin(
        id: &str,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Option<Self> {
        Some(match id {
            "github" => Self::github(client_id, client_secret),
            "google" => Self::google(client_id, client_secret),
            "microsoft" => Self::microsoft(client_id, client_secret),
            "gitlab" => Self::gitlab(client_id, client_secret),
            _ => return None,
        })
    }

    /// The scopes a person grants when connecting.
    #[must_use]
    pub fn scopes<S: Into<String>>(mut self, scopes: impl IntoIterator<Item = S>) -> Self {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// The scopes of the application's own token.
    #[must_use]
    pub fn app_scopes<S: Into<String>>(mut self, scopes: impl IntoIterator<Item = S>) -> Self {
        self.app_scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// Checks every URL: it parses, and it is `https`, or `http` when
    /// `allow_http` is on (development, a fake provider in a test).
    ///
    /// The token and revocation URLs carry the client secret and the
    /// tokens. Over `http`, anybody on the path reads them. And an `http`
    /// URL in production is most often an internal address, which is how a
    /// misconfigured provider becomes a request into your own network.
    ///
    /// # Errors
    /// [`crate::Error::Config`], naming the URL, with a `help:` line.
    pub fn check(&self, allow_http: bool) -> Result<(), crate::Error> {
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            return Err(crate::Error::Config(format!(
                "provider id `{}` is not a slug\nhelp: use lower-case letters, digits, `-` and `_`",
                self.id
            )));
        }
        let urls = [
            ("auth_url", Some(&self.endpoint.auth_url)),
            ("token_url", Some(&self.endpoint.token_url)),
            ("revoke_url", self.endpoint.revoke_url.as_ref()),
        ];
        for (name, url) in urls {
            let Some(url) = url else { continue };
            let parsed = url::Url::parse(url).map_err(|e| {
                crate::Error::Config(format!(
                    "`{}` {name} `{url}` is not a URL ({e})\nhelp: write the full URL, with https://",
                    self.id
                ))
            })?;
            match parsed.scheme() {
                "https" => {}
                "http" if allow_http => {}
                other => {
                    return Err(crate::Error::Config(format!(
                        "`{}` {name} `{url}` uses {other}, not https\n\
                         help: use the provider's https URL; only development may set \
                         `allow_http = true`",
                        self.id
                    )))
                }
            }
        }
        Ok(())
    }
}
