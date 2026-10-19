//! The admin ships locked.

use std::sync::Arc;

use brocanth_admin::policy::{
    policy_fn, Action, AllowAllUnsafeForDevelopment, DenyAll, Policy, Request,
};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Principal, Submission};

struct Staff;

fn locked() -> Admin {
    // No `.with_policy(..)` at all, which is the whole point.
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new().discovering(Discovery::new().under("blog::models")),
    )
}

#[test]
fn the_default_policy_denies() {
    let policy = DenyAll;
    let staff = Staff;

    for action in [
        Action::ViewIndex,
        Action::ViewList,
        Action::ViewDetail,
        Action::Create,
        Action::Update,
        Action::Delete,
        Action::ViewPanel,
        Action::ActOnPanel,
    ] {
        assert!(
            !policy.allows(&Principal::new(&staff), Request::new(action, "user")),
            "{action:?} must be denied even with an identity attached"
        );
    }
}

#[test]
fn an_admin_without_a_policy_refuses_its_own_index() {
    let admin = locked();
    let response = block_on(admin.index(Principal::anonymous())).unwrap();

    assert_eq!(response.status(), 403);
}

#[test]
fn an_admin_without_a_policy_refuses_every_screen() {
    let admin = locked();
    let empty = Submission::new();

    let screens = [
        block_on(admin.list(Principal::anonymous(), "user", Default::default())),
        block_on(admin.detail(Principal::anonymous(), "user", "1")),
        block_on(admin.create_form(Principal::anonymous(), "user")),
        block_on(admin.create(Principal::anonymous(), "user", &empty)),
        block_on(admin.edit_form(Principal::anonymous(), "user", "1")),
        block_on(admin.update(Principal::anonymous(), "user", "1", &empty)),
        block_on(admin.delete_confirm(Principal::anonymous(), "user", "1")),
        block_on(admin.delete(Principal::anonymous(), "user", "1")),
    ];

    for screen in screens {
        assert_eq!(screen.unwrap().status(), 403);
    }
}

#[test]
fn a_refusal_does_not_leak_the_model_list() {
    let admin = locked();
    let html = block_on(admin.index(Principal::anonymous()))
        .unwrap()
        .html()
        .unwrap()
        .to_string();

    assert!(
        !html.contains("Posts"),
        "the sidebar is built for the asker"
    );
    assert!(!html.contains("Users"));
}

#[test]
fn a_policy_decides_per_action_and_per_model() {
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(policy_fn(|principal: &Principal<'_>, request| {
                if principal.downcast::<Staff>().is_none() {
                    return false;
                }
                match request.action {
                    Action::Delete => false,
                    Action::Update => request.model == "post",
                    _ => true,
                }
            })),
    );
    let staff = Staff;
    let who = Principal::new(&staff);

    assert_eq!(block_on(admin.index(who)).unwrap().status(), 200);
    assert_eq!(
        block_on(admin.delete(who, "post", "1")).unwrap().status(),
        403
    );
    assert_eq!(
        block_on(admin.edit_form(who, "post", "1"))
            .unwrap()
            .status(),
        200
    );
    assert_eq!(
        block_on(admin.edit_form(who, "user", "1"))
            .unwrap()
            .status(),
        403
    );
    // And an anonymous caller is still refused, because the policy says so.
    assert_eq!(
        block_on(admin.index(Principal::anonymous()))
            .unwrap()
            .status(),
        403
    );
}

#[test]
fn a_denied_action_hides_its_own_links() {
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(policy_fn(|_: &Principal<'_>, request: Request<'_>| {
                request.action != Action::Delete
            })),
    );
    let html = block_on(admin.list(Principal::anonymous(), "post", Default::default()))
        .unwrap()
        .html()
        .unwrap()
        .to_string();

    assert!(html.contains("Edit"));
    assert!(
        !html.contains(">Delete<"),
        "no link to a screen that would refuse"
    );
}

#[test]
fn the_development_policy_has_to_be_named_in_full() {
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(AllowAllUnsafeForDevelopment),
    );
    assert_eq!(
        block_on(admin.index(Principal::anonymous()))
            .unwrap()
            .status(),
        200
    );
}
