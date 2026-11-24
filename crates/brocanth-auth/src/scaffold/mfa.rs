//! `auth-mfa` -- second factors as a policy, and the pages that manage them.
//!
//! `auth-screens` already asks for a TOTP or recovery code at sign-in and
//! turns TOTP on at `/account/totp`. This module adds what makes the
//! factors a set an application decides about:
//!
//! * **who must give a second factor**, `[auth_mfa] require`: the accounts
//!   that enrolled one, every member of staff, or everybody;
//! * **a sign-in that owes a factor it lacks sets one up first**, at
//!   `/account/mfa/setup`, before any session opens;
//! * **a page for the factors**, `/account/mfa`: whether TOTP is on, the
//!   recovery codes left, a new set of codes, and TOTP off;
//! * **a QR code** of the `otpauth://` link, on both enrolment pages;
//! * **login alerts**: a sign-in from a network the account has not used in
//!   ninety days mails its owner, `[auth_mfa] alerts`.
//!
//! # Decisions
//!
//! * **The pages `auth-screens` has stay there.** A project with
//!   `auth-screens` alone already has two-step sign-in, and moving the
//!   code and TOTP pages would change every such project. This module
//!   adapts them: the policy in `admit`, the QR code on `/account/totp`.
//! * **The policy is a setting, read per sign-in.** `staff` reads the
//!   account's staff record (a superuser flag or a role) at each sign-in, so
//!   a role given today asks for a factor at the next sign-in. A setting and
//!   not an option, because staging may run with `enrolled` while
//!   production runs with `staff`.
//! * **Forced enrolment holds no session.** The password proof waits sealed
//!   in the same cookie as a pending second factor, and
//!   `AccountHolder::enrolling` reads the holder from it. No half-session
//!   exists that a page could forget to check.
//! * **A required factor cannot be turned off.** `/account/mfa/off`
//!   refuses an account the policy holds to a second factor.
//! * **Login alerts need the client address.** A request hook writes it to
//!   the request state, from `ProxyHeaders` when that hook trusted the peer,
//!   else from the socket. Without an address, nothing is judged and nothing
//!   is mailed: "unknown" read as "new" alerts on every sign-in.
//! * **The QR code is drawn here, as SVG, by `qrcode`.** No image crate, no
//!   script, no request to another site that would see the secret.

use brocanth_scaffold::snippet::{configurable_hooking, settings_derive, Hooking};
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Hook, Manifest, Module, PgPool, Project, Scaffold,
    Setting, Slot,
};

use super::uses_postgres;

/// Second factors as a policy, as a choice inside `auth`.
pub struct AuthMfa;

impl Module for AuthMfa {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "auth-mfa",
            "who must use a second factor, forced TOTP set-up, recovery codes, a QR code and login alerts",
        )
        .contributes::<AgentsSection>(
            Contribution::new("auth-mfa", "who must give a second factor, and the rules of the factor pages"),
            true,
        )
        .category(Category::Routes)
        .category(Category::Middleware)
        .category(Category::Enhancer)
        .part_of_but_off("auth")
        .because("the pages call `Mfa`, which the `auth` services build")
        .needs(
            "auth-screens",
            "the second factor is asked for on its code page, and its sign-in asks this module's policy",
        )
        .uses::<PgPool>("accounts are in Postgres, so the generated end-to-end test is left out")
        .hook(
            "client",
            "write the client address to the request state, for login alerts",
            Hook::required().phase(brocanth_scaffold::Phase::Identify),
        )
        .krate(
            Dep::version("qrcode", "0.14")
                .no_default_features()
                .feature("svg")
                .note("the QR code of a new TOTP key, as SVG; no image crate"),
        )
        .asked_in("doors", "Second factors: who must use one, recovery codes, alerts")
        .after("auth-screens")
        .absent(
            "auth-mfa",
            "no second-factor policy; an account uses TOTP only when its owner turns it on",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let (derive, note) = settings_derive(project, "auth_mfa");
        let mut plugin = PLUGIN_RS
            .replace("@DERIVE@", &derive)
            .replace("@NOTE@", &note);
        if !uses_postgres(project) {
            plugin.push_str(TEST_RS);
        }
        s.file("src/plugins/auth_mfa/mod.rs", plugin);
        s.at(Slot::PluginsMod).line("pub mod auth_mfa;");
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "auth-mfa",
                section: "auth_mfa",
                ty: "crate::plugins::auth_mfa::AuthMfa",
                config_ty: "crate::plugins::auth_mfa::Settings",
                prelude: String::new(),
                adjust: None,
            },
            vec![
                Setting::text(
                    "require",
                    "enrolled",
                    "Who must give a second factor: enrolled, staff or everyone.",
                )
                .note(
                    "enrolled: the accounts that turned one on. staff: those, and every\n\
                     superuser or role holder. everyone: every account; one with no factor\n\
                     sets TOTP up before its first session.",
                ),
                Setting::bool(
                    "alerts",
                    true,
                    "Mail the owner when a sign-in comes from a new network.",
                ),
            ],
        );
        for (path, text) in PAGES {
            s.file(*path, text.to_string());
        }
        for route in [
            "/account/mfa",
            "/account/mfa/off",
            "/account/mfa/recovery",
            "/account/mfa/setup",
        ] {
            s.at(Slot::PagesRoutes).line(route);
        }
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// What `auth_screens::admit` says with `auth-mfa`: the policy from the
/// setting, forced enrolment, and the login alert.
pub(super) const ADMIT_SWAPS: &[(&str, &str)] = &[
    (
        "    let policy = brocanth_auth::FactorPolicy::recommended();\n",
        "    // Who owes a second factor: `[auth_mfa] require`.\n    \
         let policy = crate::plugins::auth_mfa::policy(ctx, auth, proof.account()).await?;\n",
    ),
    (
        "        Admission::Admit(admitted) => {\n",
        "        Admission::Admit(admitted) => {\n            \
         let account = admitted.account().clone();\n",
    ),
    (
        "            flash(ctx, Level::Success, \"You are signed in.\");\n",
        "            // A sign-in from a network this account has not used: mail its owner.\n            \
         crate::plugins::auth_mfa::alert(ctx, auth, &account).await?;\n            \
         flash(ctx, Level::Success, \"You are signed in.\");\n",
    ),
    (
        "        Admission::EnrolmentRequired { .. } | Admission::Insufficient => {\n",
        "        Admission::EnrolmentRequired { proof } => {\n            \
         // The policy wants a second factor this account does not have: it\n            \
         // sets one up before the session opens.\n            \
         let ttl = crate::modules::auth_screens::MODULE.pending_secs;\n            \
         let sealed = auth.sessions().seal_proof(&proof, ttl as i64);\n            \
         Ok(see_other(\"/account/mfa/setup\").with_header(\n                \
         \"set-cookie\",\n                \
         format!(\n                    \
         \"{PENDING}={sealed}; Path=/account; Max-Age={ttl}; HttpOnly; SameSite=Strict{secure}\"\n                \
         ),\n            \
         ))\n        \
         }\n        \
         Admission::Insufficient => {\n",
    ),
];

const PLUGIN_RS: &str = r#"//! Second factors as a policy: who must give one, the QR code of a new
//! key, and the mail of a sign-in from a new network.
//!
//! The pages are `src/pages/account/mfa/`; the sign-in that asks
//! [`policy`] is `auth_screens::admit`. Keep two rules when you change
//! them:
//!
//! 1. A page that changes a factor takes an `AccountHolder`: an
//!    impersonated session cannot build one, and the crate asks it for a
//!    fresh elevation.
//! 2. Never show a recovery code twice. The store keeps their digests only.

use std::net::IpAddr;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};
use brocanth::plugin_hooks::{
    Flow, HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState,
};
use brocanth::plugin_order::{Order, Phase};
use brocanth_auth::{AccountId, AuthStore as _, FactorPolicy, LoginAlert, Staff};

use crate::pages::{Ctx, Error};
use crate::plugins::auth::Services;

/// Who must give a second factor, and whether a new network mails the
/// owner.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// `enrolled`, `staff` or `everyone`.
    pub require: String,
    /// Mail the owner when a sign-in comes from a new network.
    pub alerts: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            require: "enrolled".to_string(),
            alerts: true,
        }
    }
}

/// Who must give a second factor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Require {
    /// The accounts that turned one on.
    Enrolled,
    /// Those, and every superuser or role holder.
    Staff,
    /// Every account.
    Everyone,
}

impl Require {
    /// The setting's word, or `None`.
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "enrolled" => Some(Self::Enrolled),
            "staff" => Some(Self::Staff),
            "everyone" => Some(Self::Everyone),
            _ => None,
        }
    }
}

/// What a page reads off the context.
#[derive(Clone, Debug)]
pub struct Rules {
    /// Who must give a second factor.
    pub require: Require,
    /// Whether a new network mails the owner.
    pub alerts: bool,
}

impl ContextItem for Rules {}

/// The address the request came from, as the `client` hook read it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClientAddress(pub IpAddr);

pub struct AuthMfa {
    settings: Settings,
    rules: Option<Rules>,
}

impl Plugin for AuthMfa {
    type Config = Settings;
    type Provides = ProvidesContext<Rules>;
    const NAME: &'static str = "auth_mfa";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Settings) -> Self {
        Self {
            settings: config,
            rules: None,
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let Some(require) = Require::parse(&self.settings.require) else {
            return Err(PluginError::new(format!(
                "[auth_mfa] require is {:?}\n\
                 help: set it to enrolled, staff or everyone",
                self.settings.require
            )));
        };
        self.rules = Some(Rules {
            require,
            alerts: self.settings.alerts,
        });
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        if let Some(rules) = &self.rules {
            cx.insert(rules.clone());
        }
    }
}

impl HookSource for AuthMfa {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request(
            "client",
            Phase::Identify,
            Order::new().after_if_present("proxy-headers.read"),
            Client,
        );
        Ok(())
    }
}

/// Writes [`ClientAddress`]: what a trusted proxy reported, else the
/// socket's peer.
struct Client;

impl RequestHook for Client {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let forwarded = state
            .get::<brocanth_middleware::ClientInfo>()
            .and_then(|info| info.address);
        if let Some(address) = forwarded.or_else(|| brocanth_middleware::PeerAddress::of(request)) {
            state.insert(ClientAddress(address));
        }
        Ok(Flow::Continue)
    }
}

/// The rules, from the context.
pub fn rules(ctx: &Ctx) -> Result<Rules, Error> {
    ctx.provided::<Rules, _>().cloned().ok_or_else(|| {
        Error::new(
            500,
            "the second-factor rules are not on this context\nhelp: register the auth_mfa plugin",
        )
    })
}

/// The policy a sign-in of `account` answers to.
pub async fn policy(
    ctx: &Ctx,
    auth: &Services,
    account: &AccountId,
) -> Result<FactorPolicy, Error> {
    policy_for(&rules(ctx)?, auth, account).await
}

/// The same, from the rules.
pub async fn policy_for(
    rules: &Rules,
    auth: &Services,
    account: &AccountId,
) -> Result<FactorPolicy, Error> {
    let always = match rules.require {
        Require::Enrolled => false,
        Require::Everyone => true,
        Require::Staff => Staff::load(&**auth.store(), account)
            .await?
            .is_some_and(|staff| staff.is_staff()),
    };
    let policy = FactorPolicy::recommended();
    Ok(if always {
        policy.requiring_a_second_factor_always()
    } else {
        policy
    })
}

/// Whether the policy holds `account` to a second factor.
pub async fn required(ctx: &Ctx, auth: &Services, account: &AccountId) -> Result<bool, Error> {
    Ok(policy(ctx, auth, account).await?.second_factor()
        == brocanth_auth::SecondFactorRequirement::Always)
}

/// The `otpauth://` link as an SVG QR code, for a page to write with
/// `{! }`. Empty when the link does not fit in a QR code.
pub fn qr_svg(uri: &str) -> String {
    let Ok(code) = qrcode::QrCode::new(uri.as_bytes()) else {
        return String::new();
    };
    let svg = code
        .render::<qrcode::render::svg::Color<'_>>()
        .min_dimensions(200, 200)
        .build();
    // Without the XML declaration: the SVG goes inside an HTML page.
    match svg.find("<svg") {
        Some(start) => svg[start..].to_string(),
        None => String::new(),
    }
}

/// Mails the owner of `account` when this sign-in came from a network it
/// has not used for ninety days. Not on the first sign-in ever, and not
/// when the client address is unknown.
pub async fn alert(ctx: &Ctx, auth: &Services, account: &AccountId) -> Result<(), Error> {
    if !rules(ctx)?.alerts {
        return Ok(());
    }
    let address = ctx.hooked::<ClientAddress>().map(|a| a.0.to_string());
    let noted = auth
        .login_alerts()
        .note(account, address.as_deref())
        .await?;
    let LoginAlert::Unfamiliar {
        first_ever: false, ..
    } = noted
    else {
        return Ok(());
    };
    let Some(email) = auth
        .store()
        .account(account)
        .await
        .map_err(|e| Error::internal(e.to_string()))?
        .and_then(|a| a.email)
    else {
        return Ok(());
    };
    let link = crate::plugins::auth_screens::screens(ctx)?.link("/account/forgot");
    let text = format!(
        "Your account was signed into from a network it has not used before.\n\n\
         If it was you, there is nothing to do. If not, choose a new password now,\n\
         which ends every session:\n\n{link}\n"
    );
    crate::plugins::auth_screens::mail(ctx, email.as_str(), "A new sign-in to your account", text)
        .await
}
"#;

/// Walks the factor pages through the application, with the store in
/// memory.
const TEST_RS: &str = r##"
#[cfg(test)]
mod tests {
    use brocanth_auth::{AccountId, Clock as _, SystemClock, Totp, TotpSecret};

    use crate::plugins::auth_screens::testing::{credentials, text, Browser, PASSWORD};
    use crate::plugins::auth_screens::PENDING;

    use super::{policy_for, qr_svg, Require, Rules};

    /// The key between `<code>` and `</code>` on an enrolment page.
    fn key(page: &str) -> TotpSecret {
        let start = page.find("<code>").expect("the key") + "<code>".len();
        let end = start + page[start..].find("</code>").expect("closed");
        TotpSecret::from_base32(&page[start..end]).expect("base32")
    }

    fn code(secret: &TotpSecret, at: i64) -> String {
        Totp::interoperable().code_at(secret, at)
    }

    /// Every `<li><code>` on a page: the recovery codes it shows once.
    fn recovery(page: &str) -> Vec<String> {
        page.split("<li><code>")
            .skip(1)
            .map(|rest| rest[..rest.find("</code>").expect("closed")].to_string())
            .collect()
    }

    /// A new account, and the page its sign-up lands on, so its message is
    /// read before the next page.
    async fn signed_up(browser: &mut Browser, email: &str) {
        let created = browser
            .post("/account/signup", &credentials(email, PASSWORD))
            .await;
        browser.follow(&created).await;
    }

    #[test]
    fn the_qr_code_is_an_svg_to_put_in_a_page() {
        let svg = qr_svg("otpauth://totp/Shop:ada%40example.com?secret=JBSWY3DPEHPK3PXP");
        assert!(svg.starts_with("<svg"), "{svg}");
        assert!(!svg.contains("<?xml"));
    }

    #[tokio::test]
    async fn the_policy_follows_the_setting() {
        let browser = Browser::new().await;
        let auth = browser.auth();
        let account = AccountId::new("nobody");
        for (require, always) in [
            (Require::Enrolled, false),
            (Require::Staff, false),
            (Require::Everyone, true),
        ] {
            let rules = Rules {
                require,
                alerts: true,
            };
            let policy = policy_for(&rules, &auth, &account).await.expect("a policy");
            assert_eq!(
                policy.second_factor() == brocanth_auth::SecondFactorRequirement::Always,
                always,
                "{require:?}"
            );
        }
        assert!(Require::parse("sometimes").is_none());
    }

    #[tokio::test]
    async fn the_factor_pages_want_a_session() {
        let mut browser = Browser::new().await;
        assert_eq!(
            browser.get("/account/mfa").await.header("location"),
            Some("/account/signin")
        );
        for path in ["/account/mfa/recovery", "/account/mfa/off"] {
            let answered = browser.post(path, "").await;
            assert_eq!(
                answered.header("location"),
                Some("/account/signin"),
                "{path}"
            );
        }
        let setup = browser.get("/account/mfa/setup").await;
        assert_eq!(setup.header("location"), Some("/account/signin"));
    }

    /// A sign-in that owes a factor it lacks: the password's proof waits,
    /// TOTP is set up, then the next code opens the session.
    #[tokio::test]
    async fn a_sign_in_that_must_enrol_sets_up_totp_before_the_session() {
        let mut browser = Browser::new().await;
        signed_up(&mut browser, "ada@example.com").await;
        let auth = browser.auth();
        let proof = auth
            .accounts()
            .login("ada@example.com", PASSWORD)
            .await
            .expect("the password")
            .into_proof();
        let sealed = auth.sessions().seal_proof(&proof, 300);
        browser.set_cookie(PENDING, &sealed);

        let page = text(&browser.get("/account/mfa/setup").await);
        assert!(page.contains("<svg"), "a QR code: {page}");
        let secret = key(&page);
        let now = SystemClock.now();
        let on = browser
            .post(
                "/account/mfa/setup",
                &format!("code={}", code(&secret, now)),
            )
            .await;
        assert_eq!(on.status, 200);
        assert!(!recovery(&text(&on)).is_empty(), "the recovery codes, once");
        assert!(!browser.signed_in(), "no session yet");

        let admitted = browser
            .post(
                "/account/code",
                &format!("code={}", code(&secret, now + 30)),
            )
            .await;
        assert_eq!(
            admitted.header("location"),
            Some(crate::modules::auth_screens::MODULE.after_sign_in)
        );
        assert!(browser.signed_in());
    }

    /// TOTP on, new recovery codes, a password alone refused for the
    /// elevation, then TOTP off.
    #[tokio::test]
    async fn the_factor_page_renews_the_codes_and_turns_totp_off() {
        let mut browser = Browser::new().await;
        signed_up(&mut browser, "ada@example.com").await;
        browser
            .post("/account/signin", &credentials("ada@example.com", PASSWORD))
            .await;
        let password = format!("password={}", PASSWORD.replace(' ', "+"));
        browser.post("/account/elevate", &password).await;
        let secret = key(&text(&browser.get("/account/totp").await));
        let now = SystemClock.now();
        let on = browser
            .post("/account/totp", &format!("code={}", code(&secret, now)))
            .await;
        let first = recovery(&text(&on));
        assert!(!first.is_empty());

        let page = text(&browser.get("/account/mfa").await);
        assert!(page.contains("is on"), "{page}");

        // The elevation from before enrolment is still fresh.
        let renewed = browser.post("/account/mfa/recovery", "").await;
        assert_eq!(renewed.status, 200);
        let second = recovery(&text(&renewed));
        assert_eq!(second.len(), first.len());
        assert_ne!(first, second, "a new set");

        // With TOTP on, the password alone does not elevate; with the code
        // it does.
        let alone = browser
            .post(
                "/account/elevate",
                &format!("{password}&then=%2Faccount%2Fmfa"),
            )
            .await;
        assert_eq!(alone.status, 403);
        let with_code = browser
            .post(
                "/account/elevate",
                &format!(
                    "{password}&code={}&then=%2Faccount%2Fmfa",
                    code(&secret, now + 30)
                ),
            )
            .await;
        assert_eq!(with_code.header("location"), Some("/account/mfa"));

        let off = browser.post("/account/mfa/off", "").await;
        assert_eq!(off.header("location"), Some("/account/mfa"));
        let page = text(&browser.get("/account/mfa").await);
        assert!(page.contains("is off"), "{page}");
    }
}
"##;

const PAGES: &[(&str, &str)] = &[
    ("src/pages/account/mfa/index.rs", INDEX_RS),
    ("src/pages/account/mfa/recovery.rs", RECOVERY_RS),
    ("src/pages/account/mfa/off.rs", OFF_RS),
    ("src/pages/account/mfa/setup.rs", SETUP_RS),
    ("src/pages/account/mfa/screens.rsx", SCREENS_RSX),
];

const SCREENS_RSX: &str = r#"// The markup of the factor pages. `csrf` is the hidden input the CSRF
// hook minted, and `qr` the SVG this application drew: both are HTML
// already, so they go in with `{! }`. Nothing else here is raw.

fn Problem(text: &str) -> Element {
    if !text.is_empty() {
        <p role="alert">{text}</p>
    }
}

fn Codes(codes: &[String]) -> Element {
    <p>Keep these recovery codes somewhere safe. Each works once, and this page will not show them again.</p>
    <ul>
        for code in codes {
            <li><code>{code}</code></li>
        }
    </ul>
}

pub fn Factors(csrf: &str, on: bool, remaining: &str, required: bool, codes: &[String]) -> Element {
    <section>
        <h1>Two-step sign-in</h1>
        if !codes.is_empty() {
            <Codes codes={codes} />
        }
        if on {
            <p>Two-step sign-in is on. Recovery codes left: {remaining}.</p>
            <form method="post" action="/account/mfa/recovery">
                {! csrf}
                <button>Make new recovery codes</button>
            </form>
            if required {
                <p>This account must use two-step sign-in, so it stays on.</p>
            } else {
                <form method="post" action="/account/mfa/off">
                    {! csrf}
                    <button>Turn two-step sign-in off</button>
                </form>
            }
        } else {
            <p>Two-step sign-in is off.</p>
            <p><a href="/account/totp">Turn it on with an authenticator app</a></p>
        }
    </section>
}

pub fn Setup(csrf: &str, secret: &str, uri: &str, qr: &str, error: &str, codes: &[String]) -> Element {
    <section>
        <h1>Set up two-step sign-in</h1>
        if !codes.is_empty() {
            <Codes codes={codes} />
            <p><a href="/account/code">Continue: enter the next code your app shows</a></p>
        } else {
            <p>This account must use two-step sign-in. Set it up to finish signing in.</p>
            <Problem text={error} />
            if !qr.is_empty() {
                <figure>{! qr}</figure>
            }
            if !secret.is_empty() {
                <p>Scan the code, <a href={uri}>open it in the app</a>, or type the key <code>{secret}</code>.</p>
            }
            <form method="post" action="/account/mfa/setup">
                {! csrf}
                <label>The code the app shows <input name="code" inputmode="numeric" autocomplete="one-time-code" required></label>
                <button>Turn it on</button>
            </form>
        }
    </section>
}
"#;

const INDEX_RS: &str = r#"//! `/account/mfa` -- the second factors of this account: whether TOTP is
//! on, the recovery codes left, and the buttons that change them.

use brocanth_auth::{AccountHolder, FactorKind};
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, Response};
use crate::plugins::auth_mfa;
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

pub async fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    show(ctx, &[]).await
}

/// The page, with `codes` shown once when the page just made them.
pub async fn show(ctx: &mut Ctx, codes: &[String]) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let Some(session) = screens::signed_in(ctx, &auth).await else {
        screens::flash(ctx, Level::Info, "Sign in first.");
        return Ok(see_other("/account/signin"));
    };
    // An impersonated session reads no factor of the account it acts for.
    let holder = AccountHolder::of(&session)?;
    let mfa = auth.mfa()?;
    let on = mfa.enrolled(&holder).await?.contains(FactorKind::Totp);
    let remaining = mfa.recovery_codes_remaining(&holder).await?.to_string();
    let required = auth_mfa::required(ctx, &auth, &session.account).await?;
    let csrf = csrf_field(ctx);
    html!(
        ctx,
        views::account::mfa::screens::Factors {
            csrf: &csrf,
            on,
            remaining: &remaining,
            required,
            codes,
        }
    )
}
"#;

const RECOVERY_RS: &str = r#"//! `/account/mfa/recovery` -- a new set of recovery codes. The old set
//! stops working. Asks for a fresh elevation.

use brocanth_auth::{AccountHolder, AuthError};
use brocanth_middleware::Level;

use crate::pages::{Ctx, Error, Response};
use crate::plugins::auth_screens::{self as screens, see_other};

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let Some(session) = screens::signed_in(ctx, &auth).await else {
        screens::flash(ctx, Level::Info, "Sign in first.");
        return Ok(see_other("/account/signin"));
    };
    let holder = AccountHolder::of(&session)?;
    match auth.mfa()?.regenerate_recovery_codes(&holder).await {
        Ok(fresh) => {
            let codes: Vec<String> = fresh
                .codes()
                .iter()
                .map(|code| code.as_str().to_string())
                .collect();
            super::index::show(ctx, &codes).await
        }
        Err(AuthError::ElevationRequired) => Ok(see_other("/account/elevate?then=/account/mfa")),
        Err(other) => Err(other.into()),
    }
}
"#;

const OFF_RS: &str = r#"//! `/account/mfa/off` -- TOTP off, and the recovery codes with it. Refused
//! for an account the policy holds to a second factor. Asks for a fresh
//! elevation.

use brocanth_auth::{AccountHolder, AuthError};
use brocanth_middleware::Level;

use crate::pages::{Ctx, Error, Response};
use crate::plugins::auth_mfa;
use crate::plugins::auth_screens::{self as screens, see_other};

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let Some(session) = screens::signed_in(ctx, &auth).await else {
        screens::flash(ctx, Level::Info, "Sign in first.");
        return Ok(see_other("/account/signin"));
    };
    let holder = AccountHolder::of(&session)?;
    if auth_mfa::required(ctx, &auth, &session.account).await? {
        screens::flash(
            ctx,
            Level::Error,
            "This account must use two-step sign-in, so it stays on.",
        );
        return Ok(see_other("/account/mfa"));
    }
    match auth.mfa()?.disable_totp(&holder).await {
        Ok(()) => {
            screens::flash(ctx, Level::Success, "Two-step sign-in is off.");
            Ok(see_other("/account/mfa"))
        }
        Err(AuthError::ElevationRequired) => Ok(see_other("/account/elevate?then=/account/mfa")),
        Err(other) => Err(other.into()),
    }
}
"#;

const SETUP_RS: &str = r#"//! `/account/mfa/setup` -- TOTP for a sign-in the policy will not admit
//! without it.
//!
//! The password passed, and its proof waits sealed in the `auth_pending`
//! cookie for a few minutes. No session exists yet: the holder is read from
//! the proof. Once TOTP is on, the person enters the next code at
//! `/account/code`, which opens the session.

use brocanth_auth::{AccountHolder, AuthError, AuthStore as _, Verified};
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, Response};
use crate::plugins::auth_mfa;
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other, PENDING};

#[derive(zod::Entry)]
pub struct Confirm {
    #[check(zod::Text::new().min(6).max(16))]
    code: String,
}

/// The sign-in waiting in the cookie, or `None`.
fn pending(ctx: &Ctx, auth: &crate::plugins::auth::Services) -> Option<Verified> {
    screens::cookie(ctx, PENDING).and_then(|sealed| auth.sessions().unseal_proof(&sealed))
}

pub async fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let Some(proof) = pending(ctx, &auth) else {
        screens::flash(ctx, Level::Error, "That took too long. Sign in again.");
        return Ok(see_other("/account/signin"));
    };
    let holder = AccountHolder::enrolling(&proof)?;
    let label = auth
        .store()
        .account(proof.account())
        .await
        .map_err(|e| Error::internal(e.to_string()))?
        .and_then(|account| account.email)
        .map(|email| email.as_str().to_string())
        .unwrap_or_default();
    let issuer = screens::screens(ctx)?.issuer().to_string();
    match auth
        .mfa()?
        .begin_totp_enrolment(&holder, &issuer, &label)
        .await
    {
        Ok(enrolment) => {
            let secret = enrolment.secret.to_base32();
            let qr = auth_mfa::qr_svg(&enrolment.uri);
            page(ctx, &secret, &enrolment.uri, &qr, "", &[], 200)
        }
        // Set up already, in another tab: the code page finishes.
        Err(AuthError::AlreadyEnrolled) => Ok(see_other("/account/code")),
        Err(other) => Err(other.into()),
    }
}

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let Some(proof) = pending(ctx, &auth) else {
        screens::flash(ctx, Level::Error, "That took too long. Sign in again.");
        return Ok(see_other("/account/signin"));
    };
    let holder = AccountHolder::enrolling(&proof)?;
    let Some(form) = screens::form::<Confirm>(ctx)? else {
        return page(
            ctx,
            "",
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
            page(ctx, "", "", "", "", &codes, 200)
        }
        Err(AuthError::InvalidCredentials) => page(
            ctx,
            "",
            "",
            "",
            "That code does not match. Check the clock of your device, or reload for a new key.",
            &[],
            422,
        ),
        Err(AuthError::AlreadyEnrolled) => Ok(see_other("/account/code")),
        Err(other) => Err(other.into()),
    }
}

fn page(
    ctx: &mut Ctx,
    secret: &str,
    uri: &str,
    qr: &str,
    error: &str,
    codes: &[String],
    status: u16,
) -> Result<Response, Error> {
    let csrf = csrf_field(ctx);
    let mut page = html!(
        ctx,
        views::account::mfa::screens::Setup {
            csrf: &csrf,
            secret,
            uri,
            qr,
            error,
            codes,
        }
    )?;
    page.status = status;
    Ok(page)
}
"#;

const AGENTS: &str = "\n\
    ## Second factors\n\
    \n\
    Who must give a second factor is `[auth_mfa] require` (`enrolled`,\n\
    `staff`, `everyone`); `auth_mfa::policy` reads it at each sign-in, in\n\
    `auth_screens::admit`. The factor pages are `src/pages/account/mfa/`.\n\
    \n\
    Rules when you change them:\n\
    \n\
    - Change a factor only through a call that takes an `AccountHolder`.\n\
    \x20 Never build one with `without_a_session` in a page.\n\
    - Open a session only with the `Admitted` the policy gives. Never skip\n\
    \x20 `admit` for a sign-in path you add.\n\
    - Show recovery codes once, on the page that made them.\n";
