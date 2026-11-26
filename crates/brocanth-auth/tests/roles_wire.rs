//! The roles tab's data contract (docs/backoffice.md §7.6): each call and
//! answer survives JSON, the tab is the same in this process and over the
//! wire, and the data side checks each call with the tab's own rule, read
//! from the claim: an owner gives `read` and `edit` on its own tab, and
//! everything else is a superuser's.
//!
//! The wire is `remote::Loopback`: it encodes each call as the backoffice
//! wire does, and checks it with the same `remote::authorize`. Its grants
//! come from the `Staff` principal, as the generated `claim` reads them.

#![cfg(feature = "admin")]

use std::sync::Arc;

use brocanth_admin::panel::remote::{Grants, Loopback, PanelWire};
use brocanth_admin::policy::Principal;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_auth::admin::{
    RemoteRoles, RoleView, RolesAnswer, RolesCall, RolesChange, RolesData, RolesOutcome,
    RolesPanel, RolesView, StaffPolicy,
};
use brocanth_auth::identity::{AccountId, EmailAddress};
use brocanth_auth::memory::MemoryStore;
use brocanth_auth::storage::{AuthStore, NewAccount, Role, StaffStore};
use brocanth_auth::Staff;

/// What a member of staff holds, as the claim carries it.
fn grants(principal: &Principal<'_>) -> Grants {
    match principal.downcast::<Staff>() {
        Some(staff) => Grants {
            superuser: staff.is_superuser(),
            permissions: staff.permissions().iter().cloned().collect(),
        },
        None => Grants::default(),
    }
}

/// The application's admin, whose tab serves itself, the loopback to it,
/// and the backoffice's admin, whose tab reads it through the loopback.
/// The backoffice's own store is empty: what its tab shows came over the
/// wire.
fn both(store: &Arc<MemoryStore>) -> (Admin, Arc<dyn PanelWire>, Admin) {
    let application = AdminConfig::new()
        .discovering(Discovery::new().under("blog::models"))
        .panel(RolesPanel::new(store.clone()));
    let wire = application
        .panel_serves()
        .into_iter()
        .fold(Loopback::new(grants), |wire, (tab, serve)| {
            wire.panel(tab, serve)
        })
        .shared();
    let local = Admin::new(
        Arc::new(Fixture::blog()),
        application.with_policy(StaffPolicy),
    );
    let remote = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(RolesPanel::new(Arc::new(MemoryStore::new())))
            .panels_through(Arc::clone(&wire))
            .with_policy(StaffPolicy),
    );
    (local, wire, remote)
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

/// An account holding a role with `permissions`, as staff.
async fn holder(store: &MemoryStore, email: &str, permissions: &[&str]) -> Staff {
    let id = account(store, email).await;
    let role = format!("r-{}", email.split('@').next().unwrap());
    store
        .put_role(&Role::new(role.clone(), permissions.iter().copied()))
        .await
        .unwrap();
    store.assign_role(&id, &role).await.unwrap();
    staff(store, &id).await
}

async fn superuser(store: &MemoryStore) -> Staff {
    let ops = account(store, "ops@example.com").await;
    store.set_superuser(&ops, true).await.unwrap();
    staff(store, &ops).await
}

async fn get(admin: &Admin, who: &Staff) -> (u16, String) {
    let page = admin
        .dispatch(
            Principal::new(who),
            "GET",
            "/admin/roles",
            "",
            &Submission::new(),
        )
        .await
        .expect("a page");
    (page.status(), page.html().unwrap_or_default().to_string())
}

async fn post(admin: &Admin, who: &Staff, body: &str) -> Option<String> {
    admin
        .dispatch(
            Principal::new(who),
            "POST",
            "/admin/roles",
            "",
            &Submission::from_urlencoded(body),
        )
        .await
        .expect("a page")
        .location()
        .map(str::to_string)
}

#[test]
fn each_call_and_answer_survives_json() {
    let calls = [
        RolesCall::Read,
        RolesCall::Change(RolesChange::Define {
            role: "editor".into(),
            permissions: vec!["post.edit".into()],
        }),
        RolesCall::Change(RolesChange::Assign {
            role: "editor".into(),
            email: "ed@example.com".into(),
        }),
        RolesCall::Change(RolesChange::Grant {
            role: "editor".into(),
            resource: "jobs".into(),
            level: "read".into(),
            give: true,
        }),
    ];
    for call in calls {
        let text = serde_json::to_string(&call).expect("it encodes");
        assert_eq!(serde_json::from_str::<RolesCall>(&text).unwrap(), call);
    }
    let answers = [
        RolesAnswer::View(RolesView {
            durable: true,
            roles: vec![RoleView {
                name: "editor".into(),
                permissions: vec!["post.edit".into()],
                members: vec![("1".into(), "ed@example.com".into())],
            }],
        }),
        RolesAnswer::Outcome(RolesOutcome::Refused("no-role".into())),
    ];
    for answer in answers {
        let text = serde_json::to_string(&answer).expect("it encodes");
        assert_eq!(serde_json::from_str::<RolesAnswer>(&text).unwrap(), answer);
    }
}

#[tokio::test]
async fn the_tab_is_the_same_in_this_process_and_over_the_wire() {
    let store = Arc::new(MemoryStore::new());
    let (local, _, remote) = both(&store);
    let ops = superuser(&store).await;
    let ed = account(&store, "ed@example.com").await;

    // A superuser defines a role, gives it, and grants on it, over the wire.
    for body in [
        "do=define&role=editor&permissions=post.edit",
        "do=assign&role=editor&email=ed%40example.com",
        "do=grant&role=editor&resource=jobs&level=read",
    ] {
        assert_eq!(
            post(&remote, &ops, body).await.as_deref(),
            Some("/admin/roles"),
            "{body}"
        );
    }
    assert_eq!(store.roles_of(&ed).await.unwrap(), ["editor"]);
    let ed = staff(&store, &ed).await;
    assert!(ed.allows("post.edit") && ed.allows("jobs.read"));

    // The same page either way, the application's roles on it.
    let (status, html) = get(&remote, &ops).await;
    assert_eq!(status, 200);
    assert!(html.contains("ed@example.com") && html.contains("jobs.read"));
    assert_eq!(get(&local, &ops).await, (status, html));

    // A refusal the tab explains crossed the wire as its code.
    assert_eq!(
        post(
            &remote,
            &ops,
            "do=assign&role=nobody&email=ed%40example.com"
        )
        .await
        .as_deref(),
        Some("/admin/roles?error=no-role")
    );
    assert_eq!(
        post(&local, &ops, "do=assign&role=nobody&email=ed%40example.com")
            .await
            .as_deref(),
        Some("/admin/roles?error=no-role")
    );
}

/// Calls made past the tab, straight on the wire: the data side checks each
/// one with the tab's rule, whatever the backoffice decided.
#[tokio::test]
async fn the_data_side_checks_each_call_with_the_tabs_rule() {
    let store = Arc::new(MemoryStore::new());
    let (_, wire, _) = both(&store);
    let roles = RemoteRoles::new(wire);
    let ops = superuser(&store).await;
    let owner = holder(&store, "ann@example.com", &["post.owner"]).await;
    let reader = holder(&store, "rob@example.com", &["roles.read"]).await;
    let other = holder(&store, "bob@example.com", &["user.read"]).await;
    let grant = |resource: &str, level: &str| RolesChange::Grant {
        role: "r-bob".into(),
        resource: resource.into(),
        level: level.into(),
        give: true,
    };

    // Reading: roles.read, an owner of any tab, a superuser. Nobody else.
    for who in [&ops, &owner, &reader] {
        assert!(roles.read(&Principal::new(who)).await.is_ok());
    }
    let refused = roles
        .read(&Principal::new(&other))
        .await
        .expect_err("no grant on roles");
    assert!(refused.contains("roles.read"), "{refused}");
    assert!(roles.read(&Principal::anonymous()).await.is_err());

    // The owner of `post` gives read and edit on `post`, and nothing else.
    let before = store.roles().await.unwrap();
    for change in [
        grant("post", "owner"),
        grant("user", "read"),
        RolesChange::Define {
            role: "x".into(),
            permissions: vec!["post.edit".into()],
        },
        RolesChange::Delete {
            role: "r-bob".into(),
        },
        RolesChange::Assign {
            role: "r-ann".into(),
            email: "bob@example.com".into(),
        },
    ] {
        let refused = roles
            .change(&Principal::new(&owner), change.clone())
            .await
            .expect_err("past the owner's tab");
        assert!(refused.contains("refused"), "{change:?}: {refused}");
    }
    for who in [&reader, &other] {
        assert!(roles
            .change(&Principal::new(who), grant("post", "read"))
            .await
            .is_err());
    }
    assert_eq!(store.roles().await.unwrap(), before, "nothing changed");
    assert_eq!(
        roles
            .change(&Principal::new(&owner), grant("post", "edit"))
            .await,
        Ok(RolesOutcome::Done)
    );
    assert_eq!(
        roles
            .change(&Principal::new(&ops), grant("user", "owner"))
            .await,
        Ok(RolesOutcome::Done)
    );
    let bob = staff(&store, &other.account().id).await;
    assert!(bob.allows("post.edit") && bob.owned() == ["user"]);
}
