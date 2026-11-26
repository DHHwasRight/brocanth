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

// -- over the backoffice wire (docs/backoffice.md §7.6) -----------------------

/// A member of staff, as the loopback reads their grants.
struct Who(brocanth_admin::panel::remote::Grants);

fn who(permissions: &[&str]) -> Who {
    Who(brocanth_admin::panel::remote::Grants {
        superuser: false,
        permissions: permissions.iter().map(|p| p.to_string()).collect(),
    })
}

/// The application's admin, whose tab serves itself, and the backoffice's,
/// whose tab reads the accounts through the loopback. The backoffice's own
/// store is empty.
fn both(store: Arc<MemoryStore>) -> (Admin, Admin) {
    use brocanth_admin::panel::remote::Loopback;
    let allow = || policy_fn(|_: &Principal<'_>, _: Request<'_>| true);
    let application =
        AdminConfig::new().panel(ImpersonatePanel::new(store).starting_at("/support/act-as"));
    let wire = application
        .panel_serves()
        .into_iter()
        .fold(
            Loopback::new(|p: &Principal<'_>| {
                p.downcast::<Who>()
                    .map(|who| who.0.clone())
                    .unwrap_or_default()
            }),
            |wire, (tab, serve)| wire.panel(tab, serve),
        )
        .shared();
    let local = Admin::new(Arc::new(Fixture::blog()), application.with_policy(allow()));
    let remote = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .panel(
                ImpersonatePanel::new(Arc::new(MemoryStore::new())).starting_at("/support/act-as"),
            )
            .panels_through(wire)
            .with_policy(allow()),
    );
    (local, remote)
}

async fn shown(admin: &Admin, who: &Who) -> Result<(u16, String), String> {
    admin
        .dispatch(
            Principal::new(who),
            "GET",
            "/admin/impersonate",
            "",
            &Submission::new(),
        )
        .await
        .map(|page| (page.status(), page.html().unwrap_or_default().to_string()))
        .map_err(|e| e.to_string())
}

#[test]
fn a_page_of_accounts_survives_json() {
    use brocanth_auth::admin::{Subjects, SubjectsCall};
    let call = SubjectsCall {
        limit: 50,
        offset: 100,
    };
    let text = serde_json::to_string(&call).unwrap();
    assert_eq!(serde_json::from_str::<SubjectsCall>(&text).unwrap(), call);
    let page = Subjects {
        durable: true,
        total: 1,
        accounts: vec![("1".into(), "ada@example.com".into())],
    };
    let text = serde_json::to_string(&page).unwrap();
    assert_eq!(serde_json::from_str::<Subjects>(&text).unwrap(), page);
}

#[tokio::test]
async fn the_tab_is_the_same_over_the_wire_and_needs_its_read_grant() {
    let (local, remote) = both(with_ada().await);
    let reader = who(&["impersonate.read"]);
    let there = shown(&remote, &reader).await;
    assert_eq!(shown(&local, &reader).await, there);
    assert!(there.is_ok_and(|(status, html)| status == 200 && html.contains("ada@example.com")));
    let refused = shown(&remote, &who(&["post.read"])).await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.contains("impersonate.read")),
        "{refused:?}"
    );
}
