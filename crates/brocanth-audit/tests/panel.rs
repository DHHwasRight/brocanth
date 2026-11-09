//! The history in a real admin: a write made on a screen lands in the
//! trail, the record's History link opens its history, and the tab lists
//! every change.
//!
//! Asserted on what a person sees: the link, the rows, the values.

use std::sync::Arc;

use brocanth_admin::admin::Response;
use brocanth_admin::policy::{AllowAllUnsafeForDevelopment, Principal};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_audit::{MemoryStore, Trail};
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
