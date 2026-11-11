//! Pages under one model, and under one record. Django's
//! `ModelAdmin.get_urls()`.

use std::sync::Arc;

use brocanth_admin::panel::PanelPage;
use brocanth_admin::policy::{policy_fn, Action, AllowAllUnsafeForDevelopment, Policy, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{
    Admin, AdminConfig, Discovery, Fixture, ModelAdmin, Principal, Response, Submission, Value,
};
use brocanth_test::Html;

/// The post model with a report under it and a preview under each post.
fn pages(m: ModelAdmin) -> ModelAdmin {
    m.page("report", "Report", |request, record| {
        assert!(record.is_none());
        Ok(match (request.path, request.post) {
            ("", false) => Some(PanelPage::body(
                "Post report",
                format!(
                    "<p>can act: {}</p><a href=\"{}\">csv</a>",
                    request.can_act,
                    request.url("csv")
                ),
            )),
            ("csv", false) => Some(PanelPage::body("CSV", "<pre>id,title</pre>")),
            ("", true) => Some(PanelPage::redirect(request.mount)),
            _ => None,
        })
    })
    .record_page("preview", "Preview", |request, record| {
        let title = record
            .and_then(|row| row.get("title"))
            .map(Value::as_input)
            .unwrap_or_default();
        Ok(match request.path {
            "" if request.post => Some(PanelPage::redirect(request.mount)),
            "" => Some(PanelPage::body(
                "Preview",
                format!(
                    "<article>{title}</article><p>can act: {}</p>",
                    request.can_act
                ),
            )),
            _ => None,
        })
    })
}

fn admin_with(policy: impl Policy) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .with_policy(policy)
            .model("post", pages),
    )
}

fn admin() -> Admin {
    admin_with(AllowAllUnsafeForDevelopment)
}

fn send(admin: &Admin, method: &str, path: &str) -> Response {
    block_on(admin.dispatch(Principal::anonymous(), method, path, "", &Submission::new())).unwrap()
}

fn html(response: &Response) -> Html {
    assert_eq!(response.status(), 200, "{response:?}");
    Html::new(response.html().expect("a page"))
}

#[test]
fn a_page_under_a_model_has_the_admin_layout_and_its_own_paths() {
    let admin = admin();
    let page = html(&send(&admin, "GET", "/admin/post/report"));
    page.assert_text("can act: true")
        .assert_link_to("/admin/post/report/csv");
    // The layout: the sidebar, with the model marked as the current screen.
    let current = page
        .links_with("aria-current", "page")
        .into_iter()
        .map(|a| a.href().unwrap_or_default().to_string())
        .collect::<Vec<_>>();
    assert_eq!(current, ["/admin/post"]);
    assert!(page.source().contains("<title>Post report"));

    // A path below the page is the page's; one it does not serve is a 404.
    html(&send(&admin, "GET", "/admin/post/report/csv")).assert_text("id,title");
    assert_eq!(send(&admin, "GET", "/admin/post/report/nope").status(), 404);
    // A POST the page answers with a redirect.
    assert_eq!(
        send(&admin, "POST", "/admin/post/report").location(),
        Some("/admin/post/report")
    );
}

#[test]
fn the_list_links_the_model_pages_and_the_detail_the_record_pages() {
    let admin = admin();
    html(&send(&admin, "GET", "/admin/post")).assert_link_to("/admin/post/report");
    html(&send(&admin, "GET", "/admin/post/1"))
        .assert_link_to("/admin/post/1/preview")
        .assert_no_link_to("/admin/post/report");
    // Another model has neither.
    html(&send(&admin, "GET", "/admin/user")).assert_no_link_to("/admin/post/report");
}

#[test]
fn a_page_under_a_record_gets_the_record() {
    let admin = admin();
    html(&send(&admin, "GET", "/admin/post/1/preview"))
        .assert_text("On the Analytical Engine")
        .assert_text("can act: true");
    assert_eq!(send(&admin, "GET", "/admin/post/404/preview").status(), 404);
    assert_eq!(
        send(&admin, "GET", "/admin/post/1/preview/nope").status(),
        404
    );
    // The admin's own screens around it still answer.
    html(&send(&admin, "GET", "/admin/post/1")).assert_text("On the Analytical Engine");
    html(&send(&admin, "GET", "/admin/post/1/edit"));
}

#[test]
fn a_model_page_asks_the_list_policy_and_a_post_asks_for_update() {
    // May list posts, may not change them.
    let reader = admin_with(policy_fn(|_: &Principal<'_>, request: Request<'_>| {
        request.action != Action::Update
    }));
    html(&send(&reader, "GET", "/admin/post/report")).assert_text("can act: false");
    assert_eq!(send(&reader, "POST", "/admin/post/report").status(), 403);
    html(&send(&reader, "GET", "/admin/post/1/preview")).assert_text("can act: false");
    assert_eq!(send(&reader, "POST", "/admin/post/1/preview").status(), 403);

    // May not list posts: no report.
    let outsider = admin_with(policy_fn(|_: &Principal<'_>, request: Request<'_>| {
        !(request.action == Action::ViewList && request.model == "post")
    }));
    assert_eq!(send(&outsider, "GET", "/admin/post/report").status(), 403);

    // May not view post 2: no preview of it, and post 1 still works.
    let partial = admin_with(policy_fn(|_: &Principal<'_>, request: Request<'_>| {
        !(request.action == Action::ViewDetail
            && request.record.and_then(|k| k.get("id")) == Some("2"))
    }));
    assert_eq!(send(&partial, "GET", "/admin/post/2/preview").status(), 403);
    html(&send(&partial, "GET", "/admin/post/1/preview"));
}

#[test]
fn nothing_answers_a_page_key_on_a_model_without_that_page() {
    let admin = admin();
    // `report` is the post model's; on `user` it is a record key.
    assert_eq!(send(&admin, "GET", "/admin/user/report").status(), 404);
}

#[test]
#[should_panic(expected = "`ModelAdmin::page` cannot use new or actions")]
fn a_page_called_new_stops_the_admin() {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .model("post", |m| m.page("new", "New", |_, _| Ok(None))),
    );
}

#[test]
#[should_panic(expected = "`ModelAdmin::record_page` cannot use edit or delete")]
fn a_record_page_called_edit_stops_the_admin() {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .model("post", |m| m.record_page("edit", "Edit", |_, _| Ok(None))),
    );
}
