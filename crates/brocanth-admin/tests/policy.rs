//! The admin ships locked.

use std::sync::Arc;

use brocanth_admin::policy::{
    policy_fn, Action, AllowAllUnsafeForDevelopment, DenyAll, Policy, Request,
};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Principal, Submission};

/// Whoever is asking. Public, because the row policies below name it.
pub struct Staff;

fn locked() -> Admin {
    // No `.unscoped_rows_unsafe_for_development().with_policy(..)` at all, which is the whole point.
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
            .unscoped_rows_unsafe_for_development()
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
            .unscoped_rows_unsafe_for_development()
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
            .unscoped_rows_unsafe_for_development()
            .with_policy(AllowAllUnsafeForDevelopment),
    );
    assert_eq!(
        block_on(admin.index(Principal::anonymous()))
            .unwrap()
            .status(),
        200
    );
}

// ---------------------------------------------------------------------------
// Row policies. The SQL half is in `brocanth-admin-model/tests/policy.rs`,
// against a real database. These are the parts the fixture can show.
// ---------------------------------------------------------------------------

mod rows {
    use brocanth_admin::row_policy::{Column, Entity, EntityPred, Ident, Policy, Pred};
    use brocanth_admin::AdminResource;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum Col {
        Id,
    }

    impl Column for Col {
        const ALL: &'static [Self] = &[Col::Id];
        fn name(&self) -> &'static str {
            "id"
        }
    }

    /// The `post` table.
    pub struct PostTable;

    impl Entity for PostTable {
        type Column = Col;
        const TABLE: Ident = Ident::must("post");
        const PRIMARY_KEY: Col = Col::Id;
    }

    pub struct Everything;

    impl Policy for Everything {
        type Entity = PostTable;
        type Subject = super::Staff;
        fn list(&self, _: &super::Staff) -> EntityPred<PostTable> {
            Pred::Always
        }
    }

    /// A resource over the `post` table that names the model `MODEL`.
    pub struct Named<const MODEL: u8>;

    impl<const MODEL: u8> AdminResource for Named<MODEL> {
        type Entity = PostTable;
        type Subject = super::Staff;
        type Policy = Everything;
        const MODEL: &'static str = match MODEL {
            0 => "post",
            1 => "user",
            _ => "comment",
        };
        fn policy(&self) -> &Everything {
            &Everything
        }
    }
}

#[test]
fn a_model_with_no_row_policy_is_refused_even_when_every_screen_is_allowed() {
    // The screen policy says yes to everything. No row policy is registered
    // and the unsafe switch is off, so every model screen is 403 and the
    // model is not offered.
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(AllowAllUnsafeForDevelopment),
    );
    let staff = Staff;
    let who = Principal::new(&staff);
    let empty = Submission::new();
    for response in [
        block_on(admin.list(who, "post", Default::default())),
        block_on(admin.detail(who, "post", "1")),
        block_on(admin.create_form(who, "post")),
        block_on(admin.edit_form(who, "post", "1")),
        block_on(admin.update(who, "post", "1", &empty)),
        block_on(admin.delete(who, "post", "1")),
    ] {
        assert_eq!(response.unwrap().status(), 403);
    }
    let index = block_on(admin.index(who)).unwrap();
    assert_eq!(index.status(), 200, "the index itself is a screen");
    assert!(
        !index.html().unwrap().contains("href=\"/admin/post\""),
        "a model nobody may list is not offered"
    );
}

#[test]
fn the_fixture_refuses_a_row_condition_rather_than_ignoring_it() {
    // An in-memory source cannot run SQL. To answer as if the condition were
    // TRUE would show every row to everybody, so it fails loudly instead.
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(AllowAllUnsafeForDevelopment)
            .resource(rows::Named::<0>),
    );
    let staff = Staff;
    let out = block_on(admin.list(Principal::new(&staff), "post", Default::default()));
    assert!(out.is_err(), "{out:?}");
}

#[test]
#[should_panic(expected = "no model with that key")]
fn a_row_policy_for_a_model_the_admin_does_not_hold_is_refused_at_startup() {
    let _ = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .resource(rows::Named::<2>),
    );
}

#[test]
#[should_panic(expected = "is written for table `post`, but the model's table is `user`")]
fn a_row_policy_for_another_table_is_refused_at_startup() {
    let _ = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .resource(rows::Named::<1>),
    );
}

#[test]
#[should_panic(expected = "two row policies for `post`")]
fn two_row_policies_for_one_model_are_refused_at_startup() {
    let _ = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .resource(rows::Named::<0>)
            .resource(rows::Named::<0>),
    );
}
