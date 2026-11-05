//! A tab the admin did not write.
//!
//! The panel here is the seam's first consumer after the shipped screens,
//! and it is deliberately trivial: what is under test is the admin's half
//! -- the route, the nav, the policy and the collision check -- not
//! anybody's rendering.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::{AdminError, Response, Route};
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::policy::{policy_fn, Action, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};

struct Staff;

/// Two paths and a button, which is every shape a panel has.
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

fn admin_with_panel(policy_allows: bool) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(Queue { key: "jobs" })
            .unscoped_rows_unsafe_for_development()
            .with_policy(policy_fn(move |_: &Principal<'_>, _| policy_allows)),
    )
}

fn get(admin: &Admin, path: &str, query: &str) -> Response {
    let staff = Staff;
    block_on(admin.dispatch(
        Principal::new(&staff),
        "GET",
        path,
        query,
        &Submission::new(),
    ))
    .expect("dispatch")
}

#[test]
fn a_panel_owns_every_path_under_its_key() {
    // The admin cannot know which sub-paths a panel serves, so the route is
    // one variant for the whole subtree and the panel is handed the rest.
    let keys = ["jobs"];
    assert!(matches!(
        Route::parse("/admin", "GET", "/admin/jobs", &keys),
        Some(Route::Panel {
            key: "jobs",
            rest: "",
            post: false
        })
    ));
    assert!(matches!(
        Route::parse("/admin", "GET", "/admin/jobs/retries", &keys),
        Some(Route::Panel {
            key: "jobs",
            rest: "retries",
            post: false
        })
    ));
    assert!(matches!(
        Route::parse("/admin", "POST", "/admin/jobs/dead/7/kill", &keys),
        Some(Route::Panel {
            key: "jobs",
            rest: "dead/7/kill",
            post: true
        })
    ));
    // And a key that is not a panel is still a model, unchanged.
    assert!(matches!(
        Route::parse("/admin", "GET", "/admin/post", &keys),
        Some(Route::List("post"))
    ));
}

#[test]
fn a_panel_renders_inside_the_admins_chrome() {
    let admin = admin_with_panel(true);
    let response = get(&admin, "/admin/jobs", "page=2");
    assert_eq!(response.status(), 200);
    let html = response.html().expect("a page");
    // Its own body...
    assert!(html.contains("page 2"), "{html}");
    // ...inside the layout, with the nav the models are in.
    assert!(html.contains("Background jobs"), "{html}");
    assert!(
        html.contains("/admin/post"),
        "the models are still in the nav: {html}"
    );
}

#[test]
fn a_panel_is_a_tab_and_knows_when_it_is_the_current_one() {
    let admin = admin_with_panel(true);
    // `aria-current` and not a class name. Which screen you are on is a
    // fact about the page; a class is a fact about the stylesheet, and a
    // test that asserts on one breaks every time somebody restyles.
    let page = get(&admin, "/admin/jobs", "");
    let html = page.html().expect("a page");
    assert!(
        current_tabs(html) == ["/admin/jobs"],
        "the panel's own tab, and only it, is current: {html}"
    );

    let page = get(&admin, "/admin/post", "");
    let html = page.html().expect("a page");
    assert!(
        current_tabs(html) == ["/admin/post"],
        "and from a model screen the model is: {html}"
    );
}

/// The `href` of every link in the rail carrying `aria-current`.
///
/// Scoped to the rail, because a pagination link in a list is also the
/// current page and says so the same way. Both are correct HTML; only one
/// of them answers "which section am I in".
fn current_tabs(html: &str) -> Vec<String> {
    let rail = html
        .split(r#"aria-label="Sections""#)
        .nth(1)
        .and_then(|rest| rest.split("</nav>").next())
        .unwrap_or("");
    rail.split("<a ")
        .skip(1)
        .filter_map(|tag| {
            let open = tag.split('>').next()?;
            if !open.contains(r#"aria-current="page""#) {
                return None;
            }
            let rest = open.split(r#"href=""#).nth(1)?;
            Some(rest.split('"').next()?.to_string())
        })
        .collect()
}

#[test]
fn a_sub_path_the_panel_does_not_serve_is_the_admins_404() {
    let admin = admin_with_panel(true);
    assert_eq!(get(&admin, "/admin/jobs/nope", "").status(), 404);
}

#[test]
fn the_default_policy_hides_a_panel_as_firmly_as_a_model() {
    // The whole reason the admin ships locked. A contributed tab must not
    // be a way in that the application never opted into.
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(Queue { key: "jobs" }),
    );
    assert_eq!(get(&admin, "/admin/jobs", "").status(), 403);
}

#[test]
fn reading_a_panel_and_pressing_its_buttons_are_two_questions() {
    // A read-only operator: may see the queue, may not empty it.
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(Queue { key: "jobs" })
            .unscoped_rows_unsafe_for_development()
            .with_policy(policy_fn(|_: &Principal<'_>, r: Request<'_>| {
                r.action != Action::ActOnPanel
            })),
    );
    let staff = Staff;
    let page = get(&admin, "/admin/jobs", "");
    assert_eq!(page.status(), 200);
    assert!(
        !page
            .html()
            .expect("a page")
            .contains("<button>Empty</button>"),
        "a reader who may not act is told so, and draws no button"
    );

    let posted = block_on(admin.dispatch(
        Principal::new(&staff),
        "POST",
        "/admin/jobs/retries/7/retry",
        "",
        &Submission::new(),
    ))
    .expect("dispatch");
    assert_eq!(
        posted.status(),
        403,
        "the button is refused before the panel runs"
    );
}

#[test]
fn a_tab_nobody_may_open_is_not_a_tab_anybody_is_shown() {
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(Queue { key: "jobs" })
            .unscoped_rows_unsafe_for_development()
            .with_policy(policy_fn(|_: &Principal<'_>, r: Request<'_>| {
                r.action != Action::ViewPanel
            })),
    );
    let page = get(&admin, "/admin/post", "");
    let html = page.html().expect("a page");
    assert!(!html.contains("Background jobs"), "{html}");
}

#[test]
fn a_post_redirects_rather_than_reposting_on_reload() {
    let admin = admin_with_panel(true);
    let staff = Staff;
    let response = block_on(admin.dispatch(
        Principal::new(&staff),
        "POST",
        "/admin/jobs",
        "",
        &Submission::new(),
    ))
    .expect("dispatch");
    assert_eq!(response.status(), 303);
}

#[test]
#[should_panic(expected = "a panel and a model both called `post`")]
fn a_panel_that_would_shadow_a_model_is_refused_by_name() {
    // Silent precedence is the alternative, and it is worse: the model
    // simply stops being reachable and nothing says why.
    let _ = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(Queue { key: "post" }),
    );
}

// ------------------------------------------------- an application's own page

fn admin_with_page(policy: impl brocanth_admin::Policy) -> Admin {
    use brocanth_admin::panel::Page;

    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            // Django's `get_urls()`: a closure, a tab, the chrome.
            .page("reports", "Reports", |r| {
                Ok(match r.path {
                    "" => Some(PanelPage::body(
                        "Reports",
                        format!(
                            "<p>Monthly report</p><a href=\"{}\">Export</a>",
                            r.url("export")
                        ),
                    )),
                    "export" => Some(PanelPage::body("Export", "<p>export ready</p>")),
                    _ => None,
                })
            })
            // A page with no tab: a link reaches it.
            .panel(
                Page::new("wizard", "Wizard", |_| {
                    Ok(Some(PanelPage::body("Wizard", "<p>step one</p>")))
                })
                .hidden(),
            )
            .unscoped_rows_unsafe_for_development()
            .with_policy(policy),
    )
}

fn html(response: Response) -> brocanth_test::Html {
    brocanth_test::Html::new(response.html().expect("a page"))
}

#[test]
fn an_application_page_has_the_chrome_a_tab_and_its_own_paths() {
    let admin = admin_with_page(policy_fn(|_: &Principal<'_>, _| true));

    let page = html(get(&admin, "/admin/reports", ""));
    assert!(page.text().contains("Monthly report"));
    let tab = page
        .link_to("/admin/reports")
        .expect("a tab in the sidebar");
    assert_eq!(tab.attr("aria-current"), Some("page"));
    assert!(page.has_link_to("/admin/post"), "the models are in the nav");
    assert!(page.has_link_to("/admin/reports/export"));

    let export = html(get(&admin, "/admin/reports/export", ""));
    assert!(export.text().contains("export ready"));
    // A path the page does not serve is the admin's own 404.
    assert_eq!(get(&admin, "/admin/reports/nope", "").status(), 404);
}

#[test]
fn a_hidden_page_has_no_tab_and_answers_all_the_same() {
    let admin = admin_with_page(policy_fn(|_: &Principal<'_>, _| true));
    let index = html(get(&admin, "/admin", ""));
    assert!(index.has_link_to("/admin/reports"));
    assert!(!index.has_link_to("/admin/wizard"), "no tab");

    let wizard = get(&admin, "/admin/wizard", "");
    assert_eq!(wizard.status(), 200);
    assert!(html(wizard).text().contains("step one"));
}

#[test]
fn an_application_page_is_behind_the_policy_like_a_panel() {
    // Only `ViewPanel` on `reports` is refused: the page answers 403 and
    // its tab is gone, while the models stay.
    let admin = admin_with_page(policy_fn(|_: &Principal<'_>, r: Request<'_>| {
        !(r.action == Action::ViewPanel && r.model == "reports")
    }));
    assert_eq!(get(&admin, "/admin/reports", "").status(), 403);
    let index = html(get(&admin, "/admin", ""));
    assert!(!index.has_link_to("/admin/reports"));
    assert!(index.has_link_to("/admin/post"));
}
