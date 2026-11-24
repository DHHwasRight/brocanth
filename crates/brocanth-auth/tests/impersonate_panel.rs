//! The Impersonate tab, rendered through the real admin.

#![cfg(feature = "admin")]

use std::sync::Arc;

use brocanth_admin::policy::{policy_fn, Action, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_auth::admin::ImpersonatePanel;
use brocanth_auth::identity::EmailAddress;
use brocanth_auth::memory::MemoryStore;
use brocanth_auth::storage::{AuthStore, NewAccount};
use brocanth_test::Html;

struct Operator;

fn admin(store: Arc<MemoryStore>, act: bool) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(ImpersonatePanel::new(store).starting_at("/support/act-as"))
            .with_policy(policy_fn(move |_: &Principal<'_>, r: Request<'_>| {
                act || r.action != Action::ActOnPanel
            })),
    )
}

fn get(admin: &Admin, path: &str) -> Html {
    let who = Operator;
    let response =
        block_on(admin.dispatch(Principal::new(&who), "GET", path, "", &Submission::new()))
            .expect("dispatch");
    assert_eq!(response.status(), 200, "{path}");
    Html::new(response.html().expect("a page").to_string())
}

async fn with_ada() -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::new());
    store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse("ada@example.com").unwrap()),
            email_verified: true,
            password_hash: None,
        })
        .await
        .unwrap();
    store
}

#[tokio::test]
async fn each_account_has_a_form_that_posts_to_the_application_route() {
    let page = get(&admin(with_ada().await, true), "/admin/impersonate");
    page.assert_text("ada@example.com");
    page.assert_form_posts_to("/support/act-as");
    page.assert_field("reason");
    page.assert_field("id");
    page.assert_button("Act as");
}

#[tokio::test]
async fn a_reader_who_may_not_act_sees_no_form() {
    let page = get(&admin(with_ada().await, false), "/admin/impersonate");
    page.assert_text("ada@example.com");
    page.assert_no_button("Act as");
}

#[tokio::test]
async fn the_tab_takes_no_post_of_its_own() {
    let who = Operator;
    let response = block_on(admin(with_ada().await, true).dispatch(
        Principal::new(&who),
        "POST",
        "/admin/impersonate",
        "",
        &Submission::new(),
    ))
    .expect("dispatch");
    assert_eq!(response.status(), 404);
}
