//! The evidence: `brocanth-admin/tests/panels.rs`, rewritten against this
//! client.
//!
//! Those tests are §G6's own example. Each one builds a `Principal` by hand,
//! calls `Admin::dispatch` with a method and a path, pulls `.html()` out of
//! the admin's own response enum, and then searches a string — and one of
//! them carries a twenty-line HTML scanner (`current_tabs`) because searching
//! a string could not answer the question it needed to ask.
//!
//! Here the same panel is mounted into a real [`App`] and driven through the
//! router. What that buys, beyond shorter tests:
//!
//! 1. **The router runs.** `Admin::dispatch` was being handed a path
//!    directly, so nothing checked that `/admin/jobs` reaches the admin's
//!    `{*rest}` route at the prefix the application chose. Here it has to.
//! 2. **The hooks run.** A request hook that rewrites the path, a response
//!    hook that adds a header, and the 404 the router builds are all on this
//!    path, exactly as in `brocanth-server`.
//! 3. **The assertions are about the page.** `aria-current` on a link inside
//!    `<nav aria-label="Sections">`, a button by its label, a form by where
//!    it posts.
//!
//! The panel below is copied from `brocanth-admin/tests/panels.rs` unchanged,
//! because the point is to drive the same thing a different way.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_context::Base;
use brocanth::plugin_hooks::{NoHooks, Request, RequestState};
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::policy::{policy_fn, Action, Principal, Request as PolicyRequest};
use brocanth_admin::{AdminConfig, AdminPlugin, AdminSetup, Discovery, Fixture};
use brocanth_test::{TestClient, TestRequest};

/// What an application's authentication hook would have put in the request
/// state. `brocanth-admin/tests/plugin.rs` uses the same one.
struct CurrentUser {
    staff: bool,
}

/// Two paths and a button, which is every shape a panel has.
///
/// Copied from `brocanth-admin/tests/panels.rs`.
struct Queue {
    key: &'static str,
}

#[async_trait]
impl Panel for Queue {
    fn key(&self) -> &str {
        self.key
    }

    fn label(&self) -> String {
        "Background jobs".to_string()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if r.post {
            return Ok(Some(PanelPage::redirect(r.url("retries"))));
        }
        match r.path {
            "" => Ok(Some(PanelPage::body(
                "Dashboard",
                format!(
                    "<p>page {}</p>{}",
                    r.param("page").unwrap_or_default(),
                    // What `can_act` is for: the button is drawn only for
                    // somebody who may press it.
                    if r.can_act {
                        "<button>Empty</button>"
                    } else {
                        ""
                    }
                ),
            ))),
            "retries" => Ok(Some(PanelPage::body("Retries", "<p>two waiting</p>"))),
            // Anything else is this panel's own 404.
            _ => Ok(None),
        }
    }
}

/// The admin a panel lives in, before a policy is chosen.
fn base_config() -> AdminConfig {
    AdminConfig::new()
        .discovering(Discovery::new().under("blog::models"))
        // The fixture cannot run a row policy's SQL, and these tests are
        // about panels beside the models, not about the models' rows.
        .unscoped_rows_unsafe_for_development()
        .panel(Queue { key: "jobs" })
}

/// An application with that admin mounted at `/admin`, and a client over it.
///
/// The client seeds a staff `CurrentUser` into the request state, which is
/// where an authentication hook would have put one, and the admin's
/// `principal` closure reads it back out. Both halves have to be there before
/// a screen answers 200, which is the admin's own design — a config with no
/// policy is still `DenyAll`.
fn client(config: AdminConfig) -> TestClient<Base, NoHooks> {
    let setup = AdminSetup::new(Arc::new(Fixture::blog()), config).principal(
        |_req: &Request, state: &RequestState| {
            state
                .get::<CurrentUser>()
                .map_or_else(Principal::anonymous, |user| Principal::new(user))
        },
    );

    let mut app = AppBuilder::new()
        .plugin(AdminPlugin::new(setup))
        .build()
        .expect("an application");
    app.mount_routes::<AdminPlugin>("/admin")
        .expect("the admin mounts");

    TestClient::new(app).seeding(|state: &mut RequestState| {
        state.insert(CurrentUser { staff: true });
    })
}

/// A client whose policy lets staff do anything.
fn staff_client() -> TestClient<Base, NoHooks> {
    client(base_config().with_policy(policy_fn(is_staff)))
}

/// The policy an application would write: staff may act, nobody else is here.
fn is_staff(who: &Principal<'_>, _: PolicyRequest<'_>) -> bool {
    who.downcast::<CurrentUser>().is_some_and(|u| u.staff)
}

// ---------------------------------------------------------------------------
// 1. A panel renders inside the admin's chrome
// ---------------------------------------------------------------------------

/// Before, in `brocanth-admin/tests/panels.rs`:
///
/// ```ignore
/// let response = get(&admin, "/admin/jobs", "page=2");
/// assert_eq!(response.status(), 200);
/// let html = response.html().expect("a page");
/// assert!(html.contains("page 2"), "{html}");
/// assert!(html.contains("Background jobs"), "{html}");
/// assert!(html.contains("/admin/post"), "the models are still in the nav: {html}");
/// ```
///
/// The third line is the one worth replacing. `/admin/post` occurs in this
/// document in several places — a `href`, and whatever else a template
/// decides to put a path into — so `contains` cannot tell "the models are
/// still in the nav" from "that path is mentioned somewhere". Scoped to the
/// rail and asked as a link, it can.
#[test]
fn a_panel_renders_inside_the_admins_chrome() {
    let page = staff_client().get("/admin/jobs?page=2");

    page.assert_ok();
    // What a reader sees, not what a template indented.
    page.html().assert_text("page 2");

    let rail = page.html().within("nav", "aria-label", "Sections");
    rail.assert_link_to("/admin/jobs");
    rail.assert_link_to("/admin/post");
}

// ---------------------------------------------------------------------------
// 2. Which tab is the current one
// ---------------------------------------------------------------------------

/// Before: the same assertion, plus a twenty-line `current_tabs` helper that
/// split on `aria-label="Sections"`, then on `</nav>`, then on `<a `, then on
/// `href="`, then on `"`.
///
/// ```ignore
/// let page = get(&admin, "/admin/jobs", "");
/// let html = page.html().expect("a page");
/// assert!(current_tabs(html) == ["/admin/jobs"], "...: {html}");
/// ```
///
/// After: two lines and no helper. The scoping is [`Html::within`] and the
/// question is [`Html::links_with`], which is the question the helper was
/// written to ask.
#[test]
fn a_panel_is_a_tab_and_knows_when_it_is_the_current_one() {
    let client = staff_client();

    assert_eq!(current_tabs(&client, "/admin/jobs"), ["/admin/jobs"]);
    // And from a model screen, the model is.
    assert_eq!(current_tabs(&client, "/admin/post"), ["/admin/post"]);
}

/// The `href` of every link in the rail carrying `aria-current`.
///
/// Scoped to the rail, because a pagination link in a list is also the
/// current page and says so the same way. Both are correct HTML; only one of
/// them answers "which section am I in".
fn current_tabs(client: &TestClient<Base, NoHooks>, path: &str) -> Vec<String> {
    client
        .get(path)
        .html()
        .within("nav", "aria-label", "Sections")
        .links_with("aria-current", "page")
        .iter()
        .filter_map(|link| link.href().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// 3. Reading a panel and pressing its buttons are two questions
// ---------------------------------------------------------------------------

/// Before:
///
/// ```ignore
/// let page = get(&admin, "/admin/jobs", "");
/// assert_eq!(page.status(), 200);
/// assert!(
///     !page.html().expect("a page").contains("<button>Empty</button>"),
///     "a reader who may not act is told so, and draws no button"
/// );
/// let posted = block_on(admin.dispatch(
///     Principal::new(&staff), "POST", "/admin/jobs/retries/7/retry", "", &Submission::new(),
/// )).expect("dispatch");
/// assert_eq!(posted.status(), 403, "the button is refused before the panel runs");
/// ```
///
/// `contains("<button>Empty</button>")` is an assertion about four angle
/// brackets: it starts failing the day the button gains a `class` or a
/// `type`, and it would keep passing if the button moved into a comment.
/// [`Html::assert_no_button`] asks for a button labelled `Empty`, however it
/// is attributed.
#[test]
fn reading_a_panel_and_pressing_its_buttons_are_two_questions() {
    // A read-only operator: may see the queue, may not empty it.
    let client = client(base_config().with_policy(policy_fn(
        |who: &Principal<'_>, r: PolicyRequest<'_>| {
            is_staff(who, r) && r.action != Action::ActOnPanel
        },
    )));

    let page = client.get("/admin/jobs");
    page.assert_ok();
    page.html().assert_no_button("Empty");

    client
        .post("/admin/jobs/retries/7/retry", &[])
        .assert_status(403);

    // And the same panel, for somebody who may act, does draw it.
    staff_client()
        .get("/admin/jobs")
        .html()
        .assert_button("Empty");
}

// ---------------------------------------------------------------------------
// 4. Post, redirect, get
// ---------------------------------------------------------------------------

/// Before:
///
/// ```ignore
/// let response = block_on(admin.dispatch(
///     Principal::new(&staff), "POST", "/admin/jobs", "", &Submission::new(),
/// )).expect("dispatch");
/// assert_eq!(response.status(), 303);
/// ```
///
/// A 303 to nowhere would pass that. `assert_redirects` is Django's, and
/// `following` then shows that the place it points at is a page — which is
/// the whole reason post-redirect-get exists and was not asserted at all.
#[test]
fn a_post_redirects_rather_than_reposting_on_reload() {
    let client = staff_client();

    client
        .post("/admin/jobs", &[])
        .assert_redirects("/admin/jobs/retries");

    let landed = client.send(TestRequest::post("/admin/jobs").form(&[]).following());
    landed.assert_ok();
    assert_eq!(landed.redirect_chain(), ["/admin/jobs/retries"]);
    landed.html().assert_text("two waiting");
}

// ---------------------------------------------------------------------------
// 5. The statuses, which the old tests could only get at through `dispatch`
// ---------------------------------------------------------------------------

/// `a_sub_path_the_panel_does_not_serve_is_the_admins_404`, and the two it
/// could not express: a path no route claims, and a verb a path does not
/// answer. Those two come from the router, which `Admin::dispatch` never saw.
#[test]
fn the_statuses_are_the_ones_a_browser_would_get() {
    let client = staff_client();

    client.get("/admin/jobs/nope").assert_status(404);
    // Outside the admin's prefix entirely: the router's 404, not the
    // admin's, and until now nothing in this repository produced one.
    client.get("/nothing/here").assert_status(404);
    // A path the table has, with a verb it does not.
    client
        .send(TestRequest::delete("/admin/jobs"))
        .assert_status(405)
        .assert_header("allow", "GET, HEAD, POST");
}

/// The admin ships locked, and a client is not a way past that.
#[test]
fn the_default_policy_hides_a_panel_as_firmly_as_a_model() {
    // A config with no policy is `DenyAll`, and a staff user is refused
    // like anybody else. A contributed tab must not be a way in that the
    // application never opted into.
    client(base_config()).get("/admin/jobs").assert_status(403);
}

/// `a_tab_nobody_may_open_is_not_a_tab_anybody_is_shown`.
///
/// Before: `assert!(!html.contains("Background jobs"))`. After: there is no
/// link to the panel in the rail — which is what "is not a tab anybody is
/// shown" means, and which stays true if the words change.
#[test]
fn a_tab_nobody_may_open_is_not_a_tab_anybody_is_shown() {
    let client = client(base_config().with_policy(policy_fn(
        |who: &Principal<'_>, r: PolicyRequest<'_>| {
            is_staff(who, r) && r.action != Action::ViewPanel
        },
    )));

    client
        .get("/admin/post")
        .html()
        .within("nav", "aria-label", "Sections")
        .assert_no_link_to("/admin/jobs");
}
