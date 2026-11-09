//! `auth-screens` -- the pages over the flows `auth` gives a project.
//!
//! `auth` writes the services and no page. This writes eight pages under
//! `src/pages/account/`, one `.rsx` file with their markup, and a small
//! plugin that holds the two values a page cannot guess: the address that
//! links in mail point at, and the name an authenticator app shows.
//!
//! # Decisions
//!
//! * **Pages, not a mounted router.** Each screen is a file the project
//!   owns, in the project's layout, with the project's renderer. A person
//!   who wants another sign-in form edits `screens.rsx`. The admin mounts
//!   a router because its screens are the crate's; these are the
//!   application's.
//! * **It needs `rsx`, `input`, `middleware-csrf`, `mail` and `flash`.**
//!   `rsx` draws the forms, `input` reads them, `middleware-csrf` checks
//!   each post, `mail` carries the links, and `flash` says what happened
//!   on the page a post redirects to. Without any one of them a screen is
//!   broken or unsafe, so each is a requirement and not an adaptation.
//! * **A link in mail comes from `[auth_screens] base_url`, never from the
//!   request.** A reset link built from the `Host` header goes wherever the
//!   attacker's request said, with the victim's token in it.
//! * **A verification link asks for a click.** A GET only draws a button,
//!   and the POST spends the token: a mail scanner that follows each link
//!   must not confirm an address for a person who never saw the mail.
//! * **The same page for a known and an unknown address.** Sign-up, sign-in
//!   and the reset request answer alike whatever the store holds. The
//!   crate already makes the timing alike; the pages keep the words alike.
//! * **The half-finished sign-in is a sealed cookie for minutes.** The
//!   password passed and the code has not: `Sessions::seal_proof` signs the
//!   proof with the session key, and `Path=/account` keeps it off every
//!   other request.

use brocanth_scaffold::snippet::{configurable, module_ref, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Manifest, Module, Opt, Project, Scaffold, Setting, Slot};

use super::uses_postgres;

/// The account screens, as a choice inside `auth`.
pub struct AuthScreens;

impl Module for AuthScreens {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "auth-screens",
            "sign-up, sign-in, sign-out, reset, verify and TOTP pages under /account",
        )
        .category(Category::Routes)
        .category(Category::Enhancer)
        .part_of_but_off("auth")
        .because("the pages call the services the `auth` plugin puts on the context")
        .needs("rsx", "the screens are `.rsx` components inside the layout")
        .needs("input", "each form is read with `ctx.read`, which `input` writes")
        .needs(
            "middleware-csrf",
            "every form posts, and a post without the CSRF check is a post any site can make",
        )
        .needs("mail", "the verification and reset links leave by mail")
        .needs("flash", "a screen says what happened on the page it redirects to")
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "jobs",
            "accounts are in Postgres, so the generated end-to-end test is left out",
        )
        .adapts_to(
            "cache",
            "accounts are in Postgres, so the generated end-to-end test is left out",
        )
        .adapts_to(
            "auth-lockout",
            "sign-in counts wrong passwords and mails the owner of a locked account; a reset lifts the lock",
        )
        .asked_in("doors", "Sign-up, sign-in and reset pages")
        .after("auth")
        .after("flash")
        .after("mail")
        .after("auth-lockout")
        .option(Opt::text(
            "after_sign_in",
            "/",
            "Where a person lands after signing in.",
        ))
        .option(Opt::unsigned(
            "pending_secs",
            300,
            "How long a sign-in waits for its second factor, in seconds.",
        ))
        .absent(
            "auth-screens",
            "no account pages; the auth services are there and nothing calls them",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file(
            "src/plugins/auth_screens/mod.rs",
            plugin_rs(project, !uses_postgres(project)),
        );
        s.at(Slot::PluginsMod).line("pub mod auth_screens;");
        configurable(
            s,
            project,
            "auth_screens",
            "AuthScreens",
            vec![
                Setting::text(
                    "base_url",
                    "http://localhost:3000",
                    "The address links in mail start with: scheme, host, port.",
                )
                .note("Never the request's Host header: a reset link must not go where a request says."),
                Setting::text(
                    "issuer",
                    project.title,
                    "The name an authenticator app shows beside the code.",
                ),
            ],
        );
        if !project.has("config") {
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        let lockout = project.has("auth-lockout");
        for (path, text) in PAGES {
            let text = match *path {
                "src/pages/account/signin.rs" if lockout => with_lockout(
                    text,
                    &[
                        (
                            "use brocanth_auth::AuthError;",
                            "use brocanth_auth::Attempt;",
                        ),
                        (SIGNIN_PLAIN, SIGNIN_LOCKOUT),
                    ],
                ),
                "src/pages/account/reset/[token].rs" if lockout => {
                    with_lockout(text, &[(RESET_PLAIN, RESET_LOCKOUT)])
                }
                _ => text.to_string(),
            };
            s.file(*path, text);
        }
        s.file("src/pages/account/screens.rsx", SCREENS_RSX.to_string());
        for route in ROUTES {
            s.at(Slot::PagesRoutes).line(*route);
        }
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The routes, for the route-table test.
const ROUTES: &[&str] = &[
    "/account/code",
    "/account/forgot",
    "/account/reset/{token}",
    "/account/signin",
    "/account/signout",
    "/account/signup",
    "/account/totp",
    "/account/verify/{token}",
];

/// The plugin, the helpers every screen shares, and, when the store is in
/// memory, tests that walk every screen through the application.
fn plugin_rs(project: &Project, with_test: bool) -> String {
    let (derive, note) = settings_derive(project, "auth_screens");
    let mut out = PLUGIN_RS
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace("\"@ISSUER@\"", &format!("{:?}", project.title))
        .replace("@AFTER@", &module_ref("auth-screens", "after_sign_in"))
        .replace("@PENDING@", &module_ref("auth-screens", "pending_secs"));
    if project.has("auth-lockout") {
        out.push_str(LOCKED_MAIL_RS);
    }
    if with_test {
        let lockout = if project.has("auth-lockout") {
            LOCKOUT_TEST_RS
        } else {
            ""
        };
        out.push_str(&TEST_RS.replace("@LOCKOUT@", lockout));
    }
    out
}

/// `page` with each plain text swapped for its locked one. Panics when a
/// plain text is not in it, so a change to a page that misses the swap
/// fails every test.
fn with_lockout(page: &str, swaps: &[(&str, &str)]) -> String {
    swaps
        .iter()
        .fold(page.to_string(), |page, (plain, locked)| {
            assert!(
                page.contains(plain),
                "the page lost the text the lockout swaps: {plain}"
            );
            page.replacen(plain, locked, 1)
        })
}

/// The password check of the sign-in page, without and with the lockout.
const SIGNIN_PLAIN: &str =
    "    let proof = match auth.accounts().login(&form.email, &form.password).await {
        Ok(signed) => signed.into_proof(),
        Err(AuthError::InvalidCredentials) => return again(ctx, &form.email, WRONG, 422),
        Err(other) => return Err(other.into()),
    };
";

/// A locked account gets the words of a wrong password; only its owner
/// learns of the lock, by mail.
const SIGNIN_LOCKOUT: &str = "    let lockout = crate::plugins::auth_lockout::lockout(&auth)?;
    let attempt = lockout
        .login(auth.accounts(), &form.email, &form.password)
        .await?;
    let proof = match attempt {
        Attempt::Admitted(signed) => signed.into_proof(),
        Attempt::Refused => return again(ctx, &form.email, WRONG, 422),
        Attempt::JustLocked { account, .. } => {
            screens::mail_locked(ctx, &account, lockout.policy().lock_for()).await?;
            return again(ctx, &form.email, WRONG, 422);
        }
    };
";

/// The success arm of the reset page, without and with the lockout.
const RESET_PLAIN: &str = "        Ok(_) => {
";

/// A reset proves the mailbox, so it lifts the lock.
const RESET_LOCKOUT: &str = "        Ok(account) => {
            crate::plugins::auth_lockout::lockout(&auth)?
                .clear(&account)
                .await?;
";

/// With `auth-lockout`: the mail to the owner of an account that just
/// locked.
const LOCKED_MAIL_RS: &str = r#"
/// Tells the owner of an account that just locked, with the way back in.
pub async fn mail_locked(
    ctx: &Ctx,
    account: &brocanth_auth::Account,
    lock_for: i64,
) -> Result<(), Error> {
    let Some(email) = &account.email else {
        return Ok(());
    };
    let link = screens(ctx)?.link("/account/forgot");
    let text = format!(
        "Somebody entered a wrong password for your account too many times, \
         so it refuses to sign in for {} minutes. Nobody signed in.\n\n\
         If it was you, wait, or choose a new password now:\n\n{link}\n",
        lock_for / 60
    );
    mail(ctx, email.as_str(), "Your account is locked", text).await
}
"#;

const PLUGIN_RS: &str = r#"//! The account screens' plugin, and what every screen under
//! `src/pages/account/` shares.
//!
//! The pages are yours: change the words in `src/pages/account/screens.rsx`
//! and the flow in each page. Keep three rules:
//!
//! 1. Answer alike for an address that has an account and one that has
//!    not. A different word is how a stranger learns who signed up.
//! 2. Build a link in mail from [`Screens::link`], never from the request.
//! 3. Every form carries [`csrf_field`].

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};
use brocanth_auth::{Admission, AuthStore, FactorPolicy, SessionRecord, Verified};
use brocanth_middleware::{Flash, Level};

use crate::pages::{Ctx, Error, Response, Sent};
use crate::plugins::auth::Services;
use crate::plugins::mail::Outbox;

/// The cookie that carries a sign-in waiting for its second factor.
pub const PENDING: &str = "auth_pending";

/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// The address links in mail start with: `https://example.com`.
    pub base_url: String,
    /// The name an authenticator app shows beside the code.
    pub issuer: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:3000".to_string(),
            issuer: "@ISSUER@".to_string(),
        }
    }
}

/// What a screen reads off the context.
#[derive(Clone, Debug)]
pub struct Screens {
    base_url: String,
    issuer: String,
}

impl ContextItem for Screens {}

impl Screens {
    /// An absolute link to `path` on this site, for a mail.
    pub fn link(&self, path: &str) -> String {
        format!("{}{path}", self.base_url.trim_end_matches('/'))
    }

    /// The name an authenticator app shows.
    pub fn issuer(&self) -> &str {
        &self.issuer
    }
}

pub struct AuthScreens {
    screens: Screens,
}

impl Plugin for AuthScreens {
    type Config = Settings;
    type Provides = ProvidesContext<Screens>;
    const NAME: &'static str = "auth_screens";

    fn new(config: Settings) -> Self {
        Self {
            screens: Screens {
                base_url: config.base_url,
                issuer: config.issuer,
            },
        }
    }

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let url = &self.screens.base_url;
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(PluginError::new(format!(
                "[auth_screens] base_url is {url:?}, which is not an absolute address\n\
                 help: set it to the address people reach this site at, such as \
                 https://example.com"
            )));
        }
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.screens.clone());
    }
}

/// The auth services. A clone: one `Arc`, so a page can hold it across an
/// `.await` and still write to the context.
pub fn services(ctx: &Ctx) -> Result<Services, Error> {
    ctx.provided::<Services, _>().cloned().ok_or_else(|| {
        Error::new(
            500,
            "the auth services are not on this context\nhelp: register the auth plugin",
        )
    })
}

/// The base address and the issuer.
pub fn screens(ctx: &Ctx) -> Result<Screens, Error> {
    ctx.provided::<Screens, _>().cloned().ok_or_else(|| {
        Error::new(
            500,
            "the account screens' settings are not on this context\n\
             help: register the auth_screens plugin",
        )
    })
}

/// The hidden input a form carries for the CSRF check. HTML, so a
/// template writes it with `{! }`.
pub fn csrf_field(ctx: &Ctx) -> String {
    ctx.hooked::<brocanth_middleware::CsrfToken>()
        .map(|token| token.hidden_field())
        .unwrap_or_default()
}

/// The form, or `None` when a field broke its rule. A body nobody can read
/// is an error: no form sent it.
pub fn form<T: zod::Entry>(ctx: &Ctx) -> Result<Option<T>, Error> {
    match ctx.read::<T>() {
        Sent::Good(value) => Ok(Some(value)),
        Sent::Faulty(_) => Ok(None),
        Sent::Unreadable(why) => Err(Error::new(why.status(), why.to_string())),
    }
}

/// One cookie of the request, by name.
pub fn cookie(ctx: &Ctx, name: &str) -> Option<String> {
    ctx.headers()
        .filter(|(header, _)| header.eq_ignore_ascii_case("cookie"))
        .flat_map(|(_, line)| line.split(';'))
        .find_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            (key.trim() == name).then(|| value.trim().to_string())
        })
        .filter(|value| !value.is_empty())
}

/// The session this request carries, if it is a live one.
pub async fn signed_in(ctx: &Ctx, auth: &Services) -> Option<SessionRecord> {
    let value = cookie(ctx, &auth.sessions().config().cookie_name)?;
    auth.sessions().load(&value).await.ok()
}

/// A message for the next page.
pub fn flash(ctx: &mut Ctx, level: Level, text: &str) {
    if let Some(flash) = ctx.hooked_mut::<Flash>() {
        flash.push(level, text);
    }
}

/// `303 See Other`: after a post, the browser asks for `location` with GET.
pub fn see_other(location: &str) -> Response {
    Response::empty(303).with_header("location", location)
}

/// A mail with a plain-text body, through the outbox.
pub async fn mail(ctx: &Ctx, to: &str, subject: &str, text: String) -> Result<(), Error> {
    let outbox = ctx.provided::<Outbox, _>().cloned().ok_or_else(|| {
        Error::new(
            500,
            "no outbox on this context\nhelp: register the mail plugin",
        )
    })?;
    let message = outbox
        .message()
        .to(brocanth_mail::Address::parse(to)?)
        .subject(subject)
        .text(text)
        .build()?;
    outbox.deliver_later(message).await?;
    Ok(())
}

/// The email of the account a session belongs to, for a label.
pub async fn email_of(auth: &Services, session: &SessionRecord) -> Result<String, Error> {
    let account = auth
        .store()
        .account(&session.account)
        .await
        .map_err(|e| Error::new(500, e.to_string()))?;
    Ok(account
        .and_then(|account| account.email)
        .map(|email| email.as_str().to_string())
        .unwrap_or_default())
}

/// The second half of every sign-in: ask the policy, then open a session,
/// or wait for a second factor, or refuse.
pub async fn admit(ctx: &mut Ctx, auth: &Services, proof: Verified) -> Result<Response, Error> {
    let enrolled = auth.mfa()?.second_factors(&proof).await?;
    let secure = if auth.sessions().config().secure {
        "; Secure"
    } else {
        ""
    };
    match FactorPolicy::recommended().admit(&enrolled, proof) {
        Admission::Admit(admitted) => {
            // The session the browser had, if any, is replaced: a new
            // identifier at sign-in is the defence against fixation.
            let before = cookie(ctx, &auth.sessions().config().cookie_name)
                .and_then(|value| auth.sessions().unseal(&value));
            let started = auth
                .sessions()
                .begin_with_factors(admitted, before.as_ref())
                .await?;
            flash(ctx, Level::Success, "You are signed in.");
            Ok(see_other(@AFTER@)
                .with_header("set-cookie", started.cookie.to_string())
                .with_header(
                    "set-cookie",
                    format!("{PENDING}=; Path=/account; Max-Age=0; HttpOnly; SameSite=Strict{secure}"),
                ))
        }
        Admission::Incomplete { proof, .. } => {
            let ttl = @PENDING@;
            let sealed = auth.sessions().seal_proof(&proof, ttl as i64);
            Ok(see_other("/account/code").with_header(
                "set-cookie",
                format!(
                    "{PENDING}={sealed}; Path=/account; Max-Age={ttl}; HttpOnly; SameSite=Strict{secure}"
                ),
            ))
        }
        Admission::EnrolmentRequired { .. } | Admission::Insufficient => {
            flash(
                ctx,
                Level::Error,
                "This account needs a second factor that it does not have.",
            );
            Ok(see_other("/account/signin"))
        }
    }
}
"#;

/// Walks the screens through the application, with the store in memory:
/// every form, every token, and the answers a stranger must not tell apart.
const TEST_RS: &str = r##"
#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::{Body, Request, Response};

    use crate::plugins::auth::Services;

    const PASSWORD: &str = "correct horse battery";

    /// One application and one cookie jar: what a browser keeps between
    /// requests. Each `Browser` builds its own application, so each has its
    /// own memory store.
    struct Browser {
        app: crate::app::App,
        jar: Vec<(String, String)>,
    }

    impl Browser {
        async fn new() -> Self {
            let mut browser = Self {
                app: crate::app::build().expect("the application builds"),
                jar: Vec::new(),
            };
            let first = browser.get("/account/signup").await;
            assert_eq!(first.status, 200);
            assert!(browser.cookie("csrf_token").is_some(), "a CSRF cookie");
            browser
        }

        fn auth(&self) -> Services {
            self.app.core.context().get::<Services, _>().clone()
        }

        fn cookie(&self, name: &str) -> Option<String> {
            self.jar
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }

        fn set_cookie(&mut self, name: &str, value: &str) {
            self.jar.retain(|(key, _)| key != name);
            self.jar.push((name.to_string(), value.to_string()));
        }

        async fn send(&mut self, mut request: Request) -> Response {
            if !self.jar.is_empty() {
                let line: Vec<String> = self.jar.iter().map(|(k, v)| format!("{k}={v}")).collect();
                request = request.with_header("cookie", &line.join("; "));
            }
            let response = crate::app::handle(&self.app, request).await;
            for (name, line) in response.headers() {
                if !name.eq_ignore_ascii_case("set-cookie") {
                    continue;
                }
                let pair = line.split(';').next().unwrap_or_default();
                let Some((key, value)) = pair.split_once('=') else {
                    continue;
                };
                self.jar.retain(|(k, _)| k != key);
                if !value.is_empty() && !line.contains("Max-Age=0") {
                    self.jar.push((key.to_string(), value.to_string()));
                }
            }
            response
        }

        async fn get(&mut self, path: &str) -> Response {
            self.send(Request::new(Verb::Get, path)).await
        }

        /// A form post with the CSRF field, as the page's form sends it.
        async fn post(&mut self, path: &str, form: &str) -> Response {
            let token = self.cookie("csrf_token").expect("a CSRF cookie");
            self.post_raw(path, &format!("_csrf_token={token}&{form}"))
                .await
        }

        async fn post_raw(&mut self, path: &str, body: &str) -> Response {
            let (path, query) = path.split_once('?').unwrap_or((path, ""));
            let mut request = Request::new(Verb::Post, path)
                .with_header("content-type", "application/x-www-form-urlencoded");
            request.query = query.to_string();
            request.body = body.to_string().into();
            self.send(request).await
        }

        /// The page a redirect points at, as the browser asks for it next.
        async fn follow(&mut self, response: &Response) -> String {
            assert_eq!(response.status, 303, "a redirect");
            let location = response.header("location").expect("a location").to_string();
            let page = self.get(&location).await;
            text(&page)
        }

        async fn sign_up(&mut self, email: &str) -> Response {
            self.post("/account/signup", &credentials(email, PASSWORD))
                .await
        }

        async fn sign_in(&mut self, email: &str, password: &str) -> Response {
            self.post("/account/signin", &credentials(email, password))
                .await
        }

        /// Whether the jar holds a session.
        fn signed_in(&self) -> bool {
            let name = self.auth().sessions().config().cookie_name.clone();
            self.cookie(&name).is_some()
        }
    }

    fn credentials(email: &str, password: &str) -> String {
        format!(
            "email={}&password={}",
            email.replace('@', "%40"),
            password.replace(' ', "+")
        )
    }

    fn text(response: &Response) -> String {
        match &response.body {
            Body::Bytes(bytes) => String::from_utf8_lossy(bytes).into_owned(),
            Body::Stream(_) => panic!("a page, not a stream"),
        }
    }

    /// The `<p role="alert">` of a form drawn again.
    fn alert(response: &Response) -> String {
        let body = text(response);
        let start = body.find(r#"<p role="alert">"#).expect("an alert");
        let rest = &body[start..];
        rest[..rest.find("</p>").expect("closed")].to_string()
    }

    /// A reset token, as the link in the reset mail carries it.
    async fn reset_token(browser: &Browser, email: &str) -> String {
        match browser
            .auth()
            .accounts()
            .request_password_reset(email)
            .await
            .expect("a reset")
            .outcome
        {
            brocanth_auth::accounts::ResetOutcome::Issue { token, .. } => {
                token.secret().to_string()
            }
            _ => panic!("{email} has an account"),
        }
    }

    const POSTS: &[&str] = &[
        "/account/signup",
        "/account/signin",
        "/account/code",
        "/account/signout",
        "/account/forgot",
        "/account/reset/a-token",
        "/account/verify/a-token",
        "/account/totp",
    ];

    #[tokio::test]
    async fn every_form_without_the_csrf_token_is_refused() {
        let mut browser = Browser::new().await;
        for path in POSTS {
            let none = browser.post_raw(path, "email=ada%40example.com").await;
            assert_eq!(none.status, 403, "{path} took a post with no token");
            let wrong = browser
                .post_raw(path, "_csrf_token=forged&email=ada%40example.com")
                .await;
            assert_eq!(wrong.status, 403, "{path} took a forged token");
        }
    }

    #[tokio::test]
    async fn every_page_draws_its_form_with_the_token_in_it() {
        let mut browser = Browser::new().await;
        let token = browser.cookie("csrf_token").expect("a CSRF cookie");
        for path in [
            "/account/signup",
            "/account/signin",
            "/account/code",
            "/account/signout",
            "/account/forgot",
            "/account/reset/a-token",
            "/account/verify/a-token",
        ] {
            let page = browser.get(path).await;
            assert_eq!(page.status, 200, "{path}");
            assert!(text(&page).contains(&token), "{path} has no CSRF field");
        }
    }

    #[tokio::test]
    async fn a_put_to_a_screen_is_not_a_post() {
        let mut browser = Browser::new().await;
        let token = browser.cookie("csrf_token").expect("a CSRF cookie");
        let mut request = Request::new(Verb::Put, "/account/signin")
            .with_header("content-type", "application/x-www-form-urlencoded");
        request.body = format!("_csrf_token={token}").into();
        assert_eq!(browser.send(request).await.status, 405);
    }

    #[tokio::test]
    async fn a_person_signs_up_then_signs_in_and_a_wrong_password_is_refused() {
        let mut browser = Browser::new().await;
        let signed_up = browser.sign_up("ada@example.com").await;
        assert_eq!(signed_up.header("location"), Some("/account/signin"));
        assert!(browser
            .follow(&signed_up)
            .await
            .contains("Check your inbox"));

        let wrong = browser.sign_in("ada@example.com", "not the password").await;
        assert_eq!(wrong.status, 422);
        assert!(!browser.signed_in());

        let signed_in = browser.sign_in("ada@example.com", PASSWORD).await;
        assert_eq!(signed_in.status, 303);
        assert_eq!(
            signed_in.header("location"),
            Some(crate::modules::auth_screens::MODULE.after_sign_in),
            "the option in the module file decides where a person lands"
        );
        assert!(browser.signed_in(), "a session cookie");

        let signed_out = browser.post("/account/signout", "").await;
        assert_eq!(signed_out.header("location"), Some("/"));
        assert!(!browser.signed_in(), "the session cookie is cleared");
    }

    #[tokio::test]
    async fn a_next_parameter_does_not_choose_where_sign_in_lands() {
        let mut browser = Browser::new().await;
        browser.sign_up("ada@example.com").await;
        for query in [
            "next=https%3A%2F%2Fevil.example",
            "return_to=%2F%2Fevil.example",
            "redirect=https://evil.example",
        ] {
            let path = format!("/account/signin?{query}");
            let answered = browser
                .post(&path, &credentials("ada@example.com", PASSWORD))
                .await;
            assert_eq!(answered.status, 303, "{query}");
            assert_eq!(
                answered.header("location"),
                Some(crate::modules::auth_screens::MODULE.after_sign_in),
                "{query}"
            );
        }
    }

    #[tokio::test]
    async fn a_taken_address_signs_up_with_the_same_answer_as_a_new_one() {
        let mut browser = Browser::new().await;
        let new = browser.sign_up("ada@example.com").await;
        let new_page = browser.follow(&new).await;
        let taken = browser.sign_up("ada@example.com").await;
        let taken_page = browser.follow(&taken).await;
        assert_eq!(new.header("location"), taken.header("location"));
        assert_eq!(new_page, taken_page, "the same words, the same page");
        assert_eq!(
            browser.sign_in("ada@example.com", PASSWORD).await.status,
            303,
            "the second sign-up did not change the password"
        );
    }

    #[tokio::test]
    async fn an_unknown_address_and_a_wrong_password_get_the_same_words() {
        let mut browser = Browser::new().await;
        browser.sign_up("ada@example.com").await;
        let wrong = browser.sign_in("ada@example.com", "not the password").await;
        let unknown = browser.sign_in("eve@example.com", "not the password").await;
        let malformed = browser.sign_in("not an address", "not the password").await;
        assert_eq!(
            (wrong.status, unknown.status, malformed.status),
            (422, 422, 422)
        );
        assert_eq!(alert(&wrong), alert(&unknown));
        assert_eq!(alert(&wrong), alert(&malformed));
    }

    #[tokio::test]
    async fn a_bad_form_is_drawn_again_with_its_value_escaped() {
        let mut browser = Browser::new().await;
        let short = browser
            .post("/account/signup", "email=ada%40example.com&password=short")
            .await;
        assert_eq!(short.status, 422);
        assert!(alert(&short).contains("12 characters"));

        let script = "%22%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E+x";
        let answered = browser
            .post(
                "/account/signup",
                &format!("email={script}&password=a+long+enough+password"),
            )
            .await;
        assert_eq!(answered.status, 422);
        let body = text(&answered);
        assert!(!body.contains("<script>alert(1)"), "{body}");
        assert!(
            body.contains("&lt;script&gt;"),
            "the value is drawn, escaped"
        );

        let page = browser
            .get("/account/verify/%22%3E%3Cscript%3Ealert(1)%3C%2Fscript%3E")
            .await;
        assert!(!text(&page).contains("<script>alert(1)"));
    }

    #[tokio::test]
    async fn a_reset_request_answers_alike_for_an_unknown_address() {
        let mut browser = Browser::new().await;
        let signed_up = browser.sign_up("ada@example.com").await;
        browser.follow(&signed_up).await;
        let mut pages = Vec::new();
        for email in ["ada%40example.com", "nobody%40example.com", ""] {
            let answered = browser
                .post("/account/forgot", &format!("email={email}"))
                .await;
            assert_eq!(
                answered.header("location"),
                Some("/account/signin"),
                "{email}"
            );
            pages.push(browser.follow(&answered).await);
        }
        assert_eq!(pages[0], pages[1], "a known and an unknown address");
        assert_eq!(pages[0], pages[2], "a known address and an empty field");
    }

    #[tokio::test]
    async fn a_reset_link_works_once_and_ends_every_session() {
        let mut browser = Browser::new().await;
        browser.sign_up("ada@example.com").await;
        browser.sign_in("ada@example.com", PASSWORD).await;
        assert!(browser.signed_in());
        let session = browser
            .cookie(&browser.auth().sessions().config().cookie_name)
            .expect("a session");
        let token = reset_token(&browser, "ada@example.com").await;
        let path = format!("/account/reset/{token}");

        let short = browser.post(&path, "password=short").await;
        assert_eq!(short.status, 422, "a short password is drawn again");

        let changed = browser.post(&path, "password=a+brand+new+password").await;
        assert_eq!(changed.header("location"), Some("/account/signin"));
        assert!(browser
            .follow(&changed)
            .await
            .contains("Your password is changed"));
        assert!(
            browser.auth().sessions().load(&session).await.is_err(),
            "the old session ended"
        );
        assert_eq!(
            browser.sign_in("ada@example.com", PASSWORD).await.status,
            422,
            "the old password is gone"
        );
        assert_eq!(
            browser
                .sign_in("ada@example.com", "a brand new password")
                .await
                .status,
            303
        );

        let again = browser.post(&path, "password=yet+another+password").await;
        assert_eq!(again.header("location"), Some("/account/forgot"));
        assert!(browser.follow(&again).await.contains("spent or too old"));
        assert_eq!(
            browser
                .sign_in("ada@example.com", "yet another password")
                .await
                .status,
            422,
            "a spent link changed nothing"
        );
    }

    #[tokio::test]
    async fn a_reset_link_of_one_account_is_not_a_verify_link() {
        let mut browser = Browser::new().await;
        browser.sign_up("ada@example.com").await;
        let token = reset_token(&browser, "ada@example.com").await;
        let answered = browser.post(&format!("/account/verify/{token}"), "").await;
        assert!(browser.follow(&answered).await.contains("spent or too old"));
    }

    #[tokio::test]
    async fn a_verify_link_needs_a_click_and_works_once() {
        use brocanth_auth::AuthStore as _;

        let mut browser = Browser::new().await;
        let accepted = browser
            .auth()
            .accounts()
            .register("ada@example.com", PASSWORD)
            .await
            .expect("register");
        let brocanth_auth::accounts::RegistrationNotice::Created {
            account,
            verification,
        } = accepted.notice
        else {
            panic!("a new account");
        };
        let path = format!("/account/verify/{}", verification.secret());
        let verified = |browser: &Browser| {
            let auth = browser.auth();
            let id = account.id.clone();
            async move {
                auth.store()
                    .account(&id)
                    .await
                    .expect("the store")
                    .expect("the account")
                    .email_verified
            }
        };

        assert_eq!(browser.get(&path).await.status, 200);
        assert!(!verified(&browser).await, "a GET confirms nothing");

        let confirmed = browser.post(&path, "").await;
        assert!(browser
            .follow(&confirmed)
            .await
            .contains("Your address is confirmed"));
        assert!(verified(&browser).await);

        let again = browser.post(&path, "").await;
        assert!(browser.follow(&again).await.contains("spent or too old"));
    }

    #[tokio::test]
    async fn the_code_page_wants_the_sealed_cookie_of_a_password_that_passed() {
        let mut browser = Browser::new().await;
        let none = browser.post("/account/code", "code=123456").await;
        assert_eq!(none.header("location"), Some("/account/signin"));
        assert!(browser.follow(&none).await.contains("took too long"));

        browser.set_cookie(super::PENDING, "forged.value");
        let forged = browser.post("/account/code", "code=123456").await;
        assert_eq!(forged.header("location"), Some("/account/signin"));
        assert!(!browser.signed_in());
    }

    #[tokio::test]
    async fn the_totp_page_wants_a_session() {
        let mut browser = Browser::new().await;
        let page = browser.get("/account/totp").await;
        assert_eq!(page.header("location"), Some("/account/signin"));
        let post = browser.post("/account/totp", "code=123456").await;
        assert_eq!(post.header("location"), Some("/account/signin"));
    }

    /// Two-step sign-in, from turning it on to a replayed code.
    #[tokio::test]
    async fn a_second_factor_is_asked_for_and_a_wrong_or_spent_code_is_refused() {
        use brocanth_auth::{Clock, SystemClock, Totp, TotpSecret};

        let mut browser = Browser::new().await;
        browser.sign_up("ada@example.com").await;
        browser.sign_in("ada@example.com", PASSWORD).await;
        let page = text(&browser.get("/account/totp").await);
        let start = page.find("<code>").expect("the key") + "<code>".len();
        let secret = &page[start..start + page[start..].find("</code>").expect("closed")];
        let secret = TotpSecret::from_base32(secret).expect("base32");
        let code = |at: i64| Totp::interoperable().code_at(&secret, at);
        let now = SystemClock.now();

        let wrong = if code(now) == "000000" {
            "111111"
        } else {
            "000000"
        };
        let refused = browser
            .post("/account/totp", &format!("code={wrong}"))
            .await;
        assert_eq!(refused.status, 422);
        let on = browser
            .post("/account/totp", &format!("code={}", code(now)))
            .await;
        assert_eq!(on.status, 200);
        let page = text(&on);
        let recovery: Vec<String> = page
            .split("<li><code>")
            .skip(1)
            .map(|rest| rest[..rest.find("</code>").expect("closed")].to_string())
            .collect();
        assert!(!recovery.is_empty(), "the recovery codes, once: {page}");

        // The password alone now leads to the code page, not a session.
        browser.post("/account/signout", "").await;
        let first = browser.sign_in("ada@example.com", PASSWORD).await;
        assert_eq!(first.header("location"), Some("/account/code"));
        assert!(!browser.signed_in());
        assert!(browser.cookie(super::PENDING).is_some());

        let wrong = browser
            .post("/account/code", &format!("code={wrong}"))
            .await;
        assert_eq!(wrong.status, 422);
        assert!(!browser.signed_in());

        // The code that turned it on is spent; the next step's code works.
        let next = code(now + 30);
        let replayed = browser
            .post("/account/code", &format!("code={}", code(now)))
            .await;
        assert_eq!(replayed.status, 422, "the enrolment code is spent");
        let admitted = browser.post("/account/code", &format!("code={next}")).await;
        assert_eq!(
            admitted.header("location"),
            Some(crate::modules::auth_screens::MODULE.after_sign_in)
        );
        assert!(browser.signed_in());
        assert!(
            browser.cookie(super::PENDING).is_none(),
            "the pending cookie is cleared"
        );

        // A recovery code works once.
        browser.post("/account/signout", "").await;
        browser.sign_in("ada@example.com", PASSWORD).await;
        let replayed = browser.post("/account/code", &format!("code={next}")).await;
        assert_eq!(replayed.status, 422, "a TOTP code works once");
        let recovered = browser
            .post("/account/code", &format!("code={}", recovery[0]))
            .await;
        assert_eq!(recovered.status, 303);
        assert!(browser.signed_in());
        browser.post("/account/signout", "").await;
        browser.sign_in("ada@example.com", PASSWORD).await;
        let spent = browser
            .post("/account/code", &format!("code={}", recovery[0]))
            .await;
        assert_eq!(spent.status, 422, "a recovery code works once");
    }
@LOCKOUT@}
"##;

/// With `auth-lockout`: the lock, seen through the sign-in page.
const LOCKOUT_TEST_RS: &str = r#"
    #[tokio::test]
    async fn a_locked_account_answers_like_a_wrong_password_until_a_reset() {
        let mut browser = Browser::new().await;
        browser.sign_up("ada@example.com").await;
        let policy = crate::plugins::auth_lockout::policy().expect("a policy");
        let mut refusals = Vec::new();
        for _ in 0..policy.max_failures() {
            refusals.push(browser.sign_in("ada@example.com", "not the password").await);
        }
        let locked = browser.sign_in("ada@example.com", PASSWORD).await;
        assert_eq!(
            locked.status, 422,
            "the right password is refused while locked"
        );
        assert!(!browser.signed_in());
        let unknown = browser.sign_in("eve@example.com", PASSWORD).await;
        for answered in refusals.iter().chain([&locked, &unknown]) {
            assert_eq!(answered.status, 422);
            assert_eq!(
                alert(answered),
                alert(&unknown),
                "a lock tells a stranger nothing"
            );
        }

        let token = reset_token(&browser, "ada@example.com").await;
        let reset = browser
            .post(
                &format!("/account/reset/{token}"),
                "password=a+brand+new+password",
            )
            .await;
        assert_eq!(reset.header("location"), Some("/account/signin"));
        let back = browser
            .sign_in("ada@example.com", "a brand new password")
            .await;
        assert_eq!(back.status, 303, "the reset lifted the lock");
        assert!(browser.signed_in());
    }
"#;

/// The markup of every screen.
const SCREENS_RSX: &str = r#"// The markup of the account screens. One file, so the forms read alike;
// each page in this directory names its own component.
//
// `csrf` is the hidden input the CSRF hook minted for this request. It is
// HTML already, so it goes in with `{! }`. Nothing else here is raw.

fn Problem(text: &str) -> Element {
    if !text.is_empty() {
        <p role="alert">{text}</p>
    }
}

pub fn Signup(csrf: &str, email: &str, error: &str) -> Element {
    <section>
        <h1>Create an account</h1>
        <Problem text={error} />
        <form method="post" action="/account/signup">
            {! csrf}
            <label>Email <input type="email" name="email" value={email} autocomplete="email" required></label>
            <label>Password <input type="password" name="password" autocomplete="new-password" minlength="12" required></label>
            <button>Create the account</button>
        </form>
        <p><a href="/account/signin">I have an account</a></p>
    </section>
}

pub fn Signin(csrf: &str, email: &str, error: &str) -> Element {
    <section>
        <h1>Sign in</h1>
        <Problem text={error} />
        <form method="post" action="/account/signin">
            {! csrf}
            <label>Email <input type="email" name="email" value={email} autocomplete="username" required></label>
            <label>Password <input type="password" name="password" autocomplete="current-password" required></label>
            <button>Sign in</button>
        </form>
        <p><a href="/account/forgot">I forgot my password</a></p>
        <p><a href="/account/signup">Create an account</a></p>
    </section>
}

pub fn Signout(csrf: &str) -> Element {
    <section>
        <h1>Sign out</h1>
        <form method="post" action="/account/signout">
            {! csrf}
            <button>Sign out</button>
        </form>
    </section>
}

pub fn Forgot(csrf: &str) -> Element {
    <section>
        <h1>Reset your password</h1>
        <form method="post" action="/account/forgot">
            {! csrf}
            <label>Email <input type="email" name="email" autocomplete="email" required></label>
            <button>Send me a link</button>
        </form>
    </section>
}

pub fn Reset(csrf: &str, action: &str, error: &str) -> Element {
    <section>
        <h1>Choose a new password</h1>
        <Problem text={error} />
        <form method="post" action={action}>
            {! csrf}
            <label>New password <input type="password" name="password" autocomplete="new-password" minlength="12" required></label>
            <button>Change the password</button>
        </form>
    </section>
}

pub fn Verify(csrf: &str, action: &str) -> Element {
    <section>
        <h1>Confirm your address</h1>
        <form method="post" action={action}>
            {! csrf}
            <button>Confirm my address</button>
        </form>
    </section>
}

pub fn Code(csrf: &str, error: &str) -> Element {
    <section>
        <h1>Enter your code</h1>
        <Problem text={error} />
        <form method="post" action="/account/code">
            {! csrf}
            <label>Code from your app, or a recovery code <input name="code" autocomplete="one-time-code" required></label>
            <button>Continue</button>
        </form>
    </section>
}

pub fn Totp(csrf: &str, secret: &str, uri: &str, error: &str, codes: &[String]) -> Element {
    <section>
        <h1>Two-step sign-in</h1>
        if !codes.is_empty() {
            <p>Two-step sign-in is on. Keep these recovery codes somewhere safe. Each works once, and this page will not show them again.</p>
            <ul>
                for code in codes {
                    <li><code>{code}</code></li>
                }
            </ul>
        } else {
            <Problem text={error} />
            if !secret.is_empty() {
                <p>Add this account to your authenticator app: <a href={uri}>open it in the app</a>, or type the key <code>{secret}</code>.</p>
            }
            <form method="post" action="/account/totp">
                {! csrf}
                <label>The code the app shows <input name="code" inputmode="numeric" autocomplete="one-time-code" required></label>
                <button>Turn it on</button>
            </form>
            <p><a href="/account/totp">Start again with a new key</a></p>
        }
    </section>
}
"#;

/// The pages. Their text is the same in every project.
const PAGES: &[(&str, &str)] = &[
    ("src/pages/account/signup.rs", SIGNUP_RS),
    ("src/pages/account/signin.rs", SIGNIN_RS),
    ("src/pages/account/code.rs", CODE_RS),
    ("src/pages/account/signout.rs", SIGNOUT_RS),
    ("src/pages/account/forgot.rs", FORGOT_RS),
    ("src/pages/account/reset/[token].rs", RESET_RS),
    ("src/pages/account/verify/[token].rs", VERIFY_RS),
    ("src/pages/account/totp.rs", TOTP_RS),
];

const SIGNUP_RS: &str = r#"//! `/account/signup` -- a new account, and a mail to confirm the address.

use brocanth_auth::accounts::RegistrationNotice;
use brocanth_auth::AuthError;
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, Response};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

#[derive(zod::Entry)]
pub struct NewAccount {
    #[check(zod::Text::new().min(3).max(254))]
    email: String,
    #[check(zod::Text::new().min(12).max(1024))]
    password: String,
}

pub fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    again(ctx, "", "", 200)
}

/// The same answer for a new address and a taken one: a stranger must not
/// learn who has an account. The mail says which it was, to its owner.
pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let links = screens::screens(ctx)?;
    let Some(form) = screens::form::<NewAccount>(ctx)? else {
        return again(
            ctx,
            "",
            "Enter an email and a password of 12 characters or more.",
            422,
        );
    };
    let accepted = match auth.accounts().register(&form.email, &form.password).await {
        Ok(accepted) => accepted,
        Err(AuthError::InvalidEmail(_)) => {
            return again(ctx, &form.email, "This is not an email address.", 422)
        }
        Err(other) => return Err(other.into()),
    };
    let (subject, text) = match accepted.notice {
        RegistrationNotice::Created { verification, .. } => (
            "Confirm your address",
            format!(
                "Confirm your address with this link:\n\n{}\n",
                links.link(&format!("/account/verify/{}", verification.secret()))
            ),
        ),
        RegistrationNotice::AlreadyRegistered { .. } => (
            "Somebody tried to create an account",
            format!(
                "Somebody tried to create an account with this address, which has one.\n\
                 If it was you, reset your password:\n\n{}\n",
                links.link("/account/forgot")
            ),
        ),
    };
    screens::mail(ctx, &form.email, subject, text).await?;
    screens::flash(
        ctx,
        Level::Info,
        "Check your inbox to confirm your address, then sign in.",
    );
    Ok(see_other("/account/signin"))
}

fn again(ctx: &mut Ctx, email: &str, error: &str, status: u16) -> Result<Response, Error> {
    let csrf = csrf_field(ctx);
    let mut page = html!(
        ctx,
        views::account::screens::Signup {
            csrf: &csrf,
            email,
            error
        }
    )?;
    page.status = status;
    Ok(page)
}
"#;

const SIGNIN_RS: &str = r#"//! `/account/signin` -- a password, then a second factor if the account has
//! one.

use brocanth_auth::AuthError;

use crate::pages::{html, views, Ctx, Error, Response};
use crate::plugins::auth_screens::{self as screens, csrf_field};

#[derive(zod::Entry)]
pub struct Credentials {
    #[check(zod::Text::new().min(1).max(254))]
    email: String,
    #[check(zod::Text::new().min(1).max(1024))]
    password: String,
}

/// One sentence for a wrong password, an unknown address and an account
/// with no password: which one it was is what an attacker wants.
const WRONG: &str = "That email and password do not match an account.";

pub fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    again(ctx, "", "", 200)
}

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let Some(form) = screens::form::<Credentials>(ctx)? else {
        return again(ctx, "", "Enter your email and your password.", 422);
    };
    let proof = match auth.accounts().login(&form.email, &form.password).await {
        Ok(signed) => signed.into_proof(),
        Err(AuthError::InvalidCredentials) => return again(ctx, &form.email, WRONG, 422),
        Err(other) => return Err(other.into()),
    };
    screens::admit(ctx, &auth, proof).await
}

fn again(ctx: &mut Ctx, email: &str, error: &str, status: u16) -> Result<Response, Error> {
    let csrf = csrf_field(ctx);
    let mut page = html!(
        ctx,
        views::account::screens::Signin {
            csrf: &csrf,
            email,
            error
        }
    )?;
    page.status = status;
    Ok(page)
}
"#;

const SIGNOUT_RS: &str = r#"//! `/account/signout` -- a button, because a sign-out is a POST: a link
//! that signs out is a link any page can make a browser follow.

use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, IntoResponse, Response};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let csrf = csrf_field(ctx);
    html!(ctx, views::account::screens::Signout { csrf: &csrf })
}

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let cleared = match screens::signed_in(ctx, &auth).await {
        Some(session) => auth.sessions().revoke(&session.id).await?,
        None => auth.sessions().clearing_cookie(),
    };
    screens::flash(ctx, Level::Info, "You are signed out.");
    Ok(see_other("/").with_header("set-cookie", cleared.to_string()))
}
"#;

const FORGOT_RS: &str = r#"//! `/account/forgot` -- a reset link by mail, if the address has an account.

use brocanth_auth::accounts::ResetOutcome;
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, IntoResponse, Response};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

#[derive(zod::Entry)]
pub struct Address {
    #[check(zod::Text::new().min(1).max(254))]
    email: String,
}

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let csrf = csrf_field(ctx);
    html!(ctx, views::account::screens::Forgot { csrf: &csrf })
}

/// The same answer whether or not the address has an account.
pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let links = screens::screens(ctx)?;
    if let Some(form) = screens::form::<Address>(ctx)? {
        let requested = auth.accounts().request_password_reset(&form.email).await?;
        if let ResetOutcome::Issue { token, .. } = requested.outcome {
            let link = links.link(&format!("/account/reset/{}", token.secret()));
            let text = format!(
                "Choose a new password with this link:\n\n{link}\n\n\
                 If you did not ask for it, ignore this mail: your password stays.\n"
            );
            screens::mail(ctx, &form.email, "Reset your password", text).await?;
        }
    }
    screens::flash(
        ctx,
        Level::Info,
        "If that address has an account, a link to reset its password is on its way.",
    );
    Ok(see_other("/account/signin"))
}
"#;

const RESET_RS: &str = r#"//! `/account/reset/{token}` -- a new password, from the link in the mail.

use brocanth_auth::AuthError;
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, Response};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

#[derive(zod::Entry)]
pub struct NewPassword {
    #[check(zod::Text::new().min(12).max(1024))]
    password: String,
}

pub fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    again(ctx, "", 200)
}

/// A changed password ends every session of the account, which the crate
/// does by itself, so the person signs in again.
pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let token = ctx.require("token")?.to_string();
    let Some(form) = screens::form::<NewPassword>(ctx)? else {
        return again(ctx, "Choose a password of 12 characters or more.", 422);
    };
    match auth.accounts().reset_password(&token, &form.password).await {
        Ok(_) => {
            screens::flash(
                ctx,
                Level::Success,
                "Your password is changed. Sign in with it.",
            );
            Ok(see_other("/account/signin"))
        }
        Err(AuthError::InvalidToken) => {
            screens::flash(
                ctx,
                Level::Error,
                "This link is spent or too old. Ask for a new one.",
            );
            Ok(see_other("/account/forgot"))
        }
        Err(other) => Err(other.into()),
    }
}

fn again(ctx: &mut Ctx, error: &str, status: u16) -> Result<Response, Error> {
    let csrf = csrf_field(ctx);
    let action = ctx.path.clone();
    let mut page = html!(
        ctx,
        views::account::screens::Reset {
            csrf: &csrf,
            action: &action,
            error,
        }
    )?;
    page.status = status;
    Ok(page)
}
"#;

const VERIFY_RS: &str = r#"//! `/account/verify/{token}` -- the address is confirmed by a click, not
//! by the GET: a mail scanner follows every link it sees.

use brocanth_auth::AuthError;
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, IntoResponse, Response};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let csrf = csrf_field(ctx);
    let action = ctx.path.clone();
    html!(
        ctx,
        views::account::screens::Verify {
            csrf: &csrf,
            action: &action,
        }
    )
}

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let token = ctx.require("token")?.to_string();
    match auth.accounts().verify_email(&token).await {
        Ok(_) => screens::flash(ctx, Level::Success, "Your address is confirmed."),
        Err(AuthError::InvalidToken) => screens::flash(
            ctx,
            Level::Error,
            "This link is spent or too old. Sign in to ask for a new one.",
        ),
        Err(other) => return Err(other.into()),
    }
    Ok(see_other("/account/signin"))
}
"#;

const CODE_RS: &str = r#"//! `/account/code` -- the second factor, after the password passed.
//!
//! The password's proof waits in the `auth_pending` cookie, sealed with
//! the session key and good for a few minutes. Six digits are a TOTP code;
//! anything longer is a recovery code.

use brocanth_auth::AuthError;
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, Response};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other, PENDING};

#[derive(zod::Entry)]
pub struct Presented {
    #[check(zod::Text::new().min(6).max(64))]
    code: String,
}

pub fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    again(ctx, "", 200)
}

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let Some(proof) =
        screens::cookie(ctx, PENDING).and_then(|sealed| auth.sessions().unseal_proof(&sealed))
    else {
        screens::flash(ctx, Level::Error, "That took too long. Sign in again.");
        return Ok(see_other("/account/signin"));
    };
    let Some(form) = screens::form::<Presented>(ctx)? else {
        return again(ctx, "Enter the code your app shows.", 422);
    };
    let code: String = form.code.chars().filter(|c| !c.is_whitespace()).collect();
    let mfa = auth.mfa()?;
    let account = proof.account().clone();
    let second = if code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()) {
        mfa.verify_totp(&account, &code).await
    } else {
        mfa.verify_recovery_code(&account, &code)
            .await
            .map(|accepted| accepted.proof)
    };
    let second = match second {
        Ok(second) => second,
        Err(AuthError::InvalidCredentials) => {
            return again(ctx, "That code is wrong or already used.", 422)
        }
        Err(other) => return Err(other.into()),
    };
    screens::admit(ctx, &auth, proof.and(second)?).await
}

fn again(ctx: &mut Ctx, error: &str, status: u16) -> Result<Response, Error> {
    let csrf = csrf_field(ctx);
    let mut page = html!(ctx, views::account::screens::Code { csrf: &csrf, error })?;
    page.status = status;
    Ok(page)
}
"#;

const TOTP_RS: &str = r#"//! `/account/totp` -- turn on two-step sign-in with an authenticator app.
//!
//! The GET draws a new key and stores it unconfirmed; the POST confirms it
//! with a code from the app and shows the recovery codes, once.

use brocanth_auth::{AccountHolder, AuthError};
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, Response};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

#[derive(zod::Entry)]
pub struct Confirm {
    #[check(zod::Text::new().min(6).max(16))]
    code: String,
}

pub async fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let links = screens::screens(ctx)?;
    let Some(session) = screens::signed_in(ctx, &auth).await else {
        screens::flash(ctx, Level::Info, "Sign in first.");
        return Ok(see_other("/account/signin"));
    };
    let holder = AccountHolder::of(&session)?;
    let label = screens::email_of(&auth, &session).await?;
    let enrolment = auth
        .mfa()?
        .begin_totp_enrolment(&holder, links.issuer(), &label)
        .await?;
    let secret = enrolment.secret.to_base32();
    page(ctx, &secret, &enrolment.uri, "", &[], 200)
}

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let Some(session) = screens::signed_in(ctx, &auth).await else {
        screens::flash(ctx, Level::Info, "Sign in first.");
        return Ok(see_other("/account/signin"));
    };
    let holder = AccountHolder::of(&session)?;
    let Some(form) = screens::form::<Confirm>(ctx)? else {
        return page(
            ctx,
            "",
            "",
            "Enter the six digits your app shows.",
            &[],
            422,
        );
    };
    match auth
        .mfa()?
        .confirm_totp_enrolment(&holder, form.code.trim())
        .await
    {
        Ok(recovery) => {
            let codes: Vec<String> = recovery
                .codes()
                .iter()
                .map(|code| code.as_str().to_string())
                .collect();
            page(ctx, "", "", "", &codes, 200)
        }
        Err(AuthError::InvalidCredentials) => page(
            ctx,
            "",
            "",
            "That code does not match. Check the clock of your device, or start again.",
            &[],
            422,
        ),
        Err(other) => Err(other.into()),
    }
}

fn page(
    ctx: &mut Ctx,
    secret: &str,
    uri: &str,
    error: &str,
    codes: &[String],
    status: u16,
) -> Result<Response, Error> {
    let csrf = csrf_field(ctx);
    let mut page = html!(
        ctx,
        views::account::screens::Totp {
            csrf: &csrf,
            secret,
            uri,
            error,
            codes,
        }
    )?;
    page.status = status;
    Ok(page)
}
"#;

/// What the generated `AGENTS.md` says about the screens.
const AGENTS: &str = "\n\
    ## Account screens\n\
    \n\
    `src/pages/account/` holds the sign-up, sign-in, sign-out, reset,\n\
    verify and TOTP pages; their markup is `src/pages/account/screens.rsx`,\n\
    and the helpers are in `src/plugins/auth_screens/mod.rs`. Where a person\n\
    lands after signing in (`after_sign_in`) and how long a sign-in waits\n\
    for its second factor (`pending_secs`) are in\n\
    `src/modules/auth_screens.rs`.\n\
    \n\
    Rules when you change them:\n\
    \n\
    - Answer alike for an address with an account and one without, in the\n\
    \x20 words and the status. Say which it was only in the mail.\n\
    - Build a link in mail with `Screens::link`, which reads\n\
    \x20 `[auth_screens] base_url`. Never build one from the `Host` header.\n\
    - Every form carries `csrf_field(ctx)`, written with `{! }`.\n\
    - A page that changes state answers a POST, never a GET.\n\
    - Read who is signed in with `auth_screens::signed_in`. Do not trust a\n\
    \x20 cookie value you did not unseal.\n";
