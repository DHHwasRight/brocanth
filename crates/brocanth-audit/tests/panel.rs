//! The history in a real admin: a write made on a screen lands in the
//! trail, the record's History link opens its history, and the tab lists
//! every change.
//!
//! Asserted on what a person sees: the link, the rows, the values.

use std::sync::Arc;

use brocanth_admin::admin::Response;
use brocanth_admin::policy::{policy_fn, Action, AllowAllUnsafeForDevelopment, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_audit::{Entry, Field, Kind, MemoryStore, Query, Trail};
use brocanth_events::{ChangeSink, RowChange};
use brocanth_test::Html;

fn admin(trail: &Trail) -> Admin {
    let config = AdminConfig::new()
        .with_policy(AllowAllUnsafeForDevelopment)
        .at("/admin")
        .user_label(|_| Some("Ada".to_string()))
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development();
    Admin::new(
        Arc::new(Fixture::blog()),
        brocanth_audit::admin::install_with(config, trail.clone()),
    )
}

fn send(admin: &Admin, method: &str, path: &str, query: &str, form: &str) -> Response {
    let form = Submission::from_urlencoded(form);
    block_on(admin.dispatch(Principal::anonymous(), method, path, query, &form))
        .unwrap_or_else(|e| panic!("{method} {path}: {e}"))
}

fn html(response: &Response) -> Html {
    Html::new(response.html().expect("a page"))
}

fn trail() -> Trail {
    Trail::with_store(Arc::new(MemoryStore::new()), 0)
}

#[test]
fn an_edit_on_a_screen_is_in_the_record_history_with_its_values() {
    let trail = trail();
    let admin = admin(&trail);
    let edit = send(
        &admin,
        "POST",
        "/admin/post/2",
        "",
        "title=Compilers%2C+revised&body=A0+and+after.&author_id=2&views=12&published_at=",
    );
    assert_eq!(edit.status(), 303, "{:?}", edit.html());

    let detail = html(&send(&admin, "GET", "/admin/post/2", "", ""));
    detail.assert_link_to("/admin/history?table=post&row=2");

    let history = html(&send(
        &admin,
        "GET",
        "/admin/history",
        "table=post&row=2",
        "",
    ));
    history
        .assert_text("History of post 2")
        .assert_text("Ada")
        .assert_text("updated via admin")
        .assert_text("Compilers, revised");
}

#[test]
fn the_tab_lists_every_change_and_one_record_filters_it() {
    let trail = trail();
    let admin = admin(&trail);
    trail.by("bob").publish(&RowChange::created("post", 9_i64));
    trail.publish(&RowChange::deleted("user", 4_i64));

    let all = html(&send(&admin, "GET", "/admin/history", "", ""));
    all.assert_text("created via app")
        .assert_text("deleted via app")
        .assert_text("bob")
        .assert_text("nobody signed in");

    let one = html(&send(
        &admin,
        "GET",
        "/admin/history",
        "table=user&row=4",
        "",
    ));
    one.assert_text("deleted via app");
    assert!(!one.text().contains("bob"), "only user 4");
}

#[test]
fn a_record_with_no_change_says_so() {
    let trail = trail();
    let admin = admin(&trail);
    let page = html(&send(
        &admin,
        "GET",
        "/admin/history",
        "table=post&row=1",
        "",
    ));
    page.assert_text("No changes recorded.");
}

#[test]
fn a_bulk_delete_is_recorded_once_per_record() {
    let trail = trail();
    let config = AdminConfig::new()
        .with_policy(AllowAllUnsafeForDevelopment)
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .action(brocanth_admin::actions::BulkAction::delete_selected());
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        brocanth_audit::admin::install_with(config, trail.clone()),
    );
    let done = send(
        &admin,
        "POST",
        "/admin/post/actions",
        "",
        "action=delete_selected&confirm=yes&pk=1&pk=2",
    );
    assert_eq!(done.status(), 303);
    let entries = block_on(trail.find(&brocanth_audit::Query::recent())).unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries
        .iter()
        .all(|e| e.source == "admin" && e.actor.is_none()));
}

#[test]
fn the_tab_is_locked_like_every_tab() {
    let trail = trail();
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        brocanth_audit::admin::install_with(AdminConfig::new().at("/admin"), trail),
    );
    let page = send(&admin, "GET", "/admin/history", "", "");
    assert_eq!(page.status(), 403);
}

fn entries(trail: &Trail) -> Vec<Entry> {
    block_on(trail.find(&Query::recent())).unwrap()
}

#[test]
fn a_create_on_a_screen_is_recorded_with_its_actor_and_values() {
    let trail = trail();
    let admin = admin(&trail);
    let made = send(
        &admin,
        "POST",
        "/admin/post",
        "",
        "title=Fresh&body=New+text.&author_id=1&views=0&published_at=",
    );
    assert_eq!(made.status(), 303, "{:?}", made.html());
    let all = entries(&trail);
    assert_eq!(all.len(), 1);
    let e = &all[0];
    assert_eq!(
        (
            e.kind,
            e.actor.as_deref(),
            e.source.as_str(),
            e.table.as_str()
        ),
        (Kind::Created, Some("Ada"), "admin", "post")
    );
    assert!(e.fields.contains(&Field {
        name: "title".into(),
        before: None,
        after: Some("Fresh".into()),
    }));
    let page = html(&send(&admin, "GET", "/admin/history", "", ""));
    page.assert_text("created via admin")
        .assert_text("title: Fresh");
}

#[test]
fn a_delete_on_a_screen_is_recorded_with_the_values_it_had() {
    let trail = trail();
    let admin = admin(&trail);
    let gone = send(&admin, "POST", "/admin/post/1/delete", "", "");
    assert_eq!(gone.status(), 303);
    let all = entries(&trail);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].kind, Kind::Deleted);
    assert_eq!(all[0].actor.as_deref(), Some("Ada"));
    assert_eq!(all[0].row, "1");
    let page = html(&send(
        &admin,
        "GET",
        "/admin/history",
        "table=post&row=1",
        "",
    ));
    page.assert_text("deleted via admin")
        .assert_text("title: was On the Analytical Engine");
}

#[test]
fn a_refused_write_is_not_recorded() {
    let trail = trail();
    let policy = policy_fn(|_: &Principal<'_>, r: Request<'_>| {
        !matches!(r.action, Action::Update | Action::Delete)
    });
    let config = AdminConfig::new()
        .with_policy(policy)
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development();
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        brocanth_audit::admin::install_with(config, trail.clone()),
    );
    let edit = send(
        &admin,
        "POST",
        "/admin/post/2",
        "",
        "title=X&body=Y&author_id=2&views=1&published_at=",
    );
    assert_eq!(edit.status(), 403);
    assert_eq!(
        send(&admin, "POST", "/admin/post/1/delete", "", "").status(),
        403
    );
    assert!(entries(&trail).is_empty());
}

#[test]
fn the_tab_is_refused_to_a_person_who_may_see_everything_else() {
    let trail = trail();
    let policy = policy_fn(|_: &Principal<'_>, r: Request<'_>| {
        !(r.action == Action::ViewPanel && r.model == "history")
    });
    let config = AdminConfig::new()
        .with_policy(policy)
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development();
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        brocanth_audit::admin::install_with(config, trail.clone()),
    );
    trail.by("bob").publish(&RowChange::created("post", 9_i64));
    assert_eq!(send(&admin, "GET", "/admin/post", "", "").status(), 200);
    let refused = send(&admin, "GET", "/admin/history", "table=post&row=9", "");
    assert_eq!(refused.status(), 403);
    assert!(!refused.html().unwrap_or("").contains("bob"));
    // The index lists no History tab for this person.
    let index = html(&send(&admin, "GET", "/admin", "", ""));
    index.assert_no_link_to("/admin/history");
}

#[test]
fn the_tab_is_read_only() {
    let trail = trail();
    let admin = admin(&trail);
    trail.publish(&RowChange::created("post", 1_i64));
    assert_eq!(send(&admin, "POST", "/admin/history", "", "").status(), 404);
    assert_eq!(
        send(&admin, "GET", "/admin/history/1", "", "").status(),
        404
    );
    assert_eq!(entries(&trail).len(), 1);
}

#[test]
fn what_people_typed_is_escaped_on_the_tab() {
    let trail = trail();
    let admin = admin(&trail);
    trail.record(Entry {
        id: 0,
        at: 0,
        actor: Some("<script>alert(1)</script>".into()),
        source: "admin".into(),
        table: "post".into(),
        row: "\"><img src=x>".into(),
        label: "<b>bold</b>".into(),
        kind: Kind::Updated,
        fields: vec![Field {
            name: "title".into(),
            before: Some("<i>old</i>".into()),
            after: Some("</td><script>x()</script>".into()),
        }],
    });
    let page = send(&admin, "GET", "/admin/history", "", "");
    let raw = page.html().unwrap();
    // The admin's own chrome carries a `<script>`, so look for the payloads.
    for bad in [
        "<script>alert(1)",
        "<script>x()",
        "<img src=x>",
        "<b>bold</b>",
        "<i>old</i>",
    ] {
        assert!(!raw.contains(bad), "{bad} reached the page unescaped");
    }
    html(&page).assert_text("<script>alert(1)</script>");
    // The row link encodes the key rather than breaking out of the href.
    assert!(raw.contains("row=%22%3E%3Cimg%20src%3Dx%3E"), "{raw}");
}

#[test]
fn a_full_page_links_to_the_older_changes() {
    let trail = trail();
    let admin = admin(&trail);
    for n in 0..55_i64 {
        trail.publish(&RowChange::created("post", n));
    }
    let first = html(&send(&admin, "GET", "/admin/history", "table=post", ""));
    first
        .assert_text("History of post")
        .assert_text("Older changes");
    let older = first
        .links()
        .into_iter()
        .find(|a| a.text().contains("Older changes"))
        .expect("the link");
    let href = older.attr("href").unwrap().to_string();
    let (path, query) = href.split_once('?').unwrap();
    assert!(query.contains("table=post"), "{href}");
    let second = html(&send(&admin, "GET", path, query, ""));
    // 55 entries, 50 a page: the five oldest are rows 0 to 4.
    let rows = second.text();
    assert!(rows.contains("post 4") && rows.contains("post 0"), "{rows}");
    assert!(!rows.contains("post 5 "), "{rows}");
    assert!(!rows.contains("Older changes"));
}

#[test]
fn a_table_filter_shows_only_that_table_and_a_link_to_everything() {
    let trail = trail();
    let admin = admin(&trail);
    trail.by("bob").publish(&RowChange::created("post", 1_i64));
    trail
        .by("carol")
        .publish(&RowChange::created("user", 1_i64));
    let page = html(&send(&admin, "GET", "/admin/history", "table=user", ""));
    page.assert_text("History of user")
        .assert_text("carol")
        .assert_link_to("/admin/history");
    assert!(!page.text().contains("bob"));
}

#[test]
fn a_bulk_delete_records_the_actor_when_the_admin_knows_one() {
    let trail = trail();
    let config = AdminConfig::new()
        .with_policy(AllowAllUnsafeForDevelopment)
        .at("/admin")
        .user_label(|_| Some("Ada".to_string()))
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .action(brocanth_admin::actions::BulkAction::delete_selected());
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        brocanth_audit::admin::install_with(config, trail.clone()),
    );
    // No confirmation: nothing is written, so nothing is recorded.
    send(
        &admin,
        "POST",
        "/admin/post/actions",
        "",
        "action=delete_selected&pk=1",
    );
    assert!(entries(&trail).is_empty());
    send(
        &admin,
        "POST",
        "/admin/post/actions",
        "",
        "action=delete_selected&confirm=yes&pk=1",
    );
    let all = entries(&trail);
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].actor.as_deref(), Some("Ada"));
    assert_eq!(all[0].kind, Kind::Deleted);
}

#[test]
fn without_the_plugin_the_installed_tab_says_so_and_a_write_still_works() {
    // No `AuditPlugin` is built in this test binary, so `installed()` has
    // no trail.
    let config = AdminConfig::new()
        .with_policy(AllowAllUnsafeForDevelopment)
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development();
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        brocanth_audit::admin::install(config),
    );
    html(&send(&admin, "GET", "/admin/history", "", ""))
        .assert_text("The audit plugin is not registered");
    let gone = send(&admin, "POST", "/admin/post/1/delete", "", "");
    assert_eq!(gone.status(), 303);
}
