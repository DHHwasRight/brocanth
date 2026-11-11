//! `auth-oauth` -- sign-in with GitHub, Google, Microsoft or GitLab.
//!
//! The flow is [`crate::oauth::web::OauthRoutes`]: the button page, the
//! redirect with `state`, `nonce` and PKCE in a signed cookie, the callback,
//! and the link that needs a signed-in session. This module writes the
//! settings that name the application's registration at each provider, and
//! a `mount` that puts the routes into the project's one table.
//!
//! # Four providers, not eight
//!
//! The catalog has eight. A declared setting is a field, a variable and a
//! flag, so each provider costs two of each. The four here are the ones a
//! new application asks for first. The others are one line in the
//! generated file, which says so. Apple is not one of them: its client
//! secret is a JWT that the application signs with a private key, and its
//! callback is a cross-site POST that the CSRF hook refuses.
//!
//! # The auth plugin does not change
//!
//! The routes hold their own HTTP client. `Auth` keeps `StubHttp` and no
//! provider, because linking and sessions do not call a provider. So
//! choosing this module adds a file and a mount, and rewrites nothing that
//! `auth` wrote.

use brocanth_scaffold::snippet::{
    configurable, module_ref, redact_secrets, settings_derive, PLUGIN_TRAIT_USE,
};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Opt, Project, Scaffold, Setting, Slot};

/// The providers this module declares settings for, by catalog id.
pub const PROVIDERS: &[(&str, &str)] = &[
    ("github", "GITHUB"),
    ("google", "GOOGLE"),
    ("microsoft", "MICROSOFT"),
    ("gitlab", "GITLAB"),
];

/// Provider sign-in, as a choice inside `auth`.
pub struct AuthOauth;

impl Module for AuthOauth {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "auth-oauth",
            "sign-in with GitHub, Google, Microsoft or GitLab at /oauth; links an HTTP client",
        )
        .category(Category::Routes)
        .category(Category::Enhancer)
        .part_of_but_off("auth")
        .because("a provider identity becomes an account and a session, and `auth` holds both")
        .asked_in("doors", "Sign-in with GitHub, Google, Microsoft or GitLab")
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "middleware-csrf",
            "the button and the link forms carry the CSRF token",
        )
        .option(Opt::text(
            "mount_at",
            "/oauth",
            "Where the sign-in routes answer. The redirect URI at each provider is <base_url><mount_at>/<provider>/callback.",
        ))
        .krate(
            Dep::brocanth("brocanth-auth")
                .feature("reqwest-client")
                .note("the provider requests: a token exchange and a profile read"),
        )
        .after("auth")
        .absent(
            "auth-oauth",
            "no provider sign-in; an account has an address and a password",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let csrf = project.has("middleware-csrf");
        let (derive, note) = settings_derive(project, "auth_oauth");
        let file = PLUGIN_RS
            .replace("@DERIVE@", &derive)
            .replace("@NOTE@", &note)
            .replace("@PROVIDER_FIELDS@", &provider_fields())
            .replace("@PROVIDER_DEFAULTS@", &provider_defaults())
            .replace("@PROVIDER_LIST@", &provider_list())
            .replace(
                "@PAGE@",
                if csrf {
                    "stamped(ctx, page.html())"
                } else {
                    "page.html()"
                },
            )
            .replace("@STAMPED@", if csrf { STAMPED_FN } else { "" });
        s.file(
            "src/plugins/auth_oauth/mod.rs",
            redact_secrets(&file, &settings()),
        );
        s.at(Slot::PluginsMod).line("pub mod auth_oauth;");
        configurable(s, project, "auth_oauth", "AuthOauth", settings());
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        s.at(Slot::RouterMounts).line(format!(
            "    crate::plugins::auth_oauth::mount(routes, {})?;",
            module_ref("auth-oauth", "mount_at")
        ));
        // An expression that reads `mount_at`, so a moved mount needs no
        // edit in the route table test (docs/module-config.md §9).
        s.at(Slot::PagesRoutes).line(format!(
            "format!(\"{{}}/{{{{*rest}}}}\", {})",
            module_ref("auth-oauth", "mount_at")
        ));
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

fn settings() -> Vec<Setting> {
    let mut out = vec![
        Setting::list(
            "providers",
            &["github"],
            "The providers to offer, by id: github, google, microsoft, gitlab.",
        )
        .note(
            "Each one needs its client id and secret below. The server refuses to\n\
             start when one is missing, so a button never answers an error.",
        ),
        Setting::text(
            "base_url",
            "http://localhost:3000",
            "The scheme and host the providers send the browser back to.",
        )
        .note("Register <base_url>/oauth/<provider>/callback at each provider's console."),
        Setting::text(
            "after_sign_in",
            "/",
            "Where a person goes after a sign-in or a link.",
        ),
        Setting::text(
            "sign_in_page",
            "",
            "Your password sign-in page, for a person who must sign in before a link.",
        )
        .optional(),
    ];
    for (id, _) in PROVIDERS {
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
        .map(|(id, _)| {
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
        .map(|(id, _)| {
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
        .map(|(id, constant)| {
            format!(
                "            (\n                catalog::{constant},\n                &self.{},\n                &self.{},\n            ),\n",
                client_id_key(id),
                client_secret_key(id)
            )
        })
        .collect()
}

const STAMPED_FN: &str = r#"
/// The page with the CSRF token in every form that posts.
///
/// The flow renders its own forms and does not know the token. Without a
/// token on the request, a test that built the context by hand, the page
/// goes out unchanged.
fn stamped(ctx: &Ctx, html: String) -> String {
    match ctx.hooked::<brocanth_middleware::CsrfToken>() {
        Some(token) => token.stamp_forms(&html),
        None => html,
    }
}
"#;

const PLUGIN_RS: &str = r##"//! Sign-in with a provider: GitHub, Google, Microsoft or GitLab.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! The routes, under `mount_at` in `src/modules/auth_oauth.rs`:
//!
//! | Route | Does |
//! |---|---|
//! | `GET /oauth/<provider>` | a page with one button, "Continue with GitHub" |
//! | `POST /oauth/<provider>` | a signed flow cookie, then a redirect to the provider |
//! | `GET /oauth/<provider>/callback` | the provider's answer: a session, or a page that asks |
//! | `GET`, `POST /oauth/<provider>/link` | connects the provider to the signed-in account |
//!
//! A sign-in page of yours links each provider with a form that posts to
//! `/oauth/<provider>`. A plain link to the same path shows the button page.
//!
//! # Linking
//!
//! A provider joins an existing account only by a POST from a session that
//! is signed in as that account. The callback never links by itself: "the
//! provider says alice@example.com" is not Alice's consent. A person whose
//! address already has an account signs in with the password first, then
//! confirms at `/oauth/<provider>/link`.
//!
//! # Another provider
//!
//! `brocanth_auth::providers::catalog` has Discord, Slack and Facebook too.
//! Add a field pair to `Settings` and a line to `Settings::configured`.
//! Apple needs a client secret that you sign with a private key, and its
//! callback is a cross-site POST: add its callback path to the CSRF
//! exemptions first.

use std::sync::{Arc, OnceLock};

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::routing::{BuildError, RouterBuilder, Verb};
use brocanth_auth::http_client::ReqwestClient;
use brocanth_auth::oauth::web::{OauthAnswer, OauthRequest, OauthRoutes};
use brocanth_auth::oauth::ClientConfig;
use brocanth_auth::plugin::ConfiguredProvider;
use brocanth_auth::providers::{catalog, Provider};
use brocanth_auth::SystemClock;

use crate::pages::{Ctx, Error, Handler, Response};
use crate::plugins::auth::Services;

/// Your registration at each provider: `[auth_oauth]` in the config file,
/// or `$<APP>_AUTH_OAUTH__<KEY>`.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// The providers to offer, by id.
    pub providers: Vec<String>,
    /// The scheme and host the providers send the browser back to.
    pub base_url: String,
    /// Where a person goes after a sign-in or a link.
    pub after_sign_in: String,
    /// Your password sign-in page, for a person who must sign in before a
    /// link. The flow adds `?next=<the link page>`.
    pub sign_in_page: Option<String>,
@PROVIDER_FIELDS@}

impl Default for Settings {
    fn default() -> Self {
        Self {
            providers: vec!["github".to_string()],
            base_url: "http://localhost:3000".to_string(),
            after_sign_in: "/".to_string(),
            sign_in_page: None,
@PROVIDER_DEFAULTS@        }
    }
}

impl Settings {
    /// Each provider in `providers`, with its registration. An error names
    /// the first one that has no id or no secret.
    fn configured(&self, prefix: &str) -> Result<Vec<ConfiguredProvider>, String> {
        let known: [(Provider, &Option<String>, &Option<String>); 4] = [
@PROVIDER_LIST@        ];
        let mut out = Vec::new();
        for id in &self.providers {
            let Some((provider, client_id, secret)) = known
                .iter()
                .find(|(provider, _, _)| provider.id == id.as_str())
            else {
                return Err(format!(
                    "[auth_oauth] providers names `{id}`, which this file does not configure\n\
                     help: use github, google, microsoft or gitlab, or add the provider to \
                     `Settings` in src/plugins/auth_oauth/mod.rs"
                ));
            };
            let (Some(client_id), Some(secret)) = (client_id, secret) else {
                return Err(format!(
                    "[auth_oauth] `{id}` has no client id or no client secret\n\
                     help: set {id}_client_id and {id}_client_secret under [auth_oauth], or \
                     the variables $<APP>_AUTH_OAUTH__{upper}_CLIENT_ID and \
                     $<APP>_AUTH_OAUTH__{upper}_CLIENT_SECRET",
                    upper = id.to_uppercase()
                ));
            };
            out.push(ConfiguredProvider {
                provider: provider.clone(),
                client: ClientConfig::new(
                    client_id.clone(),
                    secret.clone(),
                    OauthRoutes::<ReqwestClient, SystemClock>::callback_url(
                        &self.base_url,
                        prefix,
                        id,
                    ),
                ),
            });
        }
        Ok(out)
    }
}

/// The settings this run chose, kept for [`mount`].
///
/// `mount` runs when the route table is built, after the plugins are, and
/// takes no argument it could carry them in. A test that builds the table
/// without the application gets the defaults, and so no provider.
static CHOSEN: OnceLock<Settings> = OnceLock::new();

/// Holds the settings and checks them at startup. It puts nothing on the
/// context: the routes read the auth plugin's services.
pub struct AuthOauth;

impl Plugin for AuthOauth {
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "auth_oauth";

    fn new(config: Settings) -> Self {
        // The first application built in this process decides.
        let _ = CHOSEN.set(config);
        Self
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let Some(settings) = CHOSEN.get() else {
            return Ok(());
        };
        // A provider without a secret is a button that answers an error. In
        // this project's own tests no secret is set, so nothing is checked.
        if cfg!(test) {
            return Ok(());
        }
        settings
            .configured(crate::modules::auth_oauth::MODULE.mount_at)
            .map(drop)
            .map_err(PluginError::new)
    }
}

type Routes = OauthRoutes<ReqwestClient, SystemClock>;

/// Every route of the flow, under `prefix`.
///
/// Two routes: everything under the prefix, for GET and for POST. The flow
/// reads the provider and the step from the path.
pub fn mount(routes: &mut RouterBuilder<Handler>, prefix: &str) -> Result<(), BuildError> {
    let defaults = Settings::default();
    let settings = CHOSEN.get().unwrap_or(&defaults);
    // `configure` refuses a provider without its registration, so this
    // drops one only when the application is not the one being started.
    let providers = settings.configured(prefix).unwrap_or_default();
    // Fails only when the TLS backend cannot start, which no request could
    // recover from.
    let http = ReqwestClient::with_default_timeout().expect("auth_oauth: an HTTP client");
    let mut flow = Routes::new(Arc::new(http), Arc::new(SystemClock), providers)
        .at(prefix)
        .after_sign_in(&settings.after_sign_in);
    if let Some(page) = &settings.sign_in_page {
        flow = flow.sign_in_page(page);
    }
    let flow = Arc::new(flow);
    let pattern = format!("{}/{{*rest}}", prefix.trim_end_matches('/'));
    routes.route(&pattern, Verb::Get, step(Arc::clone(&flow)))?;
    routes.route(&pattern, Verb::Post, step(flow))?;
    Ok(())
}

/// One handler for every step. The flow reads the path and decides.
fn step(flow: Arc<Routes>) -> Handler {
    Box::new(move |ctx: &mut Ctx| {
        let flow = Arc::clone(&flow);
        Box::pin(async move {
            let ctx = &*ctx;
            let Some(auth) = ctx.provided::<Services, _>() else {
                return Err(Error::new(
                    503,
                    "auth_oauth: the auth services are not on this request\n\
                     help: register the auth plugin in src/plugins/mod.rs",
                ));
            };
            let request = OauthRequest {
                post: ctx.verb == Verb::Post,
                path: &ctx.path,
                query: &ctx.query,
                body: ctx.body_text().unwrap_or(""),
                cookies: ctx.header("cookie").unwrap_or(""),
            };
            let answer = flow
                .answer(auth, request)
                .await
                .map_err(|e| Error::internal(format!("auth_oauth: {e}")))?;
            let (response, cookies) = match answer {
                OauthAnswer::Redirect { location, cookies } => (
                    Response::empty(303).with_header("location", location),
                    cookies,
                ),
                OauthAnswer::Page {
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
@STAMPED@"##;

const AGENTS: &str = "\n\
    ## Provider sign-in\n\
    \n\
    `src/plugins/auth_oauth/mod.rs` mounts the OAuth flow at `/oauth`. A\n\
    sign-in button is a form that posts to `/oauth/<provider>`, never a\n\
    link that starts the flow: a GET that starts a sign-in can be fired by\n\
    any page on the web.\n\
    \n\
    Never link a provider to an account because the addresses match. The\n\
    flow links only on a POST from a session that is signed in as that\n\
    account, so a provider that reports somebody else's address cannot\n\
    take the account over.\n\
    \n\
    A client secret goes in `$<APP>_AUTH_OAUTH__<PROVIDER>_CLIENT_SECRET`,\n\
    never in a committed config file.\n";
