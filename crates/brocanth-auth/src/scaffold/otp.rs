//! `auth-otp` -- sign in with a code mailed to the address.
//!
//! Two pages under `/account/otp`, their markup, and a small plugin that
//! holds the code's length, lifetime and attempt cap. The crate does the
//! hard part (`brocanth_auth::Otp`): eight digits, five guesses, ten
//! minutes, a guess that costs an argon2id check, and the same answer for
//! an address with no account.
//!
//! # Decisions
//!
//! * **A code, not a link.** A link in a mail is opened by every scanner
//!   that reads the mail, and it signs in whichever browser opens it. A
//!   code is typed into the browser that asked for it, so the sign-in stays
//!   there.
//! * **The mailbox stands in for the password.** A mailed code is a
//!   second-class factor in the crate, and `FactorPolicy::admit` refuses it
//!   alone. This module turns on `accepting_mailbox_as_primary`, in
//!   `auth_screens::admit`: a password reset by mail already makes the
//!   mailbox a way in, so a code by mail adds no new one. An account with
//!   TOTP still gives its TOTP code after the mailed one.
//! * **The challenge waits in a cookie for minutes.** It is not a secret:
//!   it names which code is being checked, and only the mailbox has the
//!   code. `Path=/account/otp` keeps it off every other request.
//! * **Settings, checked at start.** Length, lifetime and attempts are one
//!   coupled choice (`brocanth_auth::otp`); the plugin refuses to start
//!   with a value the crate refuses, so a bad value fails the deploy and
//!   not the first sign-in.

use brocanth_scaffold::snippet::{configurable, settings_derive};
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Manifest, Module, PgPool, Project, Scaffold, Setting,
    Slot,
};

use super::uses_postgres;

/// Sign-in by mailed code, as a choice inside `auth`.
pub struct AuthOtp;

impl Module for AuthOtp {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "auth-otp",
            "sign in with an eight-digit code mailed to the address, under /account/otp",
        )
        .contributes::<AgentsSection>(
            Contribution::new("auth-otp", "how a mailed code signs somebody in, and what keeps it safe"),
            true,
        )
        .category(Category::Routes)
        .category(Category::Enhancer)
        .part_of_but_off("auth")
        .because("the pages call `Otp`, which the `auth` services build")
        .needs(
            "auth-screens",
            "the code pages share its helpers, its markup rules and the sign-in that opens the session",
        )
        .needs("mail", "the code leaves by mail, and only the mailbox ever sees it")
        .recommends(
            "middleware-rate-limit",
            "each code request costs an argon2id hash and sends a mail; without a limit a stranger asks a thousand times a minute",
        )
        .uses::<PgPool>("accounts are in Postgres, so the generated end-to-end test is left out")
        .asked_in("doors", "Sign in with a code sent by mail")
        .after("auth-screens")
        .absent(
            "auth-otp",
            "no sign-in by mailed code; a person signs in with a password",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let (derive, note) = settings_derive(project, "auth_otp");
        let mut plugin = PLUGIN_RS
            .replace("@DERIVE@", &derive)
            .replace("@NOTE@", &note);
        if !uses_postgres(project) {
            plugin.push_str(TEST_RS);
        }
        s.file("src/plugins/auth_otp/mod.rs", plugin);
        s.at(Slot::PluginsMod).line("pub mod auth_otp;");
        configurable(s, project, "auth_otp", "AuthOtp", settings());
        for (path, text) in PAGES {
            s.file(*path, text.to_string());
        }
        for route in ["/account/otp", "/account/otp/code"] {
            s.at(Slot::PagesRoutes).line(route);
        }
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

fn settings() -> Vec<Setting> {
    vec![
        Setting::integer("digits", 8, "Digits in a code, 6 to 10.").note(
            "Each digit removed divides a guesser's work by ten. Eight digits and five\n\
             guesses is one chance in twenty million per mail.",
        ),
        Setting::integer(
            "ttl_secs",
            600,
            "How long a code works, in seconds, 60 to 3600.",
        ),
        Setting::integer("max_attempts", 5, "Wrong guesses a code survives, 1 to 10."),
    ]
}

/// What `auth_screens::admit` says with `auth-otp`: the line goes right
/// before the policy is asked.
pub(super) const ADMIT_SWAPS: &[(&str, &str)] = &[(
    "    match policy.admit(&enrolled, proof) {\n",
    "    // `auth-otp`: a code mailed to the address stands in for the password.\n    \
     let policy = policy.accepting_mailbox_as_primary();\n    \
     match policy.admit(&enrolled, proof) {\n",
)];

const PLUGIN_RS: &str = r#"//! Sign-in by mailed code: the code's length, lifetime and attempt cap,
//! and the cookie that carries a challenge between the two pages.
//!
//! The pages are `src/pages/account/otp/`. Keep two rules when you change
//! them:
//!
//! 1. The page that asks for a code answers alike for every address. Only
//!    the mail says whether the address has an account.
//! 2. A rate limit covers `POST /account/otp`. Each request costs an
//!    argon2id hash and sends a mail.

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};

use crate::pages::{Ctx, Error};
use crate::plugins::auth::{Clock, Services, Store};

/// The cookie that carries the challenge from one page to the next.
pub const CHALLENGE: &str = "auth_otp";

/// The code: its length, its lifetime and how many wrong guesses it
/// survives.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// Digits in a code, 6 to 10.
    pub digits: i64,
    /// How long a code works, in seconds, 60 to 3600.
    pub ttl_secs: i64,
    /// Wrong guesses a code survives, 1 to 10.
    pub max_attempts: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            digits: 8,
            ttl_secs: 600,
            max_attempts: 5,
        }
    }
}

/// What a page reads off the context.
#[derive(Clone, Debug)]
pub struct Codes {
    digits: u32,
    ttl_secs: i64,
    max_attempts: u32,
}

impl ContextItem for Codes {}

impl Codes {
    /// The one-time code service, with these settings.
    pub fn otp(
        &self,
        auth: &Services,
    ) -> Result<brocanth_auth::Otp<Store, Clock>, brocanth_auth::AuthError> {
        auth.otp()?
            .with_digits(self.digits)?
            .with_ttl(self.ttl_secs)?
            .with_max_attempts(self.max_attempts)
    }

    /// How long a code works, in seconds.
    pub fn ttl_secs(&self) -> i64 {
        self.ttl_secs
    }

    /// Digits in a code.
    pub fn digits(&self) -> u32 {
        self.digits
    }
}

pub struct AuthOtp {
    codes: Codes,
}

impl Plugin for AuthOtp {
    type Config = Settings;
    type Provides = ProvidesContext<Codes>;
    const NAME: &'static str = "auth_otp";

    fn new(config: Settings) -> Self {
        // Out of range is caught in `before_start`; a negative value reads
        // as 0 here, which the crate refuses there.
        let small = |value: i64| u32::try_from(value).unwrap_or(0);
        Self {
            codes: Codes {
                digits: small(config.digits),
                ttl_secs: config.ttl_secs,
                max_attempts: small(config.max_attempts),
            },
        }
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // The same checks the pages make, once, before the first request.
        let auth = lc.require_service::<Services>()?;
        if let Err(e) = self.codes.otp(auth) {
            return Err(PluginError::new(format!(
                "[auth_otp] {e}\n\
                 help: set digits (6 to 10), ttl_secs (60 to 3600) and max_attempts \
                 (1 to 10) under [auth_otp]"
            )));
        }
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.codes.clone());
    }
}

/// The code settings.
pub fn codes(ctx: &Ctx) -> Result<Codes, Error> {
    ctx.provided::<Codes, _>().cloned().ok_or_else(|| {
        Error::new(
            500,
            "the one-time code settings are not on this context\n\
             help: register the auth_otp plugin",
        )
    })
}

/// The `Set-Cookie` value that carries `challenge` to the code page, or
/// clears it when `challenge` is empty.
pub fn challenge_cookie(auth: &Services, challenge: &str, ttl_secs: i64) -> String {
    let secure = if auth.sessions().config().secure {
        "; Secure"
    } else {
        ""
    };
    let age = if challenge.is_empty() { 0 } else { ttl_secs };
    format!(
        "{CHALLENGE}={challenge}; Path=/account/otp; Max-Age={age}; HttpOnly; SameSite=Strict{secure}"
    )
}
"#;

/// Walks the code pages through the application, with the store in memory.
const TEST_RS: &str = r##"
#[cfg(test)]
mod tests {
    use brocanth::plugin_api::Plugin as _;
    use brocanth_auth::OtpDelivery;

    use crate::plugins::auth_screens::testing::{
        alert, credentials, text, without_csrf, Browser, PASSWORD,
    };

    /// A code for `email`, as the mail carries it, and its challenge.
    async fn mailed(browser: &Browser, email: &str) -> (String, String) {
        let codes = super::AuthOtp::new(super::Settings::default()).codes;
        let requested = codes
            .otp(&browser.auth())
            .expect("the code service")
            .request(email)
            .await
            .expect("a request");
        match requested.delivery {
            OtpDelivery::Send { code, .. } => {
                (requested.challenge.to_string(), code.as_str().to_string())
            }
            OtpDelivery::Nothing => panic!("{email} has an account"),
        }
    }

    /// A new account, and the page its sign-up lands on, so its message is
    /// read before the next page.
    async fn signed_up(browser: &mut Browser, email: &str) {
        let created = browser
            .post("/account/signup", &credentials(email, PASSWORD))
            .await;
        browser.follow(&created).await;
    }

    #[tokio::test]
    async fn the_code_forms_refuse_a_post_without_the_csrf_token() {
        let mut browser = Browser::new().await;
        for path in ["/account/otp", "/account/otp/code"] {
            let refused = browser.post_raw(path, "email=ada%40example.com").await;
            assert_eq!(refused.status, 403, "{path}");
        }
    }

    #[tokio::test]
    async fn asking_for_a_code_answers_alike_for_every_address() {
        let mut browser = Browser::new().await;
        signed_up(&mut browser, "ada@example.com").await;
        let mut pages = Vec::new();
        for email in [
            "ada%40example.com",
            "nobody%40example.com",
            "not+an+address",
        ] {
            let asked = browser
                .post("/account/otp", &format!("email={email}"))
                .await;
            assert_eq!(
                asked.header("location"),
                Some("/account/otp/code"),
                "{email}"
            );
            assert!(
                browser.cookie(super::CHALLENGE).is_some(),
                "a challenge for {email}"
            );
            pages.push(without_csrf(&browser.follow(&asked).await));
        }
        assert_eq!(pages[0], pages[1], "a known and an unknown address");
        assert_eq!(pages[0], pages[2], "a known address and a malformed one");
    }

    #[tokio::test]
    async fn a_mailed_code_signs_in_once() {
        let mut browser = Browser::new().await;
        signed_up(&mut browser, "ada@example.com").await;
        let (challenge, code) = mailed(&browser, "ada@example.com").await;
        browser.set_cookie(super::CHALLENGE, &challenge);

        let wrong = if code == "00000000" {
            "11111111"
        } else {
            "00000000"
        };
        let refused = browser
            .post("/account/otp/code", &format!("code={wrong}"))
            .await;
        assert_eq!(refused.status, 422);
        assert!(!browser.signed_in());

        let admitted = browser
            .post("/account/otp/code", &format!("code={code}"))
            .await;
        assert_eq!(
            admitted.header("location"),
            Some(crate::modules::auth_screens::MODULE.after_sign_in)
        );
        assert!(browser.signed_in(), "a session cookie");
        assert!(
            browser.cookie(super::CHALLENGE).is_none(),
            "the challenge cookie is cleared"
        );

        browser.post("/account/signout", "").await;
        browser.set_cookie(super::CHALLENGE, &challenge);
        let replayed = browser
            .post("/account/otp/code", &format!("code={code}"))
            .await;
        assert_eq!(replayed.status, 422, "a code works once");
        assert!(!browser.signed_in());
    }

    #[tokio::test]
    async fn a_code_dies_after_its_wrong_guesses() {
        let mut browser = Browser::new().await;
        signed_up(&mut browser, "ada@example.com").await;
        let (challenge, code) = mailed(&browser, "ada@example.com").await;
        browser.set_cookie(super::CHALLENGE, &challenge);
        let wrong = if code == "00000000" {
            "11111111"
        } else {
            "00000000"
        };
        let mut refusals = Vec::new();
        for _ in 0..super::Settings::default().max_attempts {
            refusals.push(
                browser
                    .post("/account/otp/code", &format!("code={wrong}"))
                    .await,
            );
        }
        let late = browser
            .post("/account/otp/code", &format!("code={code}"))
            .await;
        assert_eq!(late.status, 422, "the right code after the cap");
        assert!(!browser.signed_in());
        for refused in &refusals {
            assert_eq!(alert(refused), alert(&late), "one answer for every cause");
        }
    }

    #[tokio::test]
    async fn the_code_page_wants_a_challenge() {
        let mut browser = Browser::new().await;
        let none = browser.post("/account/otp/code", "code=12345678").await;
        assert_eq!(none.header("location"), Some("/account/otp"));
        assert!(text(&browser.get("/account/otp").await).contains("<form"));
    }

    #[test]
    fn a_setting_out_of_range_stops_the_start() {
        let auth = crate::app::build()
            .expect("the application builds")
            .core
            .context()
            .get::<crate::plugins::auth::Services, _>()
            .clone();
        let edits: [fn(&mut super::Settings); 3] = [
            |s| s.digits = 4,
            |s| s.ttl_secs = 7200,
            |s| s.max_attempts = 0,
        ];
        for edit in edits {
            let mut settings = super::Settings::default();
            edit(&mut settings);
            assert!(super::AuthOtp::new(settings).codes.otp(&auth).is_err());
        }
    }
}
"##;

const PAGES: &[(&str, &str)] = &[
    ("src/pages/account/otp/index.rs", ASK_RS),
    ("src/pages/account/otp/code.rs", CODE_RS),
    ("src/pages/account/otp/screens.rsx", SCREENS_RSX),
];

const SCREENS_RSX: &str = r#"// The markup of the code pages. `csrf` is the hidden input the CSRF hook
// minted for this request, already HTML, so it goes in with `{! }`.

fn Problem(text: &str) -> Element {
    if !text.is_empty() {
        <p role="alert">{text}</p>
    }
}

pub fn Ask(csrf: &str) -> Element {
    <section>
        <h1>Sign in with a code</h1>
        <form method="post" action="/account/otp">
            {! csrf}
            <label>Email <input type="email" name="email" autocomplete="email" required></label>
            <button>Email me a code</button>
        </form>
        <p><a href="/account/signin">Sign in with my password</a></p>
    </section>
}

pub fn Code(csrf: &str, digits: &str, error: &str) -> Element {
    <section>
        <h1>Enter the code from the mail</h1>
        <Problem text={error} />
        <form method="post" action="/account/otp/code">
            {! csrf}
            <label>The {digits}-digit code <input name="code" inputmode="numeric" autocomplete="one-time-code" required></label>
            <button>Sign in</button>
        </form>
        <p><a href="/account/otp">Send me a new code</a></p>
    </section>
}
"#;

const ASK_RS: &str = r#"//! `/account/otp` -- a sign-in code by mail, if the address has an account.

use brocanth_auth::OtpDelivery;
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, IntoResponse, Response};
use crate::plugins::auth_otp::{self as otp};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

#[derive(zod::Entry)]
pub struct Address {
    #[check(zod::Text::new().max(254))]
    email: String,
}

pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let csrf = csrf_field(ctx);
    html!(ctx, views::account::otp::screens::Ask { csrf: &csrf })
}

/// The same answer whatever the address: an unknown or malformed one
/// takes the same path, pays the same hash and gets the same cookie. Only
/// the mail says which it was, to its owner.
pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let codes = otp::codes(ctx)?;
    let email = screens::form::<Address>(ctx)?
        .map(|form| form.email)
        .unwrap_or_default();
    let requested = codes.otp(&auth)?.request(&email).await?;
    if let OtpDelivery::Send { email, code, .. } = &requested.delivery {
        let text = format!(
            "Your sign-in code is {}.\n\n\
             It works once, for {} minutes, in the browser that asked for it.\n\
             If you did not ask for it, ignore this mail: nobody signs in without it.\n",
            code.as_str(),
            codes.ttl_secs() / 60
        );
        screens::mail(ctx, email.as_str(), "Your sign-in code", text).await?;
    }
    screens::flash(
        ctx,
        Level::Info,
        "If that address has an account, a code is on its way.",
    );
    let cookie = otp::challenge_cookie(&auth, requested.challenge.as_str(), codes.ttl_secs());
    Ok(see_other("/account/otp/code").with_header("set-cookie", cookie))
}
"#;

const CODE_RS: &str = r#"//! `/account/otp/code` -- the code from the mail, then the session.
//!
//! The challenge waits in the `auth_otp` cookie. A wrong, spent, expired
//! or unknown code gets one answer: which it was is what a guesser wants.

use brocanth_auth::{AuthError, OtpChallengeId};
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, Response};
use crate::plugins::auth_otp::{self as otp, CHALLENGE};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

#[derive(zod::Entry)]
pub struct Presented {
    #[check(zod::Text::new().min(1).max(32))]
    code: String,
}

const WRONG: &str = "That code is wrong, spent or too old. Ask for a new one if it keeps failing.";

pub fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    again(ctx, "", 200)
}

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let codes = otp::codes(ctx)?;
    let Some(challenge) = screens::cookie(ctx, CHALLENGE) else {
        screens::flash(ctx, Level::Error, "That took too long. Ask for a new code.");
        return Ok(see_other("/account/otp"));
    };
    let Some(form) = screens::form::<Presented>(ctx)? else {
        return again(ctx, WRONG, 422);
    };
    let code: String = form.code.chars().filter(|c| !c.is_whitespace()).collect();
    let proof = match codes
        .otp(&auth)?
        .authenticate(&OtpChallengeId::new(challenge), &code)
        .await
    {
        Ok(proof) => proof,
        Err(AuthError::InvalidCredentials) => return again(ctx, WRONG, 422),
        Err(other) => return Err(other.into()),
    };
    let answer = screens::admit(ctx, &auth, proof).await?;
    Ok(answer.with_header("set-cookie", otp::challenge_cookie(&auth, "", 0)))
}

fn again(ctx: &mut Ctx, error: &str, status: u16) -> Result<Response, Error> {
    let csrf = csrf_field(ctx);
    let digits = otp::codes(ctx)?.digits().to_string();
    let mut page = html!(
        ctx,
        views::account::otp::screens::Code {
            csrf: &csrf,
            digits: &digits,
            error
        }
    )?;
    page.status = status;
    Ok(page)
}
"#;

const AGENTS: &str = "\n\
    ## Sign-in by mailed code\n\
    \n\
    `src/pages/account/otp/` asks for a code by mail and checks it; the\n\
    code's length, lifetime and attempts are `[auth_otp]`, read in\n\
    `src/plugins/auth_otp/mod.rs`. The session opens through\n\
    `auth_screens::admit`, which takes a mailed code as the primary factor.\n\
    \n\
    Rules when you change them:\n\
    \n\
    - Answer alike for every address on `/account/otp`. Say which it was\n\
    \x20 only in the mail.\n\
    - Keep a rate limit on `POST /account/otp`: each request hashes and\n\
    \x20 mails.\n\
    - Never put the code in a link, a log line or the page.\n";
