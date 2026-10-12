//! Authentication as a brocanth plugin.
//!
//! The arrow points this way and only this way: this crate depends on
//! `brocanth`, and `brocanth` has never heard of it. An application that
//! stores no accounts does not name [`AuthPlugin`] in its `main`, and then
//! argon2, `jsonwebtoken`, reqwest and everything below them are absent from
//! its dependency tree, absent from its binary and absent from its
//! configuration surface.
//!
//! # What it contributes, through the seams and not around them
//!
//! * **A context provider.** [`Auth`] goes into the per-request context, so a
//!   handler writes `let auth: &Auth<_, _, _> = cx.extract();` and a handler in
//!   an application that never registered the plugin does not compile.
//! * **Two middleware offers.** `auth.session` loads and refreshes the session
//!   cookie; `auth.require_authenticated` rejects a request without one. Both
//!   are *offered*. This plugin does not mount them, because mounting takes a
//!   pattern and the pattern is the application's decision — the rule the
//!   Crystal original's author set and the core enforces by giving
//!   `configure_router` a catalogue with no `mount`.
//!
//! Request hooks are what the middleware bodies need, and they land with the
//! core's HTTP layer. Until then the offers are names with descriptions, which
//! is honest: the plugin is waiting on the core rather than working around it.
//!
//! # Nothing is constructed behind your back
//!
//! [`AuthConfig`] takes the store, the HTTP client, the clock, the signing key
//! and the hasher as values you built. There is no default store — §8 forbids
//! assuming one — and no default signing key, because a key this crate
//! invented at startup would silently invalidate every session on restart and
//! differ between instances of a fleet.

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, MiddlewareCatalog, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};

use crate::accounts::Accounts;
use crate::clock::Clock;
use crate::error::AuthError;
use crate::http_client::HttpClient;
use crate::linking::Linking;
use crate::oauth::{ClientConfig, OauthClient};
use crate::password::Passwords;
use crate::providers::Provider;
use crate::session::{SessionConfig, Sessions, SigningKey};
use crate::storage::AuthStore;

/// One provider, configured for this application.
#[derive(Clone, Debug)]
pub struct ConfiguredProvider {
    /// The provider, from [`crate::providers::catalog`] or built by hand.
    pub provider: Provider,
    /// This application's registration at it.
    pub client: ClientConfig,
}

/// What [`AuthPlugin`] is built from.
///
/// Every field is a value the application constructed. That is the core's
/// rule — a plugin arrives already configured — and here it also means the
/// fallible parts (reading a signing key, choosing a password cost, opening a
/// pool) fail at startup, in the application's own code, with the
/// application's own error handling.
pub struct AuthConfig<S: AuthStore, H: HttpClient, C: Clock> {
    /// Where identities, sessions and tokens live.
    pub store: Arc<S>,
    /// How to reach a provider.
    pub http: Arc<H>,
    /// The clock.
    pub clock: Arc<C>,
    /// The key that signs session cookies and sealed flow states.
    pub signing_key: SigningKey,
    /// How sessions behave.
    pub session: SessionConfig,
    /// The hasher. argon2id, at a cost the application chose.
    pub passwords: Arc<Passwords>,
    /// The providers this application offers.
    pub providers: Vec<ConfiguredProvider>,
}

struct Inner<S: AuthStore, H: HttpClient, C: Clock> {
    accounts: Accounts<S, C>,
    sessions: Sessions<S, C>,
    linking: Linking<S, C>,
    oauth: OauthClient<H, C>,
    providers: Vec<ConfiguredProvider>,
}

/// The context item: everything a handler needs, behind one `Arc`.
///
/// Cloning is cheap and the clone shares the JWKS cache, which is the point —
/// a per-request copy of that cache would refetch a provider's keys on every
/// sign-in.
pub struct Auth<S: AuthStore, H: HttpClient, C: Clock>(Arc<Inner<S, H, C>>);

impl<S: AuthStore, H: HttpClient, C: Clock> Clone for Auth<S, H, C> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<S: AuthStore, H: HttpClient, C: Clock> std::fmt::Debug for Auth<S, H, C> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Auth")
            .field("providers", &self.provider_ids())
            .finish()
    }
}

impl<S: AuthStore, H: HttpClient, C: Clock> Auth<S, H, C> {
    /// Builds the services from a configuration, outside any application.
    ///
    /// Useful in a test, and it is what [`AuthPlugin`] does in its `configure`
    /// hook.
    ///
    /// # Errors
    /// Fails when the OS has no randomness for the enumeration sentinel.
    pub fn build(config: AuthConfig<S, H, C>) -> Result<Self, AuthError> {
        let accounts = Accounts::new(
            config.store.clone(),
            config.clock.clone(),
            config.passwords.clone(),
        )?;
        let sessions = Sessions::new(
            config.store.clone(),
            config.clock.clone(),
            config.signing_key.clone(),
            config.session.clone(),
        );
        let linking = Linking::new(config.store.clone(), config.clock.clone());
        let oauth = OauthClient::new(config.http.clone(), config.clock.clone());
        Ok(Self(Arc::new(Inner {
            accounts,
            sessions,
            linking,
            oauth,
            providers: config.providers,
        })))
    }

    /// Registration, login, reset and verification.
    pub fn accounts(&self) -> &Accounts<S, C> {
        &self.0.accounts
    }

    /// Sessions.
    pub fn sessions(&self) -> &Sessions<S, C> {
        &self.0.sessions
    }

    /// Identity linking.
    pub fn linking(&self) -> &Linking<S, C> {
        &self.0.linking
    }

    /// The OAuth and OIDC flow.
    pub fn oauth(&self) -> &OauthClient<H, C> {
        &self.0.oauth
    }

    /// A configured provider by id.
    pub fn provider(&self, id: &str) -> Option<&ConfiguredProvider> {
        self.0.providers.iter().find(|p| p.provider.id == id)
    }

    /// Every configured provider, for rendering the sign-in buttons.
    pub fn providers(&self) -> &[ConfiguredProvider] {
        &self.0.providers
    }

    fn provider_ids(&self) -> Vec<&str> {
        self.0
            .providers
            .iter()
            .map(|p| p.provider.id.as_ref())
            .collect()
    }
}

impl<S: AuthStore, H: HttpClient, C: Clock> ContextItem for Auth<S, H, C> {}

/// The plugin.
///
/// ```no_run
/// use std::sync::Arc;
/// use brocanth::plugin_api::Plugin;
/// use brocanth::plugin_app::AppBuilder;
/// use brocanth_auth::{AuthPlugin, MemoryStore, Passwords, SystemClock};
/// use brocanth_auth::plugin::{AuthConfig, ConfiguredProvider};
/// use brocanth_auth::oauth::ClientConfig;
/// use brocanth_auth::providers::catalog::GOOGLE;
/// use brocanth_auth::session::{SessionConfig, SigningKey};
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// # let http = Arc::new(brocanth_auth::http_client::StubHttp::new());
/// let app = AppBuilder::new()
///     .plugin(AuthPlugin::new(AuthConfig {
///         store: Arc::new(MemoryStore::new()),
///         http,
///         clock: Arc::new(SystemClock),
///         signing_key: SigningKey::parse(&std::env::var("SESSION_KEY")?)?,
///         session: SessionConfig::default(),
///         passwords: Arc::new(Passwords::recommended()?),
///         providers: vec![ConfiguredProvider {
///             provider: GOOGLE,
///             client: ClientConfig::new(
///                 std::env::var("GOOGLE_CLIENT_ID")?,
///                 std::env::var("GOOGLE_CLIENT_SECRET")?,
///                 "https://example.test/auth/google/callback",
///             ),
///         }],
///     }))
///     .setup(|app| {
///         // The application mounts, at a pattern it chose. The plugin cannot.
///         app.router().mount("/*", "auth.session");
///         app.router().mount("/admin/*", "auth.require_authenticated");
///         Ok(())
///     })
///     .build()?;
/// # let _ = app;
/// # Ok(())
/// # }
/// ```
pub struct AuthPlugin<S: AuthStore, H: HttpClient, C: Clock> {
    config: Option<AuthConfig<S, H, C>>,
    auth: Option<Auth<S, H, C>>,
}

impl<S: AuthStore, H: HttpClient, C: Clock> AuthPlugin<S, H, C> {
    /// The built services, available from `after_config` onwards.
    pub fn auth(&self) -> Option<&Auth<S, H, C>> {
        self.auth.as_ref()
    }
}

impl<S: AuthStore, H: HttpClient, C: Clock> Plugin for AuthPlugin<S, H, C> {
    type Config = AuthConfig<S, H, C>;
    type Provides = ProvidesContext<Auth<S, H, C>>;
    const NAME: &'static str = "auth";

    fn new(config: Self::Config) -> Self {
        Self {
            config: Some(config),
            auth: None,
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let config = self
            .config
            .take()
            .ok_or_else(|| PluginError::new("the auth plugin was configured twice"))?;

        // Catch a misconfiguration at startup rather than at the first
        // sign-in: a provider whose client id is empty, or an Apple key that
        // does not parse, is a deployment mistake and belongs in the boot log.
        for configured in &config.providers {
            if configured.client.client_id.is_empty() {
                return Err(PluginError::new(format!(
                    "provider `{}` has an empty client id",
                    configured.provider.id
                )));
            }
            if let crate::oauth::ClientSecret::Apple(key) = &configured.client.secret {
                key.client_secret(config.clock.as_ref())
                    .map_err(|e| PluginError::new(e.to_string()))?;
            }
        }

        self.auth =
            Some(Auth::build(config).map_err(|e| PluginError::new(e.to_string()))?);
        Ok(())
    }

    fn configure_router(
        &mut self,
        catalog: &mut MiddlewareCatalog<'_>,
    ) -> Result<(), PluginError> {
        catalog.offer(
            "auth.session",
            "reads the signed session cookie, refreshes its idle timer, and puts the \
             current account on the context",
        );
        catalog.offer(
            "auth.require_authenticated",
            "answers 401 when the request has no live session; mount it at the patterns \
             that need one",
        );
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        if let Some(auth) = &self.auth {
            cx.insert(auth.clone());
        }
    }
}
