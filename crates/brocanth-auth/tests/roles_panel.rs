//! The roles tab and the staff policy, through the real admin.
//!
//! A superuser defines a role and gives it to an account. The role holder
//! then opens what the role grants and nothing else, and sees no button on
//! the roles tab: a role holder who could edit roles could give itself every
//! permission.

#![cfg(feature = "admin")]

use std::sync::Arc;

use brocanth_admin::policy::Principal;
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_auth::admin::{RolesPanel, StaffPolicy};
use brocanth_auth::identity::{AccountId, EmailAddress};
use brocanth_auth::memory::MemoryStore;
use brocanth_auth::storage::{AuthStore, NewAccount, Role, StaffStore};
use brocanth_auth::Staff;
use brocanth_test::Html;

fn admin(store: &Arc<MemoryStore>) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(RolesPanel::new(store.clone()))
            .with_policy(StaffPolicy),
    )
}

async fn account(store: &MemoryStore, email: &str) -> AccountId {
    store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse(email).expect("address")),
            email_verified: true,
            password_hash: None,
        })
        .await
        .expect("create")
        .id
}

async fn staff(store: &MemoryStore, id: &AccountId) -> Staff {
    Staff::load(store, id).await.expect("load").expect("exists")
}

fn status(admin: &Admin, who: Option<&Staff>, method: &str, path: &str, form: &Submission) -> u16 {
    let principal = who.map_or_else(Principal::anonymous, |s| Principal::new(s));
    block_on(admin.dispatch(principal, method, path, "", form))
        .expect("dispatch")
        .status()
}

fn page(admin: &Admin, who: &Staff, path: &str) -> Html {
    let response =
        block_on(admin.dispatch(Principal::new(who), "GET", path, "", &Submission::new()))
            .expect("dispatch");
    assert_eq!(response.status(), 200, "{path}");
    Html::new(response.html().expect("a page").to_string())
}

fn form(pairs: &[(&str, &str)]) -> Submission {
    let body = pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", v.replace('@', "%40").replace(' ', "+")))
        .collect::<Vec<_>>()
        .join("&");
    Submission::from_urlencoded(&body)
}

#[tokio::test]
async fn nobody_but_staff_opens_the_admin() {
    let store = Arc::new(MemoryStore::new());
    let plain = account(&store, "plain@example.com").await;
    let plain = staff(&store, &plain).await;
    let admin = admin(&store);
    assert_eq!(
        status(&admin, None, "GET", "/admin", &Submission::new()),
        403
    );
    assert_eq!(
        status(&admin, Some(&plain), "GET", "/admin", &Submission::new()),
        403,
        "an account with no flag and no role is not staff"
    );
}

#[tokio::test]
async fn a_superuser_gives_a_role_and_the_holder_gets_only_what_it_grants() {
    let store = Arc::new(MemoryStore::new());
    let ops = account(&store, "ops@example.com").await;
    store.set_superuser(&ops, true).await.unwrap();
    let ed = account(&store, "ed@example.com").await;
    let admin = admin(&store);
    let ops = staff(&store, &ops).await;

    // The superuser opens everything, the roles tab included, with buttons.
    let roles = page(&admin, &ops, "/admin/roles");
    roles.assert_text("No role has been defined.");
    roles.assert_button("Save the role");

    let define = form(&[
        ("do", "define"),
        ("role", "editor"),
        ("permissions", "roles.view post.change"),
    ]);
    assert_eq!(
        status(&admin, Some(&ops), "POST", "/admin/roles", &define),
        303
    );
    let assign = form(&[
        ("do", "assign"),
        ("role", "editor"),
        ("email", "ed@example.com"),
    ]);
    assert_eq!(
        status(&admin, Some(&ops), "POST", "/admin/roles", &assign),
        303
    );
    assert_eq!(store.roles_of(&ed).await.unwrap(), ["editor"]);

    let roles = page(&admin, &ops, "/admin/roles");
    roles.assert_text("editor");
    roles.assert_text("post.change roles.view");
    roles.assert_text("ed@example.com");
    roles.assert_button("Add");

    // The editor opens the admin and reads the roles tab, which its role
    // grants, with no button on it. An account whose role does not grant
    // `roles.view` does not see the tab.
    let ed = staff(&store, &ed).await;
    assert_eq!(
        status(&admin, Some(&ed), "GET", "/admin", &Submission::new()),
        200
    );
    let seen = page(&admin, &ed, "/admin/roles");
    seen.assert_text("editor");
    assert!(
        seen.button("Save the role").is_none(),
        "a role holder gets no button on the roles tab"
    );
    let other = account(&store, "other@example.com").await;
    store
        .put_role(&Role::new("support", ["ticket.view"]))
        .await
        .unwrap();
    store.assign_role(&other, "support").await.unwrap();
    let other = staff(&store, &other).await;
    assert_eq!(
        status(
            &admin,
            Some(&other),
            "GET",
            "/admin/roles",
            &Submission::new()
        ),
        403,
        "no role of this account grants roles.view"
    );
    let grab = form(&[
        ("do", "define"),
        ("role", "editor"),
        ("permissions", "everything"),
    ]);
    assert_eq!(
        status(&admin, Some(&ed), "POST", "/admin/roles", &grab),
        403,
        "a role holder changes no role"
    );
    assert_eq!(
        store.roles().await.unwrap()[0].permissions.len(),
        2,
        "the refused form changed nothing"
    );
}

#[tokio::test]
async fn a_name_that_is_not_a_word_comes_back_with_the_reason() {
    let store = Arc::new(MemoryStore::new());
    let ops = account(&store, "ops@example.com").await;
    store.set_superuser(&ops, true).await.unwrap();
    let ops = staff(&store, &ops).await;
    let admin = admin(&store);
    let response = block_on(admin.dispatch(
        Principal::new(&ops),
        "POST",
        "/admin/roles",
        "",
        &form(&[("do", "define"), ("role", "two words")]),
    ))
    .expect("dispatch");
    let location = response.location().expect("a redirect").to_string();
    assert!(location.contains("error="), "{location}");
    assert!(store.roles().await.unwrap().is_empty());
}
