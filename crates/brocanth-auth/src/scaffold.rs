//! `auth` -- what `brocanth create` writes when somebody asks for accounts.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: that [`AuthConfig`](crate::AuthConfig) takes
//! a store, a client, a clock and a key as *values* the application built,
//! that [`SigningKey::parse`](crate::SigningKey::parse) refuses anything that
//! is not 32 bytes, that a new project starts on
//! [`MemoryStore`](crate::MemoryStore). Those move when
//! this crate moves, and a generator living in another repository would learn
//! about it from a bug report.
//!
//! # Why the generated plugin is not `AuthPlugin`
//!
//! The same split as the cache and the queue, reached by a different road.
//! There the reason is asynchrony: a pool is opened asynchronously and
//! `Plugin::new` is not. Here nothing is opened at all -- `MemoryStore` is a
//! `HashMap` and [`Auth::build`](crate::Auth::build) is synchronous -- and
//! the reason is the shape of the configuration.
//! [`AuthPlugin`](crate::AuthPlugin) takes an [`AuthConfig`](crate::AuthConfig)
//! whose fields are an `Arc<S>`, an `Arc<H>`, a `SigningKey` and an
//! `Arc<Passwords>`. None of those comes out of a TOML file, and none of them
//! should: the key is a secret and the store is a decision. So the generated
//! plugin takes the five things a file can carry, builds the four things it
//! cannot, and publishes the same `Auth`. Swapping to `AuthPlugin` is a change
//! in one generated file.
//!
//! The generated file says the one sentence that matters for the swap: the
//! moment the store is a database, the build moves into `before_start`,
//! which builds the pool lazily and leaves the wait to `serve`'s startup
//! gate, exactly as `brocanth-cache` does.
//!
//! # What this module writes
//!
//! The local account path, and nothing else.
//!
//! * **Registration, login, password reset, email verification, sessions and
//!   identity linking.** All of [`Auth`](crate::Auth), which is one `Arc` and
//!   one context item, so a handler writes `ctx.provided()` and a handler in an
//!   application that never registered the plugin does not compile.
//! * **No provider.** The crate's default feature is `runtime`, which has no
//!   HTTP client: the common case is local accounts and no provider, and that
//!   application must not link an HTTP client, a TLS stack and a second
//!   runtime to hash a password. A provider is also not free in the other
//!   currency -- it is a client id, a secret and a redirect URI registered at
//!   somebody else's console -- so a scaffolded Google button is a button that
//!   answers an error until a developer does work the generator cannot do for
//!   them. What the generated file carries instead is the three lines that add
//!   one, written out, with the catalog entry named. Adding a provider is a
//!   struct literal in this crate, and the generated file is where that
//!   literal goes.
//!
//!   Provider sign-in is the child module `auth-oauth`. Its `Dep` turns on
//!   `reqwest-client`, and the CLI merges the features of every module that
//!   names `brocanth-auth` into one dependency line.
//! * **No page, no one-time code, no login alert.** The pages (sign-up,
//!   sign-in, reset, verify, TOTP) are the child module `auth-screens`.
//!   One-time codes and login alerts have no module yet; the stores
//!   implement their traits, so an application builds them from
//!   `auth.store()`.
//! * **No impersonation.** It is an operator tool and it needs an operator:
//!   a second account with a privilege this application has not defined yet.
//!
//! # Where the store lives
//!
//! `MemoryStore` alone, and `PgStore` on the project's one pool when it has
//! one (`uses::<PgPool>`): the plugin reads `brocanth_db::Db` from the
//! services the `db` plugin fills, and opens no pool of its own. The
//! `postgres` feature is declared on the `Dep` with `feature_with`, so the
//! CLI holds no rule for this crate.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{configurable, redact_secrets, settings_derive};
use brocanth_scaffold::{
    Category, Dep, Manifest, Module, PgPool, Project, Scaffold, Setting, Slot,
};

mod lockout;
mod oauth;
mod screens;
mod staff;

pub use lockout::AuthLockout;
pub use oauth::AuthOauth;
pub use screens::AuthScreens;
pub use staff::{Roles, Superuser};

/// Accounts, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct Auth;

impl Module for Auth {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "auth",
            "argon2id accounts and signed sessions; no provider, so no HTTP client",
        )
        .category(Category::Middleware)
        .category(Category::Enhancer)
        .uses::<PgPool>("accounts and sessions are rows on the project's pool, not in memory")
        .adapts_to(
            "auth-oauth",
            "the session cookie is SameSite=Lax, so the provider callback sees who is signed in",
        )
        .asked_in("doors", "Accounts and passwords. No OAuth provider")
        .krate(Dep::brocanth("brocanth-auth").feature_with("db", "postgres"))
        .absent(
            "brocanth-auth",
            "no accounts; nothing hashes a password and nothing signs a session cookie",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let mut file = if uses_postgres(project) {
            postgres_plugin_rs(project)
        } else {
            plugin_rs(project)
        };
        if project.has("auth-oauth") {
            assert!(
                file.contains(STRICT),
                "the auth scaffold lost its SameSite line"
            );
            file = file.replacen(STRICT, LAX, 1);
        }
        s.file(
            "src/plugins/auth/mod.rs",
            redact_secrets(&file, &settings(project)),
        );
        s.at(Slot::PluginsMod).line("pub mod auth;");
        configurable(s, project, "auth", "Auth", settings(project));
        // The services are read at startup, so a brand-new project uses the
        // value it was given rather than warning that nothing does. It also
        // prints the provider count, which is zero, because the number a
        // reader is surprised by is the one they go and look up.
        s.at(Slot::AppContext).block(
            "    let auth: &crate::plugins::auth::Services = cx.extract();\n\
            \x20   let cookie = &auth.sessions().config().cookie_name;\n\
            \x20   let providers = auth.providers().len();\n\
            \x20   crate::plugins::say!(\n\
            \x20       info,\n\
            \x20       \"auth: session cookie {cookie}, {providers} provider(s)\";\n\
            \x20       %cookie,\n\
            \x20       providers\n\
            \x20   );"
                .to_string(),
        );
    }
}

/// Whether this project's accounts live in Postgres: on the project's one
/// pool, when it has one.
pub fn uses_postgres(project: &Project) -> bool {
    project.provider::<PgPool>().is_some()
}

/// The plugin over `PgStore`: the `MemoryStore` file, with the store on
/// the pool the `db` plugin opened, migrated by the readiness check, and a
/// sweep of dead rows. No URL of its own and no second pool: one set of
/// connections that `[db] max_connections` sizes.
///
/// Written as edits of [`plugin_rs`], so the two files cannot drift on the
/// parts they share. Each edit is checked: a pattern that is gone is a
/// panic in this crate's tests, not a file that quietly kept `MemoryStore`.
fn postgres_plugin_rs(project: &Project) -> String {
    let mut text = plugin_rs(project);
    for (from, to) in POSTGRES_EDITS {
        assert!(text.contains(from), "the auth scaffold lost `{from}`");
        text = text.replacen(from, to, 1);
    }
    text
}

const POSTGRES_EDITS: &[(&str, &str)] = &[
    (
        "//! 1. **`Store = MemoryStore`.** It is a `Mutex<HashMap>`. Accounts and
//!    sessions last exactly as long as the process, so a restart is a logout
//!    for everybody and a deploy loses every account. That is the one thing
//!    in this file to replace before anybody else uses this application.
//!    Do not write a store: `brocanth-auth` has `PgStore` behind its
//!    `postgres` feature. Add the `db` module (`brocanth add db`), and
//!    this file is written over `PgStore`, on the pool `db` opens, with the
//!    tables made by a readiness check.",
        "//! 1. **`Store = PgStore`.** Accounts and sessions are rows in the\n\
//!    `brocanth_auth_*` tables of the `db` plugin's database, so they survive a restart and\n\
//!    a deploy. The pool is built in `before_start` with no connection, and\n\
//!    the readiness check that `serve` awaits before it listens runs\n\
//!    `migrate` once (it is idempotent and takes an advisory lock). To review\n\
//!    the schema in a migration file instead, copy `PgStore::schema_sql()`\n\
//!    into one and delete the `migrate` call.",
    ),
    (
        "use brocanth::plugin_context::{ContextBuilder, ProvidesContext};",
        "use brocanth::plugin_context::{ContextBuilder, ProvidesContext};\n\
         use brocanth::plugin_shutdown::ShutdownCx;",
    ),
    (
        "use brocanth_auth::{MemoryStore, Passwords, SystemClock};",
        "use brocanth_auth::{Clock as _, Passwords, PgStore, SystemClock};",
    ),
    (
        "/// In memory, for now. See the file header: this is the line to change first.
pub type Store = MemoryStore;",
        "/// Postgres, on the pool the `db` plugin opened. See the file header.
pub type Store = PgStore;",
    ),
    (
        "    /// Built at `configure`, so this is an `Option` for exactly the length of
    /// the build.
    services: Option<Services>,
}",
        "    /// Built at `before_start`, so this is an `Option` for exactly the length
    /// of the build.
    services: Option<Services>,
    /// The store, kept for the sweep.
    store: Option<Arc<Store>>,
    /// The sweep, stopped at shutdown.
    sweeper: Option<tokio::task::JoinHandle<()>>,
}",
    ),
    (
        "            settings: config,
            services: None,
        }",
        "            settings: config,
            services: None,
            store: None,
            sweeper: None,
        }",
    ),
    (
        "    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if self.settings.cookie_name.is_empty() {",
        "    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        if self.settings.cookie_name.is_empty() {",
    ),
    (
        "        // Nothing here opens anything, so there is no `before_start`:
        // `MemoryStore` is a `HashMap` and `Auth::build` is synchronous. A
        // store over a database is a pool, a pool is opened asynchronously,
        // and no stage may wait for it -- so that swap moves this block into
        // `before_start`, where it builds the pool lazily and registers a
        // readiness check that `serve` awaits. `brocanth-cache` does that.
        self.services = Some(",
        "        // The pool the `db` plugin opened, which registers before this one:
        // one set of connections for the pages and the accounts.
        let db = lc.require_service::<brocanth_db::Db>()?;
        let store = PgStore::from_pool(db.pool().clone());
        let store = Arc::new(store);
        self.store = Some(Arc::clone(&store));
        // Ready when the database answers. The first check also creates the
        // tables, and the first check is the one `serve` awaits before it
        // listens: an unreachable database stops the start there.
        let probe = Arc::clone(&store);
        let migrated = Arc::new(tokio::sync::OnceCell::new());
        lc.ready_when(move || {
            let probe = Arc::clone(&probe);
            let migrated = Arc::clone(&migrated);
            async move {
                migrated
                    .get_or_try_init(|| probe.migrate())
                    .await
                    .map_err(|e| format!(\"creating the auth tables: {e}\"))?;
                probe.ping().await.map_err(|e| e.to_string())
            }
        });

        self.services = Some(",
    ),
    (
        "                store: Arc::new(Store::new()),",
        "                store,",
    ),
    (
        "    fn provide(&self, cx: &mut ContextBuilder) {
        // `configure` runs before",
        "    fn start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // Dead sessions, spent tokens and expired one-time codes, every
        // fifteen minutes. Every read checks expiry already, so this only
        // keeps the tables small. Each instance runs it, and the deletes do
        // not mind.
        let Some(store) = self.store.clone() else {
            return Ok(());
        };
        let idle_timeout = self.settings.idle_timeout;
        self.sweeper = Some(tokio::spawn(async move {
            let mut every = tokio::time::interval(std::time::Duration::from_secs(15 * 60));
            loop {
                every.tick().await;
                if let Err(e) = store.sweep(SystemClock.now(), idle_timeout).await {
                    crate::plugins::say!(warn, \"[auth] sweep: {e}\"; error = %e);
                }
            }
        }));
        Ok(())
    }

    fn shutdown(&mut self, _sd: &mut ShutdownCx<'_>) -> Result<(), PluginError> {
        if let Some(sweeper) = self.sweeper.take() {
            sweeper.abort();
        }
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        // `before_start` runs before",
    ),
];

/// The `same_site` line of the generated plugin with no provider.
const STRICT: &str = "            // Strict, and not the crate's own `Lax` default. `Lax` is there
            // for the cross-site top-level navigation an OAuth callback is,
            // and this application has no provider. Add one and this line
            // has to become `Lax` or the cookie is dropped on the way back.
            same_site: SameSite::Strict,";

/// The same line with `auth-oauth`: the callback is a cross-site top-level
/// navigation, and it reads the session to tell a link from a sign-in and
/// to replace the session it arrived with.
const LAX: &str = "            // Lax, because `auth-oauth` is on. The provider callback is a
            // cross-site top-level navigation, and it reads this cookie to
            // tell a link from a sign-in. `Strict` would drop it there: a
            // signed-in person who connects a provider would be signed in as
            // the provider's account instead. Every form still carries the
            // CSRF token, so `Lax` opens no cross-site post.
            same_site: SameSite::Lax,";

/// The `[auth]` section of `config/development.toml`.
///
/// The key is not in it, and that is the point of the section: what is
/// written down is the *name* of the environment variable, so a file that is
/// in git says where the secret comes from without being it.
///
/// `secure` is false here and would not be in production, and the cookie name
/// moves with it: `__Host-` is the only prefix a browser enforces, and what it
/// enforces is `Secure`, a path of `/` and no `Domain`. The two cannot
/// disagree, so the generated plugin stops the application when they do.
fn settings(project: &Project) -> Vec<Setting> {
    let var = env_var(project);
    let mut out = Vec::new();
    out.push(
        Setting::text(
            "signing_key_env",
            &var,
            "The name of the variable that holds the session signing key.",
        )
        .note(format!(
            "Not the key. 32 bytes, base64url. `brocanth` will not invent one: a key\n\
             minted at startup logs every session out on each restart and differs\n\
             between the instances of a fleet, so a missing key stops the application.\n\
             \n\
             \x20 export {var}=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')"
        )),
    );
    let totp = totp_env_var(project);
    out.push(
        Setting::text(
            "totp_key_env",
            &totp,
            "The name of the variable that holds the key sealing TOTP secrets.",
        )
        .note(format!(
            "Not the key. 32 bytes, base64url, and not the signing key: that one is\n\
             rotated to sign everybody out. Unset, TOTP secrets are stored plain;\n\
             set it before anybody turns on two-step sign-in.\n\
             \n\
             \x20 export {totp}=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')"
        )),
    );
    out.push(
        Setting::text("cookie_name", "session", "The session cookie's name.").note(
            "Development, over plain HTTP on localhost. In production this is\n\
             \"__Host-session\" with secure = true, and the two move together: a\n\
             __Host- cookie that is not Secure is a cookie no browser stores.",
        ),
    );
    out.push(Setting::bool(
        "secure",
        false,
        "Send the cookie over HTTPS only.",
    ));
    out.push(
        Setting::integer(
            "idle_timeout",
            28800,
            "Seconds of inactivity before a session is dead.",
        )
        .note(
            "Eight hours unattended, thirty days total. Idle bounds a browser nobody is\n\
                 sitting at; absolute bounds a stolen cookie that is being kept warm.",
        ),
    );
    out.push(Setting::integer(
        "absolute_timeout",
        2592000,
        "Seconds from creation before a session is dead, however much it was used.",
    ));
    out
}

/// The environment variable the generated project reads its key from.
///
/// `MY_BLOG_SESSION_KEY`, from the same convention `src/app.rs` states for
/// every other variable this binary reads: one prefix, derived once, whatever
/// spelling the project name arrived in.
/// The environment variable the generated project reads its TOTP key from.
fn totp_env_var(project: &Project) -> String {
    format!("{}_TOTP_KEY", project.ident().to_uppercase())
}

fn env_var(project: &Project) -> String {
    format!("{}_SESSION_KEY", project.ident().to_uppercase())
}

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "auth");
    let var = env_var(project);
    let totp = totp_env_var(project);
    format!(
        r##"//! Accounts and sessions, as this application registers them.
//!
//! One interface, no providers yet, and a local account path that does not
//! leak who has one: `register`, `login`, `request_password_reset` and
//! `verify_email` answer the same shape and take the same time whether or not
//! the address exists, because the expensive hash runs on both paths.
//!
//! `brocanth-auth` is in `Cargo.toml` because this file is here, and through
//! no other route. Delete the registration in `src/plugins/mod.rs` and
//! delete this file, and the application has no accounts and links nothing
//! to hash a password with.
//!
//! # Signing somebody in
//!
//! Everything is behind one context item, and a handler in an application
//! that never registered this plugin does not compile:
//!
//! ```ignore
//! use brocanth_auth::mfa::{{Admission, FactorPolicy}};
//!
//! let auth = ctx.provided::<Services, _>().expect("a dispatched request");
//!
//! // 1. The credential. `login` answers the same shape and takes the same
//! //    time whether or not the address has an account.
//! let who = auth.accounts().login(email, password).await?;
//!
//! // 2. What this account could prove: its confirmed TOTP and its
//! //    recovery codes, read from the store.
//! let proof = who.into_proof();
//! let enrolled = auth.mfa()?.second_factors(&proof).await?;
//!
//! // 3. The policy, which consumes the proof. `Admitted` has no public
//! //    constructor, so this step cannot be skipped -- a sign-in that
//! //    forgot the second factor is a program that does not compile rather
//! //    than one that ships.
//! let Admission::Admit(admitted) = FactorPolicy::default().admit(&enrolled, proof) else {{
//!     return Err(Error::new(401, "sign in"));
//! }};
//!
//! // 4. The session, which takes the admission by value.
//! let started = auth.sessions().begin_with_factors(admitted, None).await?;
//! // `started.cookie` is the `Set-Cookie` header to send back.
//! ```
//!
//! A page is an `async fn`, so it makes these calls itself. The
//! `auth-screens` module writes the sign-up, sign-in, reset, verify and
//! TOTP pages under `/account` that do exactly this; read
//! `src/pages/account/signin.rs` and `src/plugins/auth_screens/mod.rs`
//! (`admit`) before you write your own.
//!
//! # Three decisions this file makes, each in one line
//!
//! They are type aliases so that changing one changes one line and nothing
//! else in the project changes type.
//!
//! 1. **`Store = MemoryStore`.** It is a `Mutex<HashMap>`. Accounts and
//!    sessions last exactly as long as the process, so a restart is a logout
//!    for everybody and a deploy loses every account. That is the one thing
//!    in this file to replace before anybody else uses this application.
//!    Do not write a store: `brocanth-auth` has `PgStore` behind its
//!    `postgres` feature. Add the `db` module (`brocanth add db`), and
//!    this file is written over `PgStore`, on the pool `db` opens, with the
//!    tables made by a readiness check.
//! 2. **`Http = StubHttp`.** There are no providers, so nothing in this
//!    binary ever performs a provider request, and a client that answers "no
//!    stubbed route" is a louder failure than one that opens a socket. It is
//!    also the line that keeps reqwest, rustls and their trees out of the lock
//!    file. Adding a provider changes it; see below.
//! 3. **`Passwords::recommended()`, not a setting.** The hash is argon2id and
//!    there is no choice to make. The one question a deployment gets to answer
//!    is cost, and it is a named constructor rather than three numbers in a
//!    file, because three numbers in a file is how an application ends up
//!    hashing at a cost nobody chose.
//!
//! # Adding a provider
//!
//! The `auth-oauth` module does this for GitHub, Google, Microsoft and
//! GitLab: the routes, their settings and the `Lax` cookie. By hand, it is
//! three steps, and the flow code never learns the provider's name.
//!
//! 1. In `Cargo.toml`, `brocanth-auth = {{ ..., features = ["reqwest-client"] }}`.
//! 2. Here, `pub type Http = brocanth_auth::http_client::ReqwestClient;`.
//! 3. Here, in `configure`, push one `ConfiguredProvider`:
//!
//! ```ignore
//! use brocanth_auth::oauth::ClientConfig;
//! use brocanth_auth::plugin::ConfiguredProvider;
//! use brocanth_auth::providers::catalog::GOOGLE;
//!
//! providers: vec![ConfiguredProvider {{
//!     provider: GOOGLE,
//!     client: ClientConfig::new(
//!         std::env::var("GOOGLE_CLIENT_ID")?,
//!         std::env::var("GOOGLE_CLIENT_SECRET")?,
//!         "https://example.test/auth/google/callback",
//!     ),
//! }}]
//! ```
//!
//! A provider is data -- endpoints, scopes, and a mapping from its profile
//! JSON to a common identity -- so the second one is another entry in that
//! `vec!` and no new code anywhere. `brocanth_auth::providers::catalog` has
//! the ones that are already written down.
//!
//! Two things change with the first provider. `same_site` below must become
//! `Lax`, because the callback is a cross-site top-level navigation and
//! `Strict` drops the cookie on the way back. And a matching email still does
//! not link an account to a provider: `Linking` returns a refusal the
//! application answers with a page that asks, because "this provider says
//! alice@example.com" is not Alice consenting, and several real products have
//! shipped that account takeover.
//!
//! # What is not here
//!
//! One-time codes and login alerts. Both stores implement their traits
//! (`OtpStore`, `SignInLocationStore`); an application that wants them
//! builds them from `auth.store()` and puts them on the context itself.
//! Second factors are `auth.mfa()`, and `auth-screens` has the TOTP page.

use std::sync::Arc;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ProvidesContext}};
use brocanth_auth::http_client::StubHttp;
use brocanth_auth::plugin::AuthConfig;
use brocanth_auth::session::{{SameSite, SessionConfig, SigningKey}};
use brocanth_auth::{{MemoryStore, Passwords, SystemClock}};

/// Where identities, sessions and single-use tokens live.
///
/// In memory, for now. See the file header: this is the line to change first.
pub type Store = MemoryStore;

/// How this application reaches a provider.
///
/// It does not. There are no providers, so nothing calls this.
pub type Http = StubHttp;

/// Where the time comes from.
pub type Clock = SystemClock;

/// Everything a handler needs, behind one `Arc`: registration, login, the
/// token service, sessions and identity linking.
///
/// An alias rather than a wrapper, because `brocanth_auth::Auth` is already a
/// `ContextItem`. Cloning is cheap and the clone shares the provider key
/// cache, which is why there is one of these and not one per request.
pub type Services = brocanth_auth::Auth<Store, Http, Clock>;

/// What this plugin is built from.
///
/// Five values a file can carry. The store, the client, the clock and the
/// hasher are built below, because none of them is a thing a TOML file can
/// say.
///
/// {note}
{derive}
pub struct Settings {{
    /// The *name* of the environment variable holding the session signing
    /// key, not the key. 32 bytes, base64url.
    pub signing_key_env: String,
    /// The *name* of the environment variable holding the key that seals
    /// TOTP secrets at rest. Unset, they are stored plain.
    pub totp_key_env: String,
    /// The session cookie's name. `__Host-` in production: it is the only
    /// prefix a browser enforces, and what it enforces is `Secure`, a path of
    /// `/` and no `Domain` -- so a subdomain takeover cannot set a session
    /// cookie for the parent.
    pub cookie_name: String,
    /// Whether the cookie is `Secure`. False only on plain-HTTP localhost,
    /// and then the name above must lose its `__Host-` prefix as well.
    pub secure: bool,
    /// Seconds of inactivity before the session is dead.
    pub idle_timeout: i64,
    /// Seconds from creation before it is dead however much it was used.
    pub absolute_timeout: i64,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            signing_key_env: "{var}".to_string(),
            totp_key_env: "{totp}".to_string(),
            cookie_name: "__Host-session".to_string(),
            secure: true,
            idle_timeout: 8 * 60 * 60,
            absolute_timeout: 30 * 24 * 60 * 60,
        }}
    }}
}}

pub struct Auth {{
    settings: Settings,
    /// Built at `configure`, so this is an `Option` for exactly the length of
    /// the build.
    services: Option<Services>,
}}

impl Plugin for Auth {{
    type Config = Settings;
    type Provides = ProvidesContext<Services>;
    const NAME: &'static str = "auth";

    fn new(config: Settings) -> Self {{
        Self {{
            settings: config,
            services: None,
        }}
    }}

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.settings.cookie_name.is_empty() {{
            return Err(PluginError::new(
                "[auth] cookie_name must not be empty\n\
                 help: set `cookie_name` under [auth] in the config file, such as \"__Host-session\"",
            ));
        }}
        // A `__Host-` cookie is `Secure` by definition, so a configuration
        // where the two disagree produces a cookie no browser stores and a
        // login that silently never takes. Caught here, at startup, where it
        // can still be answered.
        if self.settings.cookie_name.starts_with("__Host-") && !self.settings.secure {{
            return Err(PluginError::new(
                "[auth] a __Host- cookie is Secure by definition, and secure is false\n\
                 help: set secure = true under [auth], or drop the prefix for plain-HTTP development",
            ));
        }}

        let name = self.settings.signing_key_env.clone();
        let signing_key = match std::env::var(&name) {{
            // Rejected rather than stretched. Padding a four-character secret
            // to 32 bytes is how a deployment ends up with a forgeable cookie
            // and no warning anywhere.
            Ok(raw) => SigningKey::parse(&raw).map_err(|e| {{
                PluginError::new(format!(
                    "[auth] ${{name}}: {{e}}\n\
                     help: set it to 32 random bytes, base64url:\n  \
                     export {{name}}=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')"
                ))
            }})?,
            // This project's own tests build the application with no secret
            // in the environment. The key is fixed and public, and `cfg(test)`
            // keeps it out of every binary.
            Err(_) if cfg!(test) => SigningKey::from_bytes([7; 32]),
            Err(_) => {{
                return Err(PluginError::new(format!(
                    "[auth] ${{name}} is not set. There is no default: a key invented at startup \
                     would log every session out on each restart and differ between the \
                     instances of a fleet.\n\
                     help: set it to 32 random bytes, base64url:\n  \
                     export {{name}}=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')"
                )));
            }}
        }};

        let passwords = Passwords::recommended().map_err(|e| {{
            PluginError::new(format!(
                "[auth] argon2id: {{e}}\n\
                 help: the parameters are brocanth-auth's own, so this is a bug; report it with this message"
            ))
        }})?;

        let session = SessionConfig {{
            cookie_name: self.settings.cookie_name.clone(),
            secure: self.settings.secure,
            idle_timeout: self.settings.idle_timeout,
            absolute_timeout: self.settings.absolute_timeout,
            // Strict, and not the crate's own `Lax` default. `Lax` is there
            // for the cross-site top-level navigation an OAuth callback is,
            // and this application has no provider. Add one and this line
            // has to become `Lax` or the cookie is dropped on the way back.
            same_site: SameSite::Strict,
            // Path `/`, and thirty minutes for an impersonated session. Both
            // are the crate's defaults and both are arguments rather than
            // numbers; `SessionConfig` carries them.
            ..SessionConfig::default()
        }};

        // Nothing here opens anything, so there is no `before_start`:
        // `MemoryStore` is a `HashMap` and `Auth::build` is synchronous. A
        // store over a database is a pool, a pool is opened asynchronously,
        // and no stage may wait for it -- so that swap moves this block into
        // `before_start`, where it builds the pool lazily and registers a
        // readiness check that `serve` awaits. `brocanth-cache` does that.
        self.services = Some(
            brocanth_auth::Auth::build(AuthConfig {{
                store: Arc::new(Store::new()),
                http: Arc::new(Http::new()),
                clock: Arc::new(SystemClock),
                signing_key,
                session,
                passwords: Arc::new(passwords),
                // Add one here. The file header has the three lines.
                providers: Vec::new(),
            }})
            .map_err(|e| PluginError::new(format!("[auth] {{e}}")))?,
        );
        // TOTP secrets sealed at rest, when the variable holds a key.
        if let Some(services) = self.services.take() {{
            self.services = Some(sealing_totp(services, &self.settings.totp_key_env)?);
        }}
        // In the services too, for a plugin that reads the accounts at
        // startup, such as `superuser`.
        if let Some(services) = &self.services {{
            lc.services_mut().insert(services.clone());
        }}
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `configure` runs before the server is handed the application, so
        // the services are here by the time any request builds a context.
        if let Some(services) = &self.services {{
            cx.insert(services.clone());
        }}
    }}
}}

/// The services with TOTP secrets sealed at rest, when `var` holds a key.
/// Unset, the secrets are stored plain: set it before anybody enrols.
fn sealing_totp(services: Services, var: &str) -> Result<Services, PluginError> {{
    let Ok(raw) = std::env::var(var) else {{
        return Ok(services);
    }};
    let key = brocanth_auth::SecretKey::parse(&raw)
        .map_err(|e| PluginError::new(format!("[auth] ${{var}}: {{e}}")))?;
    services
        .with_totp_key(key)
        .map_err(|e| PluginError::new(format!("[auth] {{e}}")))
}}
"##
    )
}
