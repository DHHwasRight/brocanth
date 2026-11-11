//! Pages under one model, and under one record. Django's
//! `ModelAdmin.get_urls()`.

use std::sync::{Arc, Mutex};

use brocanth_admin::data::{AdminData, Scope};
use brocanth_admin::journal::AdminWrite;
use brocanth_admin::panel::PanelPage;
use brocanth_admin::policy::{policy_fn, Action, AllowAllUnsafeForDevelopment, Policy, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{
    Admin, AdminConfig, Discovery, Fixture, ModelAdmin, PrimaryKey, Principal, Response,
    Submission, Value,
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

// ---------------------------------------------------------------------------
// Async pages, and the writes they make
// ---------------------------------------------------------------------------

/// A report that awaits a read, and a "publish" page under each post that
/// writes through the admin. The journal keeps what it hears.
fn async_admin(data: Arc<Fixture>, policy: impl Policy, heard: Arc<Mutex<Vec<String>>>) -> Admin {
    let reads = data.clone();
    Admin::new(
        data,
        AdminConfig::new()
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .with_policy(policy)
            .journal(move |write: &AdminWrite<'_>| {
                heard.lock().unwrap().push(format!(
                    "{} {} {}",
                    write.kind.as_str(),
                    write.model,
                    write.key
                ));
            })
            .model("post", move |m| {
                m.page_async("count", "Count", move |cx| {
                    let reads = reads.clone();
                    Box::pin(async move {
                        // Not ready on the first poll, as a database read.
                        YieldOnce(false).await;
                        let n = reads.len(cx.model());
                        Ok(Some(PanelPage::body("Count", format!("<p>{n} posts</p>"))))
                    })
                })
                .record_page_async("publish", "Publish", |cx| {
                    Box::pin(async move {
                        let key = cx.key().expect("a record page has a key").clone();
                        // `other=2` asks the page to write another post.
                        let target = match cx.request().param("other") {
                            Some(id) => PrimaryKey::single("id", id.parse::<i64>().unwrap()),
                            None => key,
                        };
                        let values = [("published".to_string(), Value::Bool(true))];
                        let done = cx.update(&target, &values).await?;
                        Ok(Some(PanelPage::body(
                            "Publish",
                            format!("<p>done: {done}</p>"),
                        )))
                    })
                })
            }),
    )
}

/// A future that is pending once, then ready.
struct YieldOnce(bool);

impl std::future::Future for YieldOnce {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.0 {
            return std::task::Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}

fn published(data: &Fixture, admin: &Admin, id: i64) -> Value {
    let model = admin.descriptor("post").unwrap();
    block_on(data.fetch(model, &PrimaryKey::single("id", id), Scope::Unscoped))
        .unwrap()
        .unwrap()
        .get("published")
        .cloned()
        .unwrap()
}

fn send_query(admin: &Admin, method: &str, path: &str, query: &str) -> Response {
    block_on(admin.dispatch(
        Principal::anonymous(),
        method,
        path,
        query,
        &Submission::new(),
    ))
    .unwrap()
}

#[test]
fn an_async_page_awaits_before_it_renders() {
    let data = Arc::new(Fixture::blog());
    let admin = async_admin(data, AllowAllUnsafeForDevelopment, Arc::default());
    html(&send(&admin, "GET", "/admin/post/count")).assert_text("2 posts");
}

#[test]
fn a_page_write_goes_through_the_admin_and_the_journal_hears_it() {
    let data = Arc::new(Fixture::blog());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let admin = async_admin(data.clone(), AllowAllUnsafeForDevelopment, heard.clone());
    assert_eq!(published(&data, &admin, 2), Value::Bool(false));

    // A GET writes nothing, even when the page asks.
    html(&send(&admin, "GET", "/admin/post/2/publish")).assert_text("done: false");
    assert_eq!(published(&data, &admin, 2), Value::Bool(false));
    assert!(heard.lock().unwrap().is_empty());

    // A POST writes, and the journal hears it.
    html(&send(&admin, "POST", "/admin/post/2/publish")).assert_text("done: true");
    assert_eq!(published(&data, &admin, 2), Value::Bool(true));
    assert_eq!(*heard.lock().unwrap(), ["updated post 2"]);
}

#[test]
fn a_page_write_asks_the_policy_for_the_record_it_writes() {
    // May change post 1, not post 2.
    let data = Arc::new(Fixture::blog());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let admin = async_admin(
        data.clone(),
        policy_fn(|_: &Principal<'_>, request: Request<'_>| {
            !(request.action == Action::Update
                && request.record.and_then(|k| k.get("id")) == Some("2"))
        }),
        heard.clone(),
    );
    // The page under post 2 is refused before it runs.
    assert_eq!(send(&admin, "POST", "/admin/post/2/publish").status(), 403);
    // The page under post 1 runs, but its write of post 2 is refused.
    html(&send_query(
        &admin,
        "POST",
        "/admin/post/1/publish",
        "other=2",
    ))
    .assert_text("done: false");
    assert_eq!(published(&data, &admin, 2), Value::Bool(false));
    assert!(heard.lock().unwrap().is_empty());
}

#[test]
fn a_page_post_without_the_csrf_token_is_refused_before_the_page() {
    use brocanth::plugin_api::Plugin;
    use brocanth::plugin_app::AppBuilder;
    use brocanth_admin::{AdminPlugin, AdminSetup};
    use brocanth_middleware::{Csrf, CsrfConfig};
    use brocanth_test::{TestClient, TestRequest};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    let data = Arc::new(Fixture::blog());
    let config = AdminConfig::new()
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .with_policy(AllowAllUnsafeForDevelopment)
        .model("post", |m| {
            m.record_page_async("publish", "Publish", |cx| {
                Box::pin(async move {
                    let key = cx.key().unwrap().clone();
                    let values = [("published".to_string(), Value::Bool(true))];
                    cx.update(&key, &values).await?;
                    Ok(Some(PanelPage::redirect(cx.request().mount)))
                })
            })
        });
    let mut app = AppBuilder::new()
        .plugin_hooking(Csrf::new(CsrfConfig::default()))
        .plugin(AdminPlugin::new(AdminSetup::new(data.clone(), config)))
        .build()
        .expect("an application");
    app.mount_routes::<AdminPlugin>("/admin")
        .expect("the routes");
    let client = TestClient::new(app);
    let admin = Admin::new(
        data.clone(),
        AdminConfig::new().discovering(Discovery::new().under("blog::models")),
    );
    let cookie = format!("csrf_token={TOKEN}");

    let refused = client.send(
        TestRequest::post("/admin/post/2/publish")
            .header("cookie", &cookie)
            .form(&[("x", "y")]),
    );
    assert_eq!(refused.status(), 403);
    assert_eq!(published(&data, &admin, 2), Value::Bool(false));

    let done = client.send(
        TestRequest::post("/admin/post/2/publish")
            .header("cookie", &cookie)
            .form(&[("_csrf_token", TOKEN)]),
    );
    assert_eq!(done.status(), 303);
    assert_eq!(published(&data, &admin, 2), Value::Bool(true));
}
