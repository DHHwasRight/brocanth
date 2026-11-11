//! The modules `brocanth create` offers: `oauth`, `oauth-client` and
//! `oauth-client-memory`.
//!
//! `oauth` is the parent: the crate, and the rule about tokens. A key with
//! a hyphen must name its parent (the CLI's naming test), so `oauth-client`
//! sits inside `oauth`, the way `webhooks-out` sits inside `webhooks`.
//!
//! * `oauth-client` writes `src/plugins/oauth_client/mod.rs`: the settings,
//!   the providers and their scopes, a plugin that builds the client over
//!   the `db` module's pool, and `mount`, which puts the connect pages into
//!   the route table. It needs `auth`, because a connection belongs to a
//!   signed-in account.
//! * `oauth-client-memory` keeps the tokens in memory instead, for tests
//!   and development.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::{
    configurable, integration, module_ref, redact_secrets, settings_derive, PLUGIN_TRAIT_USE,
};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Opt, Project, Scaffold, Setting, Slot};

/// The built-in providers this module declares settings for.
pub const PROVIDERS: &[&str] = &["github", "google", "microsoft", "gitlab"];

/// The parent: the crate and the rule about tokens.
pub struct Oauth;

impl Module for Oauth {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "oauth",
            "an OAuth 2.0 client for third-party APIs (not sign-in); pick its parts below",
        )
        .category(Category::Enhancer)
        .asked_in(
            "doors",
            "Call GitHub, Google, Microsoft or GitLab APIs for your users",
        )
        .krate(Dep::brocanth("brocanth-oauth-client"))
        // The key, not the crate name: `brocanth-oauth-client` is wider
        // than every other name in the list, and would realign it in
        // every project.
        .absent(
            "oauth",
            "no OAuth client; a call to a third-party API carries a token you manage by hand",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

const AGENTS: &str = "\n\
    ## Third-party APIs\n\
    \n\
    Call a provider's API through `OauthClient::http(account, provider)`, or\n\
    `OauthClient::app_http(provider)` for the application's own token. They\n\
    refresh an expired token first and retry once after a 401. Do not read\n\
    a token out of the store, and never put one in a log, a page, an error or\n\
    a URL: an access token is a password for that person's account.\n";

/// The connect pages, the token store and the client.
pub struct OauthClient;

impl Module for OauthClient {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "oauth-client",
            "connect pages at /connections; tokens sealed in Postgres, refreshed once, sent as a bearer",
        )
        .category(Category::Routes)
        .category(Category::Enhancer)
        .asked_in("doors", "Connect accounts and call their APIs")
        .part_of("oauth")
        .because("it is the client of `brocanth-oauth-client`, which the parent brings")
        .needs(
            "auth",
            "a connection belongs to a signed-in account, and `auth` holds the sessions",
        )
        .needs(
            "db",
            "the tokens are rows in the `db` module's database; a restart must not drop them",
        )
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "middleware-csrf",
            "the Connect and Disconnect forms carry the CSRF token",
        )
        .adapts_to(
            "oauth-client-memory",
            "the tokens are kept in memory, and [oauth_client] has no migrate",
        )
        .integration(
            "admin",
            "a tab at /admin/connections: every connection without its tokens, and a Revoke button",
            true,
        )
        .option(Opt::text(
            "mount_at",
            "/connections",
            "Where the connect pages answer. The redirect URI at each provider is <base_url><mount_at>/<provider>/callback.",
        ))
        .krate(
            Dep::brocanth("brocanth-oauth-client")
                .feature_unless("oauth-client-memory", "postgres")
                .feature_with("admin", "admin"),
        )
        .after("auth")
        .after("db")
        .after("config")
        .absent(
            "oauth-client",
            "no connected accounts; nothing calls a provider's API for a person",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file(
            "src/plugins/oauth_client/mod.rs",
            redact_secrets(
                &plugin_rs(project),
                &settings(project.has("oauth-client-memory")),
            ),
        );
        s.at(Slot::PluginsMod).line("pub mod oauth_client;");
        configurable(
            s,
            project,
            "oauth_client",
            "OauthClientPlugin",
            settings(project.has("oauth-client-memory")),
        );
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        s.at(Slot::RouterMounts).line(format!(
            "    crate::plugins::oauth_client::mount(routes, {})?;",
            module_ref("oauth-client", "mount_at")
        ));
        s.at(Slot::PagesRoutes).line("/connections");
        s.at(Slot::PagesRoutes).line("/connections/{*rest}");
        if project.has("admin") {
            integration(s, "oauth-client", "admin", Slot::AdminPanels, ADMIN_TAB);
        }
        s.at(Slot::AgentsSections).block(CLIENT_AGENTS.to_string());
    }
}

const ADMIN_TAB: &str = "// Every connection, without its tokens, with a Revoke button, as a tab.
config = config.panel(brocanth_oauth_client::admin::ConnectionsPanel::installed());";

const CLIENT_AGENTS: &str = "\n\
    ## Connected accounts\n\
    \n\
    `src/plugins/oauth_client/mod.rs` mounts the connect pages at\n\
    `/connections`. A Connect button is a form that posts to\n\
    `/connections/<provider>`, never a link: a GET that starts a connection\n\
    can be fired by any page on the web. Pass `return_to` only as a path on\n\
    this site; anything else is dropped.\n\
    \n\
    When a call fails with `brocanth_oauth_client::Error::Reconnect`, the\n\
    grant is gone: send the person to `/connections` to connect again. Do\n\
    not retry it. The scopes are in `scopes()` in that file: ask for the\n\
    fewest the code needs.\n\
    \n\
    The key that seals the tokens goes in `$<APP>_OAUTH_CLIENT__KEY`, and a\n\
    client secret in `$<APP>_OAUTH_CLIENT__<PROVIDER>_CLIENT_SECRET`, never in\n\
    a committed config file. Changing the key makes every stored token\n\
    unreadable.\n";

/// The tokens in memory, for tests and development.
pub struct OauthClientMemory;

impl Module for OauthClientMemory {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "oauth-client-memory",
            "the connected accounts in this process's memory; for tests and development, gone at restart",
        )
        .category(Category::Enhancer)
        .asked_in("doors", "")
        .part_of_but_off("oauth")
        .because("it is where `oauth-client` keeps its tokens")
        .needs(
            "oauth-client",
            "it replaces the token store of `oauth-client`, and there is nothing to store without it",
        )
        .after("oauth-client")
        .development_only(
            "the tokens live in the process: a restart drops every connection, \
             and two processes do not share one",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // `oauth-client` reads `project.has("oauth-client-memory")` and
        // writes the memory store. This module adds the note.
        s.at(Slot::AgentsSections).block(MEMORY_AGENTS.to_string());
    }
}

const MEMORY_AGENTS: &str = "\n\
    ## Connected accounts in memory\n\
    \n\
    The tokens live in this process (`oauth-client-memory`). A restart drops\n\
    every connection, and two processes do not share them. It is for\n\
    development and tests: `brocanth check --deploy` refuses it. Run\n\
    `brocanth remove oauth-client-memory` before production; the tokens then\n\
    go to Postgres.\n";

fn settings(memory: bool) -> Vec<Setting> {
    let mut out = vec![
        Setting::list(
            "providers",
            &["github"],
            "The providers to offer, by id: github, google, microsoft, gitlab.",
        )
        .note(
            "Each one needs its client id and secret below. The server refuses to\n\
             start when one is missing, so a Connect button never answers an error.",
        ),
        Setting::text(
            "base_url",
            "http://localhost:3000",
            "The scheme and host the providers send the browser back to.",
        )
        .note("Register <base_url>/connections/<provider>/callback at each provider's console."),
        Setting::text(
            "key",
            "",
            "The key that seals the tokens: 32 random bytes, base64url. No flag: it is a secret.",
        )
        .note(
            "Make one with: head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '='\n\
             Set it in the environment. A new key makes every stored token unreadable.",
        )
        .optional()
        .secret(),
        Setting::bool(
            "allow_http",
            false,
            "Accept http:// provider URLs. Development only, for a local fake provider.",
        ),
    ];
    if !memory {
        out.push(
            Setting::bool(
                "migrate",
                true,
                "Create the token table on the first readiness check.",
            )
            .note(
                "Off, for a project that keeps brocanth_oauth_client::PgTokenStore::SCHEMA in a migration.",
            ),
        );
    }
    for id in PROVIDERS {
        out.push(Setting::text(client_id_key(id), "", client_id_help(id)).optional());
        out.push(
            Setting::text(client_secret_key(id), "", client_secret_help(id))
                .optional()
                .secret(),
        );
    }
    out
}

fn client_id_key(id: &str) -> &'static str {
    match id {
        "github" => "github_client_id",
        "google" => "google_client_id",
        "microsoft" => "microsoft_client_id",
        _ => "gitlab_client_id",
    }
}

fn client_secret_key(id: &str) -> &'static str {
    match id {
        "github" => "github_client_secret",
        "google" => "google_client_secret",
        "microsoft" => "microsoft_client_secret",
        _ => "gitlab_client_secret",
    }
}

fn client_id_help(id: &str) -> &'static str {
    match id {
        "github" => "The client id of the GitHub OAuth app.",
        "google" => "The client id of the Google OAuth client.",
        "microsoft" => "The application (client) id of the Microsoft Entra app.",
        _ => "The application id of the GitLab application.",
    }
}

fn client_secret_help(id: &str) -> &'static str {
    match id {
        "github" => "The client secret of the GitHub OAuth app. No flag: it is a secret.",
        "google" => "The client secret of the Google OAuth client. No flag: it is a secret.",
        "microsoft" => "The client secret of the Microsoft Entra app. No flag: it is a secret.",
        _ => "The secret of the GitLab application. No flag: it is a secret.",
    }
}

fn provider_fields() -> String {
    PROVIDERS
        .iter()
        .map(|id| {
            format!(
                "    /// {}\n    pub {}: Option<String>,\n    /// {}\n    pub {}: Option<String>,\n",
                client_id_help(id),
                client_id_key(id),
                client_secret_help(id),
                client_secret_key(id),
            )
        })
        .collect()
}

fn provider_defaults() -> String {
    PROVIDERS
        .iter()
        .map(|id| {
            format!(
                "            {}: None,\n            {}: None,\n",
                client_id_key(id),
                client_secret_key(id)
            )
        })
        .collect()
}

fn provider_list() -> String {
    PROVIDERS
        .iter()
        .map(|id| {
            format!(
                "        (\n            \"{id}\",\n            &settings.{},\n            &settings.{},\n        ),\n",
                client_id_key(id),
                client_secret_key(id)
            )
        })
        .collect()
}

fn plugin_rs(project: &Project) -> String {
    let memory = project.has("oauth-client-memory");
    let csrf = project.has("middleware-csrf");
    let (derive, note) = settings_derive(project, "oauth_client");
    let (store_doc, migrate_field, migrate_default, store) = if memory {
        (
            "//! The tokens are kept in this process's memory (`oauth-client-memory`):\n\
             //! a restart drops every connection. For tests and development only.",
            "",
            "",
            STORE_MEMORY,
        )
    } else {
        (
            "//! The tokens are rows in the `db` module's database, sealed with `key`.",
            "    /// Create the token table on the first readiness check.\n    pub migrate: bool,\n",
            "            migrate: true,\n",
            STORE_POSTGRES,
        )
    };
    let store_use = if memory {
        "Key, MemoryTokenStore, OauthClient, Provider, TokenStore"
    } else {
        "Key, OauthClient, PgTokenStore, Provider, TokenStore"
    };
    PLUGIN_RS
        .replace("@STORE_USE@", store_use)
        .replace("@STORE_DOC@", store_doc)
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace("@MIGRATE_FIELD@", migrate_field)
        .replace("@MIGRATE_DEFAULT@", migrate_default)
        .replace("@PROVIDER_FIELDS@", &provider_fields())
        .replace("@PROVIDER_DEFAULTS@", &provider_defaults())
        .replace("@PROVIDER_LIST@", &provider_list())
        .replace("@STORE@", store)
        .replace("@MOUNT@", &module_ref("oauth-client", "mount_at"))
        .replace(
            "@ENV@",
            &format!("{}_OAUTH_CLIENT__KEY", project.ident().to_uppercase()),
        )
        .replace(
            "@PAGE@",
            if csrf {
                "stamped(ctx, page.html())"
            } else {
                "page.html()"
            },
        )
        .replace("@STAMPED@", if csrf { STAMPED_FN } else { "" })
}

const STAMPED_FN: &str = r#"
/// The page with the CSRF token in every form that posts.
///
/// The connect pages render their own forms and do not know the token.
/// Without a token on the request, a test that built the context by hand,
/// the page goes out unchanged.
fn stamped(ctx: &Ctx, html: String) -> String {
    match ctx.hooked::<brocanth_middleware::CsrfToken>() {
        Some(token) => token.stamp_forms(&html),
        None => html,
    }
}
"#;

const STORE_POSTGRES: &str = r#"        // The pool the `db` plugin opened. It is lazy, so nothing here
        // waits; the first readiness check creates the table.
        let Some(db) = lc.services().get::<brocanth_db::Db>().cloned() else {
            return Err(PluginError::new(
                "[oauth_client] no database: the tokens are rows in the `db` module's database\n\
                 help: register the db plugin before this one",
            ));
        };
        let store = Arc::new(PgTokenStore::new(db));
        if self.settings.migrate {
            let migrated = Arc::new(tokio::sync::OnceCell::new());
            let probe = Arc::clone(&store);
            lc.ready_when(move || {
                let probe = Arc::clone(&probe);
                let migrated = Arc::clone(&migrated);
                async move {
                    migrated
                        .get_or_try_init(|| probe.migrate())
                        .await
                        .map(drop)
                        .map_err(|e| {
                            format!(
                                "creating the token table: {e}\n\
                                 help: check `[db] url`; with `migrate = false`, run \
                                 `brocanth_oauth_client::PgTokenStore::SCHEMA` in a migration"
                            )
                        })
                }
            });
        }
"#;

const STORE_MEMORY: &str = r#"        // In memory: `oauth-client-memory`. Remove that module and the
        // tokens become rows in the `db` module's database.
        let store = Arc::new(MemoryTokenStore::new());
"#;

const PLUGIN_RS: &str = r##"//! Connected accounts: call a provider's API as a person who connected it.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! The routes, under `mount_at` in `src/modules/oauth_client.rs`:
//!
//! | Route | Does |
//! |---|---|
//! | `GET /connections` | the signed-in person's connections, a button each |
//! | `POST /connections/<provider>` | a sealed flow cookie, then a redirect to the provider |
//! | `GET /connections/<provider>/callback` | the provider's answer: the token is stored |
//! | `POST /connections/<provider>/disconnect` | revokes at the provider, deletes the row |
//!
//! A page of yours links a provider with a form that posts to
//! `/connections/<provider>`, with an optional `return_to` path.
//!
@STORE_DOC@
//!
//! # Calling the API
//!
//! ```ignore
//! let oauth = ctx.provided::<OauthClient, _>().expect("dispatched");
//! let github = oauth.http(&account, "github");
//! let response = github.send(github.get("https://api.github.com/user")).await?;
//! ```
//!
//! `http` refreshes an expired token first, once for every caller at the
//! same time, and retries once after a 401. `Error::Reconnect` means the
//! person must connect again. `oauth.app_http("<provider>")` calls as the
//! application (client credentials).
//!
//! # Another provider
//!
//! `brocanth_oauth_client::endpoints` has GitHub, Google, Microsoft and
//! GitLab. For another one, push a `Provider::new(id, name,
//! Endpoint::new(auth_url, token_url), client_id, secret)` in `providers`.

use std::sync::{Arc, OnceLock};

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ProvidesContext};
use brocanth::routing::{BuildError, RouterBuilder, Verb};
use brocanth_auth::session::Privilege;
use brocanth_auth::AuthError;
use brocanth_oauth_client::web::{ConnectAnswer, ConnectRequest, ConnectRoutes};
use brocanth_oauth_client::{@STORE_USE@};

use crate::pages::{Ctx, Error, Handler, Response};
use crate::plugins::auth::Services;

/// The scopes each provider asks for. The code that calls the API needs
/// them, so they are code here, not settings. Ask for the fewest.
fn scopes(id: &str) -> &'static [&'static str] {
    match id {
        "github" => &["read:user"],
        "google" => &["openid", "email"],
        // `offline_access` is what gives a refresh token.
        "microsoft" => &["offline_access", "User.Read"],
        "gitlab" => &["read_user"],
        _ => &[],
    }
}

/// Your registration at each provider: `[oauth_client]` in the config file,
/// or `$<APP>_OAUTH_CLIENT__<KEY>`.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// The providers to offer, by id.
    pub providers: Vec<String>,
    /// The scheme and host the providers send the browser back to.
    pub base_url: String,
    /// The key that seals the tokens: 32 random bytes, base64url.
    pub key: Option<String>,
    /// Accept `http://` provider URLs. Development only.
    pub allow_http: bool,
@MIGRATE_FIELD@@PROVIDER_FIELDS@}

impl Default for Settings {
    fn default() -> Self {
        Self {
            providers: vec!["github".to_string()],
            base_url: "http://localhost:3000".to_string(),
            key: None,
            allow_http: false,
@MIGRATE_DEFAULT@@PROVIDER_DEFAULTS@        }
    }
}

/// Each provider in `providers`, with its registration and its scopes.
fn providers(settings: &Settings) -> Result<Vec<Provider>, PluginError> {
    let known: [(&str, &Option<String>, &Option<String>); 4] = [
@PROVIDER_LIST@    ];
    let mut out = Vec::new();
    for id in &settings.providers {
        let Some((_, client_id, secret)) = known.iter().find(|(known, _, _)| known == id) else {
            return Err(PluginError::new(format!(
                "[oauth_client] providers names `{id}`, which this file does not configure\n\
                 help: use github, google, microsoft or gitlab, or add the provider to \
                 `providers` in src/plugins/oauth_client/mod.rs"
            )));
        };
        let (Some(client_id), Some(secret)) = (client_id, secret) else {
            // This project's own tests set no secret: the provider is left
            // out, and the pages still answer.
            if cfg!(test) {
                continue;
            }
            return Err(PluginError::new(format!(
                "[oauth_client] `{id}` has no client id or no client secret\n\
                 help: set {id}_client_id and {id}_client_secret under [oauth_client], or \
                 the variables $<APP>_OAUTH_CLIENT__{upper}_CLIENT_ID and \
                 $<APP>_OAUTH_CLIENT__{upper}_CLIENT_SECRET",
                upper = id.to_uppercase()
            )));
        };
        if let Some(provider) = Provider::builtin(id, client_id.as_str(), secret.as_str()) {
            out.push(provider.scopes(scopes(id).iter().copied()));
        }
    }
    Ok(out)
}

/// The key, from the settings.
fn key(settings: &Settings) -> Result<Key, PluginError> {
    match settings.key.as_deref() {
        Some(raw) => Key::parse(raw).map_err(|e| PluginError::new(format!("[oauth_client] {e}"))),
        // This project's own tests run with no key. It is fixed and public,
        // and `cfg(test)` keeps it out of every binary.
        None if cfg!(test) => Ok(Key::from_bytes([7; 32])),
        None => Err(PluginError::new(format!(
            "[oauth_client] no key: the stored tokens are sealed with it\n\
             help: set $@ENV@; {}",
            Key::HELP
        ))),
    }
}

/// The routes this run built, for [`mount`]. The route table is built
/// before the plugins start, so a request reads them when it arrives.
static ROUTES: OnceLock<ConnectRoutes> = OnceLock::new();

/// Builds the client over the token store, puts it on the context and in
/// the admin tab.
pub struct OauthClientPlugin {
    settings: Settings,
    client: Option<OauthClient>,
}

impl Plugin for OauthClientPlugin {
    type Config = Settings;
    type Provides = ProvidesContext<OauthClient>;
    const NAME: &'static str = "oauth_client";

    fn new(config: Settings) -> Self {
        Self {
            settings: config,
            client: None,
        }
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let key = key(&self.settings)?;
        let providers = providers(&self.settings)?;
@STORE@        let mount = @MOUNT@;
        let base = format!("{}{mount}", self.settings.base_url.trim_end_matches('/'));
        let store: Arc<dyn TokenStore> = store;
        let mut builder = OauthClient::builder(key)
            .store(store)
            .callback_base(&base)
            .allow_http(self.settings.allow_http);
        for provider in providers {
            builder = builder.provider(provider);
        }
        let client = builder
            .build()
            .map_err(|e| PluginError::new(format!("[oauth_client] {e}")))?;
        client.install();
        let secure = base.starts_with("https://");
        let _ = ROUTES.set(ConnectRoutes::new(client.clone(), mount, secure));
        self.client = Some(client);
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        if let Some(client) = &self.client {
            cx.insert(client.clone());
        }
    }
}

/// The connect pages, under `prefix`.
pub fn mount(routes: &mut RouterBuilder<Handler>, prefix: &str) -> Result<(), BuildError> {
    let prefix = prefix.trim_end_matches('/');
    let rest = format!("{prefix}/{{*rest}}");
    routes.route(prefix, Verb::Get, step())?;
    routes.route(&rest, Verb::Get, step())?;
    routes.route(&rest, Verb::Post, step())?;
    Ok(())
}

/// The signed-in account, or `None`. An operator who impersonates a
/// person does not connect or disconnect accounts as them.
async fn account(ctx: &Ctx) -> Result<Option<String>, Error> {
    let Some(auth) = ctx.provided::<Services, _>() else {
        return Err(Error::new(
            503,
            "oauth_client: the auth services are not on this request\n\
             help: register the auth plugin in src/plugins/mod.rs",
        ));
    };
    match auth
        .sessions()
        .from_cookie_header(ctx.header("cookie").unwrap_or(""))
        .await
    {
        Ok(session)
            if session.privilege != Privilege::Anonymous && session.impersonation.is_none() =>
        {
            Ok(Some(session.account.to_string()))
        }
        Err(AuthError::Store(e)) => Err(Error::internal(format!(
            "oauth_client: reading the session: {e}"
        ))),
        _ => Ok(None),
    }
}

/// One handler for every page. The routes read the path and decide.
fn step() -> Handler {
    Box::new(|ctx: &mut Ctx| {
        Box::pin(async move {
            let ctx = &*ctx;
            let Some(routes) = ROUTES.get() else {
                return Err(Error::new(
                    503,
                    "oauth_client: the plugin did not start\n\
                     help: register it in src/plugins/mod.rs",
                ));
            };
            let account = account(ctx).await?;
            let request = ConnectRequest {
                post: ctx.verb == Verb::Post,
                path: &ctx.path,
                query: &ctx.query,
                body: ctx.body_text().unwrap_or(""),
                cookies: ctx.header("cookie").unwrap_or(""),
                account: account.as_deref(),
            };
            let answer = routes
                .answer(request)
                .await
                .map_err(|e| Error::internal(format!("oauth_client: {e}")))?;
            let (response, cookies) = match answer {
                ConnectAnswer::Redirect { location, cookies } => (
                    Response::empty(303).with_header("location", location),
                    cookies,
                ),
                ConnectAnswer::Page {
                    status,
                    page,
                    cookies,
                } => (Response::new(status, @PAGE@), cookies),
            };
            Ok(cookies.iter().fold(response, |response, cookie| {
                response.with_header("set-cookie", cookie.as_str())
            }))
        })
    })
}
@STAMPED@
#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    #[tokio::test]
    async fn the_connections_page_needs_a_signed_in_account() {
        let app = crate::app::build().expect("the application builds");
        let path = @MOUNT@;
        let response = crate::app::handle(&app, Request::new(Verb::Get, path)).await;
        assert_eq!(response.status, 401);
    }
}
"##;
