//! `impersonate` -- staff act as a customer, the session says so, and a
//! banner on every page says so too.
//!
//! The crate does the hard part (`brocanth_auth::impersonation`): both
//! identities on the session row for its whole life, and a type that
//! refuses an impersonated session the credentials (password, second
//! factors, recovery codes, elevation, a nested impersonation, provider
//! links). `Staff::impersonate` adds the staff rules. This module writes
//! the edges: two pages, a hooking plugin for the banner, the admin tab,
//! and an audit entry.
//!
//! # Decisions
//!
//! * **Only what the role grants.** The route asks `Staff::IMPERSONATE`,
//!   `impersonate.edit`: a level on the `impersonate` resource, which is
//!   the admin tab's RBAC key. A superuser holds it; the owner of
//!   `impersonate` gives it in the Roles tab, and nothing more.
//! * **No staff subject at all.** `Staff::impersonate` refuses every member
//!   of staff as a subject, superusers included, for every operator. That
//!   is stricter than "no superuser by a non-superuser": acting as another
//!   member of staff would hand the operator rights nobody gave them, and
//!   an impersonated session is never staff anyway.
//! * **The operator types the password first.** The crate wants an
//!   elevated operator session; the start page sends the operator to
//!   `/account/elevate?then=/impersonate`.
//! * **The banner is a response hook.** It goes into every HTML page after
//!   `<body>`, whatever renders the page, with a stop button that carries
//!   the CSRF token. The layout belongs to the project, so no module edits
//!   it. A request hook reads the session once and puts what the banner
//!   needs in the request state.
//! * **The admin tab lists, a page starts.** A panel answers with a body or
//!   a redirect and cannot set a cookie, so the tab's forms post to
//!   `/impersonate`.
//! * **Stopping deletes the session.** `Sessions::end_impersonation` hands
//!   back the operator's own cookie; nothing turns an impersonated session
//!   into an ordinary one.

use brocanth_scaffold::snippet::{hooking, hooking_line, integration};
use brocanth_scaffold::{
    AdminPanel, AgentsSection, Category, Contribution, Dep, Hook, Manifest, Module, PgPool,
    Project, Scaffold, Slot, Tab,
};

use super::uses_postgres;

/// Impersonation, as a choice on the `brocanth create` menu.
pub struct Impersonate;

const TYPE: &str = "crate::plugins::impersonate::Impersonate";

impl Module for Impersonate {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "impersonate",
            "staff act as a customer, with a banner on every page, the operator on the session and an audit entry",
        )
        .contributes::<AgentsSection>(
            Contribution::new("impersonate", "who may act as somebody, and what an impersonated session may not do"),
            true,
        )
        .contributes::<AdminPanel>(
            Tab::new(
                "impersonate",
                "an Impersonate tab that lists the accounts, each with a form that starts acting as it",
            ),
            true,
        )
        .category(Category::Routes)
        .category(Category::Middleware)
        .category(Category::Enhancer)
        .needs(
            "superuser",
            "only staff impersonate, and `superuser` reads the signed-in member of staff",
        )
        .needs(
            "auth-screens",
            "the operator types the password again at /account/elevate, and the pages share its helpers",
        )
        .adapts_to(
            "audit",
            "each start and stop is an entry in the change history, with the operator and the reason",
        )
        .uses::<PgPool>("accounts are in Postgres, so the generated end-to-end test is left out")
        .hook(
            "read",
            "read the session once, and note an impersonation for the banner",
            Hook::required().phase(brocanth_scaffold::Phase::Identify),
        )
        .hook(
            "banner",
            "put the impersonation banner, with its stop button, after <body> on every HTML page",
            Hook::on().phase(brocanth_scaffold::Phase::Identify),
        )
        .krate(
            Dep::brocanth("brocanth-auth")
                .feature_with("admin", "admin")
                .note("the Impersonate tab, with the admin"),
        )
        .asked_in("doors", "Staff act as a customer, with a banner and an audit entry")
        .after("superuser")
        .after("auth-screens")
        .after("audit")
        .absent(
            "impersonate",
            "no impersonation; support sees a customer's page only by asking for a screenshot",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let record = if project.has("audit") {
            RECORD_AUDIT
        } else {
            RECORD_PLAIN
        };
        let mut plugin = PLUGIN_RS.replace("@RECORD@", record);
        if !uses_postgres(project) {
            plugin.push_str(TEST_RS);
        }
        s.file("src/plugins/impersonate/mod.rs", plugin);
        s.at(Slot::PluginsMod).line("pub mod impersonate;");
        hooking(
            s,
            "impersonate",
            TYPE,
            format!(
                "{COMMENT}{}",
                hooking_line(TYPE, "crate::plugins::impersonate::Settings::default()")
            ),
        );
        s.hooking_instance("impersonate");
        if project.collector::<AdminPanel>().is_some() {
            integration(s, "impersonate", "admin", Slot::AdminPanels, ADMIN_TAB);
        }
        for (path, text) in PAGES {
            s.file(*path, text.to_string());
        }
        for route in ["/impersonate", "/impersonate/stop"] {
            s.at(Slot::PagesRoutes).line(route);
        }
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The comment above the registration, inside `plugins::register!`.
const COMMENT: &str = "
        // -- impersonate ---------------------------------------------------
        //
        // Notes an impersonated session on each request, and puts a banner
        // with a stop button on every HTML page it answers.
        //
";

/// The admin tab, behind `integrate.admin`.
const ADMIN_TAB: &str = "// Impersonate: every account, with a form that posts to /impersonate.
let store = late.cell(|ctx| {
    let auth = ctx.provided::<crate::plugins::auth::Services, _>()?;
    Some(std::sync::Arc::clone(auth.store()))
});
config = config.panel(brocanth_auth::admin::ImpersonatePanel::late(store));";

/// Without `audit`: a line in the log.
const RECORD_PLAIN: &str = r#"/// Notes a start or a stop. With the `audit` module, an entry in the
/// change history; here, a line in the log.
pub fn record(_ctx: &Ctx, what: &str, operator: &AccountId, subject: &AccountId, reason: &str) {
    crate::plugins::say!(
        info,
        "impersonate: {operator} {what} acting as {subject}: {reason}";
        %operator,
        %subject,
        %reason
    );
}
"#;

/// With `audit`: an entry in the change history.
const RECORD_AUDIT: &str = r#"/// Notes a start or a stop as an entry in the change history: the
/// operator is the actor, the subject is the row, the reason is the label.
pub fn record(ctx: &Ctx, what: &str, operator: &AccountId, subject: &AccountId, reason: &str) {
    let Some(trail) = ctx.provided::<brocanth_audit::Trail, _>() else {
        return;
    };
    trail.record(brocanth_audit::Entry {
        id: 0,
        at: trail.now(),
        actor: Some(operator.to_string()),
        source: "impersonate".to_string(),
        table: "brocanth_auth_sessions".to_string(),
        row: subject.to_string(),
        label: format!("{what}: {reason}"),
        kind: if what == "started" {
            brocanth_audit::Kind::Created
        } else {
            brocanth_audit::Kind::Deleted
        },
        fields: Vec::new(),
    });
}
"#;

const PLUGIN_RS: &str = r#"//! Impersonation: the banner on every page of an impersonated session,
//! and the note of each start and stop.
//!
//! The pages are `src/pages/impersonate/`. The crate refuses an
//! impersonated session the credentials: a page that changes a password, a
//! factor or a provider link takes an `AccountHolder`, which such a session
//! cannot build. Everything else the application lets the customer do, the
//! operator can do too: guard it with `brocanth-policy`.

use std::sync::{Arc, OnceLock};

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    AsyncRequestHook, BodyShape, Both, Flow, HookRegistrar, HookSource, OnAsyncRequest, OnResponse,
    Request, RequestState, ResponseHead, ResponseHook,
};
use brocanth::plugin_order::{Order, Phase};
use brocanth::plugin_routes::BoxFuture;
use brocanth_auth::{AccountId, AuthStore as _};

use crate::pages::Ctx;
use crate::plugins::auth::Services;

/// What the banner says, noted by the `read` hook.
#[derive(Clone, Debug)]
pub struct Acting {
    /// The account the operator acts as: its address, or its id.
    pub subject: String,
    /// The reason the operator typed.
    pub reason: String,
}

/// Nothing to set: who may act is a role, and how long an impersonated
/// session lives is `brocanth_auth::SessionConfig::impersonation_timeout`.
#[derive(Clone, Debug, Default)]
pub struct Settings {}

pub struct Impersonate {
    /// The auth services, set in `before_start`: with accounts in Postgres
    /// they exist only from then, and the hooks are installed before.
    services: Arc<OnceLock<Services>>,
}

impl Plugin for Impersonate {
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "impersonate";
    const INSTALLS_HOOKS: bool = true;

    fn new(_config: Settings) -> Self {
        Self {
            services: Arc::new(OnceLock::new()),
        }
    }

    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let services = lc.require_service::<Services>()?.clone();
        let _ = self.services.set(services);
        Ok(())
    }
}

impl HookSource for Impersonate {
    type Families = Both<OnAsyncRequest, OnResponse>;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        let read = Read {
            services: Arc::clone(&self.services),
        };
        registrar.request_async("read", Phase::Identify, Order::new(), read);
        registrar.response("banner", Phase::Identify, Order::new(), Banner);
        Ok(())
    }
}

/// Reads the session a request carries, and notes [`Acting`] when an
/// operator opened it. One read of the store, and only with a cookie.
struct Read {
    services: Arc<OnceLock<Services>>,
}

impl AsyncRequestHook for Read {
    fn before_dispatch<'a>(
        &'a self,
        request: &'a mut Request,
        state: &'a mut RequestState,
    ) -> BoxFuture<'a, Result<Flow, PluginError>> {
        Box::pin(async move {
            let (Some(auth), Some(cookies)) = (
                self.services.get(),
                request
                    .headers()
                    .get("cookie")
                    .and_then(|value| value.to_str().ok()),
            ) else {
                return Ok(Flow::Continue);
            };
            let Ok(session) = auth.sessions().from_cookie_header(cookies).await else {
                return Ok(Flow::Continue);
            };
            let Some(marker) = &session.impersonation else {
                return Ok(Flow::Continue);
            };
            let subject = auth
                .store()
                .account(&session.account)
                .await
                .ok()
                .flatten()
                .and_then(|account| account.email)
                .map_or_else(|| session.account.to_string(), |email| email.to_string());
            state.insert(Acting {
                subject,
                reason: marker.reason.clone(),
            });
            Ok(Flow::Continue)
        })
    }
}

/// Puts the banner after `<body ...>` in an HTML page of an impersonated
/// session.
struct Banner;

impl ResponseHook for Banner {
    fn on_headers(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    fn on_body(
        &self,
        _request: &Request,
        head: &mut ResponseHead,
        body: &mut Vec<u8>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let Some(acting) = state.get::<Acting>() else {
            return Ok(());
        };
        let html = head
            .headers
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/html"));
        if !html {
            return Ok(());
        }
        let Some(open) = find(body, b"<body") else {
            return Ok(());
        };
        let Some(end) = body[open..].iter().position(|b| *b == b'>') else {
            return Ok(());
        };
        let csrf = state
            .get::<brocanth_middleware::CsrfToken>()
            .map(|token| token.hidden_field())
            .unwrap_or_default();
        let banner = format!(
            "<aside aria-label=\"Impersonation\" role=\"status\"><p>You are acting as \
             <strong>{}</strong>: {}</p><form method=\"post\" action=\"/impersonate/stop\">\
             {csrf}<button>Stop acting as this account</button></form></aside>",
            escape(&acting.subject),
            escape(&acting.reason)
        );
        let at = open + end + 1;
        body.splice(at..at, banner.into_bytes());
        Ok(())
    }
}

/// Where `needle` starts in `haystack`, ignoring ASCII case.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

/// Text for HTML.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

@RECORD@"#;

/// Walks impersonation through the application, with the store in memory.
const TEST_RS: &str = r##"
#[cfg(test)]
mod tests {
    use brocanth_auth::{AccountId, AuthStore as _, EmailAddress, StaffStore as _};

    use crate::plugins::auth_screens::testing::{credentials, text, Browser, PASSWORD};

    async fn account(browser: &mut Browser, email: &str) -> AccountId {
        let created = browser
            .post("/account/signup", &credentials(email, PASSWORD))
            .await;
        browser.follow(&created).await;
        browser
            .auth()
            .store()
            .account_by_email(&EmailAddress::parse(email).expect("an address"))
            .await
            .expect("the store")
            .expect("the account")
            .id
    }

    /// A superuser, signed in, whose session typed its password again.
    async fn operator(browser: &mut Browser) -> AccountId {
        let id = account(browser, "op@example.com").await;
        browser
            .auth()
            .store()
            .set_superuser(&id, true)
            .await
            .expect("the store");
        browser
            .post("/account/signin", &credentials("op@example.com", PASSWORD))
            .await;
        id
    }

    fn password() -> String {
        format!("password={}", PASSWORD.replace(' ', "+"))
    }

    #[tokio::test]
    async fn the_start_and_stop_forms_refuse_a_post_without_the_csrf_token() {
        let mut browser = Browser::new().await;
        for path in ["/impersonate", "/impersonate/stop"] {
            let refused = browser.post_raw(path, "reason=x").await;
            assert_eq!(refused.status, 403, "{path}");
        }
    }

    #[tokio::test]
    async fn somebody_who_is_not_staff_cannot_start() {
        let mut browser = Browser::new().await;
        account(&mut browser, "ada@example.com").await;
        account(&mut browser, "eve@example.com").await;
        browser
            .post("/account/signin", &credentials("eve@example.com", PASSWORD))
            .await;
        assert_eq!(browser.get("/impersonate").await.status, 403);
        let refused = browser
            .post("/impersonate", "email=ada%40example.com&reason=curious")
            .await;
        assert_eq!(refused.status, 403);
    }

    #[tokio::test]
    async fn an_operator_acts_with_a_banner_and_stops() {
        let mut browser = Browser::new().await;
        account(&mut browser, "ada@example.com").await;
        operator(&mut browser).await;
        let start = "email=ada%40example.com&reason=TICKET-42";

        // The session has not typed the password again yet.
        let first = browser.post("/impersonate", start).await;
        assert_eq!(
            first.header("location"),
            Some("/account/elevate?then=/impersonate")
        );
        let elevated = browser
            .post(
                "/account/elevate",
                &format!("{}&then=%2Fimpersonate", password()),
            )
            .await;
        assert_eq!(elevated.header("location"), Some("/impersonate"));

        let started = browser.post("/impersonate", start).await;
        assert_eq!(started.header("location"), Some("/"));
        let page = text(&browser.get("/").await);
        assert!(page.contains("aria-label=\"Impersonation\""), "{page}");
        assert!(page.contains("ada@example.com"), "{page}");
        assert!(page.contains("TICKET-42"), "{page}");
        assert!(page.contains("action=\"/impersonate/stop\""), "{page}");

        // The credentials stay the customer's: no elevation, no factors.
        assert_ne!(
            browser
                .post("/account/elevate", &password())
                .await
                .header("location"),
            Some("/account/totp")
        );
        // An impersonated session is not staff, so it cannot nest.
        assert_eq!(browser.get("/impersonate").await.status, 403);

        let stopped = browser.post("/impersonate/stop", "").await;
        assert_eq!(stopped.header("location"), Some("/impersonate"));
        let back = browser.get("/impersonate").await;
        assert_eq!(back.status, 200, "the operator's own session again");
        assert!(!text(&back).contains("aria-label=\"Impersonation\""));
    }

    #[tokio::test]
    async fn a_member_of_staff_is_never_a_subject() {
        let mut browser = Browser::new().await;
        let other = account(&mut browser, "root@example.com").await;
        browser
            .auth()
            .store()
            .set_superuser(&other, true)
            .await
            .expect("the store");
        operator(&mut browser).await;
        browser
            .post(
                "/account/elevate",
                &format!("{}&then=%2Fimpersonate", password()),
            )
            .await;
        let refused = browser
            .post("/impersonate", "email=root%40example.com&reason=TICKET-1")
            .await;
        assert_eq!(refused.status, 422);
        let unknown = browser
            .post("/impersonate", "email=nobody%40example.com&reason=TICKET-2")
            .await;
        assert_eq!(unknown.status, 422);
    }
}
"##;

const PAGES: &[(&str, &str)] = &[
    ("src/pages/impersonate/index.rs", START_RS),
    ("src/pages/impersonate/stop.rs", STOP_RS),
    ("src/pages/impersonate/screens.rsx", SCREENS_RSX),
];

const SCREENS_RSX: &str = r#"// The markup of the start page. `csrf` is the hidden input the CSRF hook
// minted, already HTML, so it goes in with `{! }`.

fn Problem(text: &str) -> Element {
    if !text.is_empty() {
        <p role="alert">{text}</p>
    }
}

pub fn Start(csrf: &str, error: &str) -> Element {
    <section>
        <h1>Act as an account</h1>
        <p>The session records who you are and why. It cannot change a password, a second factor or a role.</p>
        <Problem text={error} />
        <form method="post" action="/impersonate">
            {! csrf}
            <label>The account's email <input type="email" name="email" required></label>
            <label>Reason, such as a ticket <input name="reason" maxlength="200" required></label>
            <button>Act as this account</button>
        </form>
    </section>
}
"#;

const START_RS: &str = r#"//! `/impersonate` -- a member of staff starts acting as an account.
//!
//! Needs `impersonate.edit` (a superuser holds it), a session that typed
//! its password again, a subject that is not staff, and a reason. The
//! admin's Impersonate tab posts here too, with the account's `id`.

use brocanth_auth::{AccountId, AuthError, AuthStore as _, EmailAddress, Staff};
use brocanth_middleware::Level;

use crate::pages::{html, views, Ctx, Error, Response};
use crate::plugins::auth_screens::{self as screens, csrf_field, see_other};

#[derive(zod::Entry)]
pub struct Subject {
    #[text(max = 512)]
    id: Option<String>,
    #[text(max = 254)]
    email: Option<String>,
    #[check(zod::Text::new().min(1).max(200))]
    reason: String,
}

/// The member of staff on this request, when it may impersonate.
async fn operator(ctx: &Ctx) -> Result<Staff, Error> {
    match crate::plugins::superuser::staff(ctx).await? {
        Some(staff) if staff.allows(Staff::IMPERSONATE) => Ok(staff),
        _ => Err(Error::new(
            403,
            format!(
                "acting as an account needs the permission {}",
                Staff::IMPERSONATE
            ),
        )),
    }
}

pub async fn handle(ctx: &mut Ctx) -> Result<Response, Error> {
    operator(ctx).await?;
    again(ctx, "", 200)
}

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let staff = operator(ctx).await?;
    let auth = screens::services(ctx)?;
    let Some(session) = screens::signed_in(ctx, &auth).await else {
        return Ok(see_other("/account/signin"));
    };
    let Some(form) = screens::form::<Subject>(ctx)? else {
        return again(ctx, "Name the account and give a reason.", 422);
    };
    let subject = match (form.id, form.email) {
        (Some(id), _) if !id.is_empty() => Some(AccountId::new(id)),
        (_, Some(email)) => match EmailAddress::parse(&email) {
            Ok(address) => auth
                .store()
                .account_by_email(&address)
                .await
                .map_err(|e| Error::internal(e.to_string()))?
                .map(|account| account.id),
            Err(_) => None,
        },
        _ => None,
    };
    let Some(subject) = subject else {
        return again(ctx, "No account has that address.", 422);
    };
    let target = Staff::load(&**auth.store(), &subject).await?;
    match target {
        None => return again(ctx, "No account has that address.", 422),
        Some(target) if target.is_staff() => {
            return again(ctx, "A member of staff cannot be impersonated.", 422)
        }
        Some(_) => {}
    }
    match staff
        .impersonate(&auth, &session, &subject, form.reason.clone())
        .await
    {
        Ok(started) => {
            crate::plugins::impersonate::record(
                ctx,
                "started",
                &session.account,
                &subject,
                &form.reason,
            );
            screens::flash(ctx, Level::Info, "You are acting as this account.");
            Ok(see_other("/").with_header("set-cookie", started.cookie.to_string()))
        }
        Err(AuthError::ElevationRequired) => Ok(see_other("/account/elevate?then=/impersonate")),
        Err(other) => Err(other.into()),
    }
}

fn again(ctx: &mut Ctx, error: &str, status: u16) -> Result<Response, Error> {
    let csrf = csrf_field(ctx);
    let mut page = html!(
        ctx,
        views::impersonate::screens::Start { csrf: &csrf, error }
    )?;
    page.status = status;
    Ok(page)
}
"#;

const STOP_RS: &str = r#"//! `/impersonate/stop` -- the operator stops acting as the account.
//!
//! The impersonated session is deleted, and the browser gets the
//! operator's own session back. A POST, from the banner's button.

use brocanth_auth::AuthError;
use brocanth_middleware::Level;

use crate::pages::{Ctx, Error, Response};
use crate::plugins::auth_screens::{self as screens, see_other};

pub async fn handle_post(ctx: &mut Ctx) -> Result<Response, Error> {
    let auth = screens::services(ctx)?;
    let Some(session) = screens::signed_in(ctx, &auth).await else {
        return Ok(see_other("/account/signin"));
    };
    let Some(marker) = session.impersonation.clone() else {
        return Ok(see_other("/"));
    };
    match auth.sessions().end_impersonation(&session).await {
        Ok(cookie) => {
            crate::plugins::impersonate::record(
                ctx,
                "stopped",
                &marker.operator,
                &session.account,
                &marker.reason,
            );
            screens::flash(ctx, Level::Info, "You are yourself again.");
            Ok(see_other("/impersonate").with_header("set-cookie", cookie.to_string()))
        }
        // The operator's own session ended meanwhile: sign in again.
        Err(AuthError::NoSession) => Ok(see_other("/account/signin")
            .with_header("set-cookie", auth.sessions().clearing_cookie().to_string())),
        Err(other) => Err(other.into()),
    }
}
"#;

const AGENTS: &str = "\n\
    ## Impersonation\n\
    \n\
    Staff with `impersonate.edit` act as a customer at `/impersonate` (and\n\
    from the admin's Impersonate tab); `/impersonate/stop` ends it. The\n\
    session keeps both identities, and a banner on every HTML page says so.\n\
    \n\
    Rules:\n\
    \n\
    - A page that changes a password, a second factor or a provider link\n\
    \x20 takes an `AccountHolder`; an impersonated session cannot build one.\n\
    - Anything else an impersonated session can do. Guard money, deletion\n\
    \x20 and other one-way actions with `brocanth-policy`, reading\n\
    \x20 `session.impersonation`.\n\
    - Never impersonate a member of staff, and never clear\n\
    \x20 `session.impersonation`: stopping deletes the session.\n";
