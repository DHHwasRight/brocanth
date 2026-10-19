//! `auth` -- what `brocanth create` writes when somebody asks for accounts.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: that [`AuthConfig`](crate::AuthConfig) takes
//! a store, a client, a clock and a key as *values* the application built,
//! that [`SigningKey::parse`](crate::SigningKey::parse) refuses anything that
//! is not 32 bytes, that [`MemoryStore`](crate::MemoryStore) is the only
//! [`AuthStore`](crate::AuthStore) in the workspace today. Those move when
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
//! moment the store is a database, the build moves into `before_start` and
//! reaches for a runtime handle, exactly as `brocanth-cache` does.
//!
//! # What this module writes, out of a crate with 8,875 lines in it
//!
//! The local account path, and nothing else.
//!
//! * **Registration, login, password reset, email verification, sessions and
//!   identity linking.** All of [`Auth`](crate::Auth), which is one `Arc` and
//!   one context item, so a handler writes `cx.extract()` and a handler in an
//!   application that never registered the plugin does not compile.
//! * **No provider.** The crate's own `default = []` is the argument, in its
//!   own words: the common case is local accounts and no provider, and that
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
//!   The cost cannot be put behind a second tick on the menu either, which is
//!   the shape [`crate::providers`] would otherwise suggest: `Manifest` is
//!   static, a `Dep` carries its features with it, and the generator writes
//!   one dependency line per chosen module -- so a child module that only
//!   turned `reqwest-client` on would write `brocanth-auth` into the manifest
//!   twice.
//! * **No second factor, no one-time code, no login alert.** Each needs a
//!   store trait of its own -- `MfaStore`, `OtpStore`, `SignInLocationStore` --
//!   and [`crate::plugin`] keeps all three off `Auth` for exactly that reason:
//!   putting them there would make every application implement a TOTP table to
//!   get a password form. A scaffold that contradicted the plugin it is
//!   scaffolding would be teaching the wrong thing on the first day.
//! * **No impersonation.** It is an operator tool and it needs an operator:
//!   a second account with a privilege this application has not defined yet.
//!
//! # What it cannot write, and why that is not this module's fault
//!
//! A sign-in page. A generated handler is
//! `Box<dyn Fn(&mut Ctx) -> Result<Response, Error>>` -- synchronous -- and
//! every method here is `async fn`. The queue has the same gap and answers it
//! the same way: the service goes on the context, the call is written out in
//! the file, and the file says what has to change before a page can make it.
//!
//! The two middleware offers are not mounted either. `auth.session` and
//! `auth.require_authenticated` are names with descriptions until the core's
//! request hooks land; `MiddlewareCatalog` has no `mount` and the pattern is
//! the application's decision in any case.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{plugin_line, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

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
        .krate(Dep::brocanth("brocanth-auth"))
        // Only for the order of the `[auth]` section and the builder line:
        // the cookie name and the key's variable come out of the file the
        // config plugin loaded, so this belongs under it rather than above
        // it. Not a requirement -- without the config plugin `main` builds
        // the settings from a literal and the compiler checks it.
        .after("config")
        .absent(
            "brocanth-auth",
            "no accounts; nothing hashes a password and nothing signs a session cookie",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/auth/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod auth;");
        s.at(Slot::AppPlugins)
            .block(plugin_line(project, "auth", "Auth"));
        // The services are read at startup, so a brand-new project uses the
        // value it was given rather than warning that nothing does. It also
        // prints the provider count, which is zero, because the number a
        // reader is surprised by is the one they go and look up.
        s.at(Slot::AppContext).block(
            "    let auth: &crate::plugins::auth::Services = cx.extract();\n\
            \x20   eprintln!(\n\
            \x20       \"auth: session cookie {}, {} provider(s)\",\n\
            \x20       auth.sessions().config().cookie_name,\n\
            \x20       auth.providers().len()\n\
            \x20   );"
                .to_string(),
        );
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("config") {
            s.at(Slot::ConfigSections).block(section(project));
        }
    }
}

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
fn section(project: &Project) -> String {
    let var = env_var(project);
    format!(
        "\n[auth]\n\
         # The *name* of the variable that holds the session signing key, not the\n\
         # key. 32 bytes, base64url. `brocanth` will not invent one: a key minted at\n\
         # startup logs every session out on each restart and differs between the\n\
         # instances of a fleet, so a missing key stops the application instead.\n\
         #\n\
         #   export {var}=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')\n\
         signing_key_env = \"{var}\"\n\
         # Development, over plain HTTP on localhost. In production this is\n\
         # \"__Host-session\" with secure = true, and the two move together: a\n\
         # __Host- cookie that is not Secure is a cookie no browser stores.\n\
         cookie_name = \"session\"\n\
         secure = false\n\
         # Eight hours unattended, thirty days total. They answer different\n\
         # questions: idle bounds a browser nobody is sitting at, absolute bounds a\n\
         # stolen cookie that is being kept warm.\n\
         idle_timeout = 28800\n\
         absolute_timeout = 2592000\n"
    )
}

/// The environment variable the generated project reads its key from.
///
/// `MY_BLOG_SESSION_KEY`, from the same convention `src/app.rs` states for
/// every other variable this binary reads: one prefix, derived once, whatever
/// spelling the project name arrived in.
fn env_var(project: &Project) -> String {
    format!("{}_SESSION_KEY", project.ident().to_uppercase())
}

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "auth");
    let var = env_var(project);
    format!(
        r##"//! Accounts and sessions, as this application registers them.
//!
//! One interface, no providers yet, and a local account path that does not
//! leak who has one: `register`, `login`, `request_password_reset` and
//! `verify_email` answer the same shape and take the same time whether or not
//! the address exists, because the expensive hash runs on both paths.
//!
//! `brocanth-auth` is in `Cargo.toml` because this file is here, and through
//! no other route. Delete the registration in `src/app.rs` and delete this
//! file, and the application has no accounts and links nothing to hash a
//! password with.
//!
//! # Signing somebody in
//!
//! Everything is behind one context item, and a handler in an application
//! that never registered this plugin does not compile:
//!
//! ```ignore
//! use brocanth_auth::mfa::{{Admission, Enrolled, FactorPolicy, FactorSet}};
//!
//! let auth: &Services = cx.extract();
//!
//! // 1. The credential. `login` answers the same shape and takes the same
//! //    time whether or not the address has an account.
//! let who = auth.accounts().login(email, password).await?;
//!
//! // 2. What this account could prove. Nothing: `Mfa` needs a store trait
//! //    this application does not implement, so the enrolment is empty and
//! //    the constructor is named so that a reviewer stops on it. The day
//! //    there is an `MfaStore`, `Mfa::second_factors` answers this properly
//! //    and this line goes.
//! let enrolled =
//!     Enrolled::without_asking_the_store(who.account().id.clone(), FactorSet::empty());
//!
//! // 3. The policy, which consumes the proof. `Admitted` has no public
//! //    constructor, so this step cannot be skipped -- a sign-in that
//! //    forgot the second factor is a program that does not compile rather
//! //    than one that ships.
//! let Admission::Admit(admitted) =
//!     FactorPolicy::default().admit(&enrolled, who.into_proof())
//! else {{
//!     return Err(Error::new(401, "sign in"));
//! }};
//!
//! // 4. The session, which takes the admission by value.
//! let started = auth.sessions().begin_with_factors(admitted, None).await?;
//! // `started.cookie` is the `Set-Cookie` header to send back.
//! ```
//!
//! **A page cannot do that yet.** A handler here is
//! `Box<dyn Fn(&mut Ctx) -> Result<Response, Error>>`, which is synchronous,
//! and every method above is `async fn`. What runs today is a test, a
//! `manage` subcommand, and anything else of yours that already has a
//! runtime. The sign-in form arrives with the core's request hooks, which are
//! also what the two middleware offers below are waiting for.
//!
//! # Three decisions this file makes, each in one line
//!
//! They are type aliases so that changing one changes one line and nothing
//! else in the project changes type.
//!
//! 1. **`Store = MemoryStore`.** It is a `Mutex<HashMap>`. Accounts and
//!    sessions last exactly as long as the process, so a restart is a logout
//!    for everybody and a deploy loses every account. That is the one thing
//!    in this file to replace before anybody else uses this application: write
//!    an `AuthStore` over your database -- fourteen methods, of which the two
//!    that carry a requirement are a unique email and a `consume_token` that
//!    deletes and returns under one lock -- and change the line below.
//!
//!    When you do, the build moves: a pool is opened asynchronously and
//!    `Plugin::new` is not, so `configure` becomes `before_start` and reaches
//!    for the current runtime handle. `brocanth-cache` does it in about ten
//!    lines and they are worth copying rather than inventing.
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
//! Three steps, and the flow code never learns the provider's name.
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
//! Second factors, one-time codes and login alerts. Each needs a store trait
//! of its own -- `MfaStore`, `OtpStore`, `SignInLocationStore` -- and
//! `brocanth-auth` deliberately keeps all three off `Auth`, so that an
//! application wanting a password form does not have to implement a TOTP
//! table. An application that wants them builds them here, from the same
//! `Arc<Store>`, and puts them on the context itself: one line each, in this
//! file, where the decision is visible.

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

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.settings.cookie_name.is_empty() {{
            return Err(PluginError::new("[auth] cookie_name must not be empty"));
        }}
        // A `__Host-` cookie is `Secure` by definition, so a configuration
        // where the two disagree produces a cookie no browser stores and a
        // login that silently never takes. Caught here, at startup, where it
        // can still be answered.
        if self.settings.cookie_name.starts_with("__Host-") && !self.settings.secure {{
            return Err(PluginError::new(
                "[auth] a __Host- cookie is Secure by definition; set secure = true, \
                 or drop the prefix for plain-HTTP development",
            ));
        }}

        let name = self.settings.signing_key_env.clone();
        let raw = std::env::var(&name).map_err(|_| {{
            PluginError::new(format!(
                "[auth] ${{name}} is not set. It is 32 random bytes, base64url:\n  \
                 export {{name}}=$(head -c 32 /dev/urandom | base64 | tr '+/' '-_' | tr -d '=')\n\
                 There is no default: a key invented at startup would log every session \
                 out on each restart and differ between the instances of a fleet."
            ))
        }})?;
        // Rejected rather than stretched. Padding a four-character secret to
        // 32 bytes is how a deployment ends up with a forgeable cookie and no
        // warning anywhere.
        let signing_key = SigningKey::parse(&raw)
            .map_err(|e| PluginError::new(format!("[auth] ${{name}}: {{e}}")))?;

        let passwords = Passwords::recommended()
            .map_err(|e| PluginError::new(format!("[auth] argon2id: {{e}}")))?;

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
        // and `Plugin::new` is not async -- so that swap moves this block
        // into `before_start`, where it takes the current runtime handle and
        // blocks on a scoped thread. `brocanth-cache` does exactly that.
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
"##
    )
}
