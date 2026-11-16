//! The roles tab and the staff policy, through the real admin.
//!
//! A superuser defines a role and gives it to an account. The role holder
//! then opens what the role grants and nothing else, and sees no button on
//! the roles tab: a role holder who could edit roles could give itself every
//! permission. The one exception: the owner of a tab gives `read` and `edit`
//! on that tab, and nothing else (`docs/module-capabilities.md` §5.2).

#![cfg(feature = "admin")]

use std::sync::Arc;

use brocanth_admin::late::Late;
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
        ("permissions", "roles.read post.edit"),
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
    roles.assert_text("post.edit");
    roles.assert_text("roles.read");
    roles.assert_text("ed@example.com");
    roles.assert_button("Add");

    // The editor opens the admin and reads the roles tab, which its role
    // grants, with no button on it. An account whose role does not grant
    // `roles.read` does not see the tab.
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
        .put_role(&Role::new("support", ["ticket.read"]))
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
        "no role of this account grants roles.read"
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

/// The admin with every model's rows open, so the screen policy is the only
/// gate: what `StaffPolicy` decides is what the test sees.
fn open_rows(store: &Arc<MemoryStore>) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .panel(RolesPanel::new(store.clone()))
            .with_policy(StaffPolicy),
    )
}

/// An account holding a role, as staff.
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

/// Every model screen, as (method, path, form, the permission it needs).
fn post_screens() -> Vec<(&'static str, &'static str, Submission, &'static str)> {
    let edit = || form(&[("title", "Taken over"), ("body", "x"), ("author_id", "1")]);
    vec![
        ("GET", "/admin/post", Submission::new(), "post.read"),
        ("GET", "/admin/post/1", Submission::new(), "post.read"),
        ("GET", "/admin/post/new", Submission::new(), "post.edit"),
        ("POST", "/admin/post", edit(), "post.edit"),
        ("GET", "/admin/post/1/edit", Submission::new(), "post.edit"),
        ("POST", "/admin/post/1", edit(), "post.edit"),
        (
            "GET",
            "/admin/post/1/delete",
            Submission::new(),
            "post.edit",
        ),
        (
            "POST",
            "/admin/post/1/delete",
            Submission::new(),
            "post.edit",
        ),
    ]
}

/// A member of staff whose role grants nothing on `post` is refused on
/// every `post` screen, and the refused writes change nothing.
#[tokio::test]
async fn staff_without_a_permission_is_refused_on_every_screen() {
    let store = Arc::new(MemoryStore::new());
    let admin = open_rows(&store);
    let ops = superuser(&store).await;
    let reader = holder(&store, "reader@example.com", &["user.read"]).await;
    assert_eq!(
        status(&admin, Some(&reader), "GET", "/admin", &Submission::new()),
        200,
        "the index opens to every member of staff"
    );
    assert_eq!(
        status(
            &admin,
            Some(&reader),
            "GET",
            "/admin/user",
            &Submission::new()
        ),
        200
    );
    for (method, path, submission, needs) in post_screens() {
        assert_eq!(
            status(&admin, Some(&reader), method, path, &submission),
            403,
            "{method} {path} needs {needs}"
        );
    }
    // Neither the refused update nor the refused delete touched the row.
    let post = page(&admin, &ops, "/admin/post/1");
    post.assert_text("On the Analytical Engine");
    assert_eq!(
        status(
            &admin,
            Some(&reader),
            "GET",
            "/admin/roles",
            &Submission::new()
        ),
        403,
        "a panel needs <panel>.read"
    );
}

/// Each level opens its own screens and the ones below it, and no other.
#[tokio::test]
async fn each_level_opens_its_own_screens_only() {
    let store = Arc::new(MemoryStore::new());
    let admin = open_rows(&store);
    for granted in ["post.read", "post.edit", "post.owner"] {
        let email = format!("{}@example.com", granted.replace('.', "-"));
        let who = holder(&store, Box::leak(email.into_boxed_str()), &[granted]).await;
        for (method, path, submission, needs) in post_screens() {
            // A level includes the ones below it: `edit` reads, `owner`
            // edits. A write that is allowed would change the fixture; only
            // the pages are checked for "allowed".
            let expected_open = needs == "post.read" || granted != "post.read";
            if expected_open && method == "POST" {
                continue;
            }
            let got = status(&admin, Some(&who), method, path, &submission);
            if expected_open {
                assert_eq!(got, 200, "{granted}: {method} {path}");
            } else {
                assert_eq!(got, 403, "{granted}: {method} {path} needs {needs}");
            }
        }
    }
}

/// A role whose permission looks like a wildcard grants that string and
/// nothing else, and a role named `superuser` is not the flag.
#[tokio::test]
async fn no_role_grants_what_only_the_flag_grants() {
    let store = Arc::new(MemoryStore::new());
    let admin = admin(&store);
    let star = holder(
        &store,
        "star@example.com",
        &["*", "post.*", "*.owner", "superuser"],
    )
    .await;
    assert!(!star.is_superuser());
    assert_eq!(
        status(
            &admin,
            Some(&star),
            "GET",
            "/admin/post",
            &Submission::new()
        ),
        403
    );
    let id = account(&store, "named@example.com").await;
    store
        .put_role(&Role::new("superuser", ["roles.read"]))
        .await
        .unwrap();
    store.assign_role(&id, "superuser").await.unwrap();
    let named = staff(&store, &id).await;
    assert!(!named.is_superuser());
    let define = form(&[
        ("do", "define"),
        ("role", "x"),
        ("permissions", "post.read"),
    ]);
    assert_eq!(
        status(&admin, Some(&named), "POST", "/admin/roles", &define),
        403
    );
}

/// A role holder that holds `roles.read` and even `roles.edit` is refused on
/// every button of the roles tab, and the store does not change. Only the
/// superuser flag opens them.
#[tokio::test]
async fn a_role_holder_presses_no_roles_button() {
    let store = Arc::new(MemoryStore::new());
    let admin = admin(&store);
    let ops = superuser(&store).await;
    let climber = holder(&store, "climber@example.com", &["roles.read", "roles.edit"]).await;
    let victim = account(&store, "victim@example.com").await;
    store.assign_role(&victim, "r-climber").await.unwrap();
    let before = store.roles().await.unwrap();

    let climber_id = climber.account().id.to_string();
    let victim_id = victim.to_string();
    let attempts = [
        form(&[
            ("do", "define"),
            ("role", "r-climber"),
            ("permissions", "post.read post.edit"),
        ]),
        form(&[
            ("do", "define"),
            ("role", "god"),
            ("permissions", "post.edit"),
        ]),
        form(&[
            ("do", "assign"),
            ("role", "r-climber"),
            ("email", "ops@example.com"),
        ]),
        form(&[
            ("do", "unassign"),
            ("role", "r-climber"),
            ("account", &victim_id),
        ]),
        form(&[("do", "delete"), ("role", "r-climber")]),
        form(&[
            ("do", "unassign"),
            ("role", "r-climber"),
            ("account", &climber_id),
        ]),
    ];
    for attempt in &attempts {
        assert_eq!(
            status(&admin, Some(&climber), "POST", "/admin/roles", attempt),
            403,
            "{attempt:?}"
        );
    }
    assert_eq!(store.roles().await.unwrap(), before, "no role changed");
    assert_eq!(store.roles_of(&victim).await.unwrap(), ["r-climber"]);
    assert!(store.roles_of(&ops.account().id).await.unwrap().is_empty());
    assert!(!store.is_superuser(&climber.account().id).await.unwrap());
}

/// The superuser's other buttons: take a role away, delete a role (its
/// members lose it), and the wrong inputs come back with a reason.
#[tokio::test]
async fn a_superuser_unassigns_and_deletes_and_is_told_what_is_wrong() {
    let store = Arc::new(MemoryStore::new());
    let admin = admin(&store);
    let ops = superuser(&store).await;
    let ed = holder(&store, "ed@example.com", &["post.read"]).await;
    let ed_id = ed.account().id.clone();

    for (submission, reason) in [
        (
            form(&[
                ("do", "assign"),
                ("role", "r-ed"),
                ("email", "not an address"),
            ]),
            "that is not an email address",
        ),
        (
            form(&[
                ("do", "assign"),
                ("role", "r-ed"),
                ("email", "nobody@example.com"),
            ]),
            "no account has that address",
        ),
        (
            form(&[
                ("do", "assign"),
                ("role", "ghost"),
                ("email", "ops@example.com"),
            ]),
            "no role has that name",
        ),
        (form(&[("do", "promote")]), "the form named no action"),
        (form(&[]), "the form named no action"),
        (
            form(&[("do", "define"), ("role", "")]),
            "a role name is letters",
        ),
    ] {
        let response = block_on(admin.dispatch(
            Principal::new(&ops),
            "POST",
            "/admin/roles",
            "",
            &submission,
        ))
        .expect("dispatch");
        let location = response.location().expect("a redirect").to_string();
        assert!(location.starts_with("/admin/roles?error="), "{location}");
        let (_, query) = location.split_once('?').unwrap();
        let shown = page_with_query(&admin, &ops, "/admin/roles", query);
        shown.assert_text(reason);
    }
    assert!(store.roles_of(&ops.account().id).await.unwrap().is_empty());

    let unassign = form(&[
        ("do", "unassign"),
        ("role", "r-ed"),
        ("account", ed_id.as_str()),
    ]);
    assert_eq!(
        status(&admin, Some(&ops), "POST", "/admin/roles", &unassign),
        303
    );
    assert!(store.roles_of(&ed_id).await.unwrap().is_empty());

    store.assign_role(&ed_id, "r-ed").await.unwrap();
    let delete = form(&[("do", "delete"), ("role", "r-ed")]);
    assert_eq!(
        status(&admin, Some(&ops), "POST", "/admin/roles", &delete),
        303
    );
    assert!(store.roles().await.unwrap().is_empty());
    assert!(store.roles_of(&ed_id).await.unwrap().is_empty());
    let ed = staff(&store, &ed_id).await;
    assert!(!ed.is_staff());
    assert_eq!(
        status(&admin, Some(&ed), "GET", "/admin", &Submission::new()),
        403,
        "with its only role deleted, ed is not staff"
    );
}

fn page_with_query(admin: &Admin, who: &Staff, path: &str, query: &str) -> Html {
    let response =
        block_on(admin.dispatch(Principal::new(who), "GET", path, query, &Submission::new()))
            .expect("dispatch");
    assert_eq!(response.status(), 200, "{path}?{query}");
    Html::new(response.html().expect("a page").to_string())
}

/// The redirect carries a code, and the tab shows its own sentence for it.
/// Text that a link puts in `error=` is not shown at all: a member of staff
/// could otherwise be sent a page that says what an attacker wrote, in the
/// admin's own voice.
#[tokio::test]
async fn the_roles_tab_shows_only_its_own_messages() {
    let store = Arc::new(MemoryStore::new());
    let admin = admin(&store);
    let ops = superuser(&store).await;

    let response = block_on(admin.dispatch(
        Principal::new(&ops),
        "POST",
        "/admin/roles",
        "",
        &form(&[("do", "promote")]),
    ))
    .expect("dispatch");
    assert_eq!(
        response.location(),
        Some("/admin/roles?error=no-action"),
        "the redirect carries a code, not a sentence"
    );

    let plain = page(&admin, &ops, "/admin/roles").text();
    for query in [
        "error=Your+session+expired.+Sign+in+again+at+evil.example",
        "error=the+form+named+no+action",
        "error=",
        "error=no-action-x",
    ] {
        let shown = page_with_query(&admin, &ops, "/admin/roles", query);
        assert!(
            !shown.text().contains("evil.example"),
            "{query}: {}",
            shown.text()
        );
        assert_eq!(
            shown.text(),
            plain,
            "{query}: an unknown code shows nothing"
        );
    }
    page_with_query(&admin, &ops, "/admin/roles", "error=no-role")
        .assert_text("no role has that name");
}

/// What a superuser typed is text on the tab, not markup. What a link puts
/// in `error=` is neither: the tab does not show it.
#[tokio::test]
async fn the_roles_tab_escapes_what_it_shows() {
    let store = Arc::new(MemoryStore::new());
    let admin = admin(&store);
    let ops = superuser(&store).await;
    let define = Submission::from_urlencoded(
        "do=define&role=xss&permissions=%3Cscript%3Ealert(1)%3C%2Fscript%3E",
    );
    assert_eq!(
        status(&admin, Some(&ops), "POST", "/admin/roles", &define),
        303
    );
    let response = block_on(admin.dispatch(
        Principal::new(&ops),
        "GET",
        "/admin/roles",
        "error=%3Cimg+src%3Dx+onerror%3Dalert(2)%3E",
        &Submission::new(),
    ))
    .expect("dispatch");
    let html = response.html().expect("a page");
    assert!(!html.contains("<script>alert(1)"), "{html}");
    assert!(!html.contains("<img src=x"), "{html}");
    let page = Html::new(html.to_string());
    page.assert_text("<script>alert(1)</script>");
    assert!(!page.text().contains("onerror"), "{html}");
}

/// `integrate.admin` off for `superuser`: the admin has no policy, so it is
/// `DenyAll`, and even a superuser is refused.
#[tokio::test]
async fn without_the_staff_policy_the_admin_is_shut() {
    let store = Arc::new(MemoryStore::new());
    let ops = superuser(&store).await;
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(RolesPanel::new(store.clone())),
    );
    for path in ["/admin", "/admin/post", "/admin/roles"] {
        assert_eq!(
            status(&admin, Some(&ops), "GET", path, &Submission::new()),
            403,
            "{path}"
        );
    }
}

/// `integrate.admin` off for `roles`: there is no roles tab, for anybody.
#[tokio::test]
async fn without_the_roles_panel_there_is_no_roles_tab() {
    let store = Arc::new(MemoryStore::new());
    let ops = superuser(&store).await;
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(StaffPolicy),
    );
    assert_eq!(
        status(&admin, Some(&ops), "GET", "/admin", &Submission::new()),
        200
    );
    let define = form(&[("do", "define"), ("role", "x")]);
    assert_eq!(
        status(&admin, Some(&ops), "POST", "/admin/roles", &define),
        404
    );
    assert!(store.roles().await.unwrap().is_empty());
}

/// Before the plugin fills the cell, the tab is an error that says what to
/// do, not a panic and not an empty list.
#[tokio::test]
async fn a_late_store_that_is_not_there_yet_says_so() {
    let store = Arc::new(MemoryStore::new());
    let ops = superuser(&store).await;
    let cell = Late::new();
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(RolesPanel::<MemoryStore>::late(cell.clone()))
            .with_policy(StaffPolicy),
    );
    let error = block_on(admin.dispatch(
        Principal::new(&ops),
        "GET",
        "/admin/roles",
        "",
        &Submission::new(),
    ))
    .expect_err("no store yet");
    assert!(error.to_string().contains("help:"), "{error}");
    assert!(cell.set(store.clone()));
    page(&admin, &ops, "/admin/roles").assert_text("No role has been defined.");
}

/// A principal that is not a `Staff`, or a `Staff` that is not staff, opens
/// nothing; `StaffPolicy` asks no other question.
#[tokio::test]
async fn only_a_staff_principal_passes_the_policy() {
    use brocanth_admin::policy::{Action, Policy, Request};
    let store = Arc::new(MemoryStore::new());
    let plain = account(&store, "plain@example.com").await;
    let plain = staff(&store, &plain).await;
    let ops = superuser(&store).await;
    let index = Request::new(Action::ViewIndex, "");
    assert!(!StaffPolicy.allows(&Principal::anonymous(), index));
    assert!(!StaffPolicy.allows(&Principal::new(&"ops@example.com".to_string()), index));
    assert!(!StaffPolicy.allows(&Principal::new(&plain), index));
    assert!(StaffPolicy.allows(&Principal::new(&ops), index));
    assert!(StaffPolicy.allows(
        &Principal::new(&ops),
        Request::new(Action::ActOnPanel, "roles")
    ));
}

/// The owner of a tab gives and takes `read` and `edit` on that tab, on any
/// role, and nothing else: not `owner`, not another tab, not a role.
#[tokio::test]
async fn an_owner_gives_read_and_edit_on_its_own_tab_only() {
    let store = Arc::new(MemoryStore::new());
    // Rows open, so the screen policy is the only gate.
    let admin = open_rows(&store);
    let ops = superuser(&store).await;
    // A superuser makes ann the owner of `post`.
    let ann = holder(&store, "ann@example.com", &[]).await;
    let ann_id = ann.account().id.clone();
    let give = |role: &str, resource: &str, level: &str| {
        form(&[
            ("do", "grant"),
            ("role", role),
            ("resource", resource),
            ("level", level),
        ])
    };
    let posted = |who: &Staff, submission: &Submission| {
        let response =
            block_on(admin.dispatch(Principal::new(who), "POST", "/admin/roles", "", submission))
                .expect("dispatch");
        response.location().map(str::to_string)
    };
    assert_eq!(
        posted(&ops, &give("r-ann", "post", "owner")).as_deref(),
        Some("/admin/roles")
    );
    let ann = staff(&store, &ann_id).await;
    assert_eq!(ann.owned(), ["post"]);

    // Bob holds nothing on `post` yet.
    let bob = holder(&store, "bob@example.com", &["user.read"]).await;
    let bob_id = bob.account().id.clone();
    assert_eq!(
        status(&admin, Some(&bob), "GET", "/admin/post", &Submission::new()),
        403
    );

    // Ann sees the roles tab, with a form for her tab only, and gives Bob's
    // role `edit` on `post`. The grant is a row in the roles tables, so
    // Bob's next request opens the screens: no code changed.
    let tab = page(&admin, &ann, "/admin/roles");
    tab.assert_button("Give");
    assert!(
        tab.button("Save the role").is_none(),
        "an owner defines no role"
    );
    assert_eq!(
        posted(&ann, &give("r-bob", "post", "edit")).as_deref(),
        Some("/admin/roles")
    );
    let bob = staff(&store, &bob_id).await;
    assert!(bob.allows("post.edit") && bob.allows("post.read"));
    assert_eq!(
        status(
            &admin,
            Some(&bob),
            "GET",
            "/admin/post/1/edit",
            &Submission::new()
        ),
        200
    );

    // Everything past her tab comes back refused, and changes nothing.
    let before = store.roles().await.unwrap();
    for refused in [
        give("r-bob", "post", "owner"),
        give("r-bob", "user", "read"),
        give("r-ann", "user", "edit"),
        form(&[
            ("do", "define"),
            ("role", "x"),
            ("permissions", "post.edit"),
        ]),
        form(&[
            ("do", "assign"),
            ("role", "r-ann"),
            ("email", "bob@example.com"),
        ]),
        form(&[("do", "delete"), ("role", "r-bob")]),
    ] {
        let location = posted(&ann, &refused).expect("a redirect");
        assert!(location.contains("error="), "{refused:?}: {location}");
    }
    assert_eq!(
        store.roles().await.unwrap(),
        before,
        "no refused form changed a role"
    );
    page_with_query(&admin, &ann, "/admin/roles", "error=not-yours")
        .assert_text("only a superuser does that");

    // She takes back what she gave.
    let take = form(&[
        ("do", "revoke"),
        ("role", "r-bob"),
        ("permission", "post.edit"),
    ]);
    assert_eq!(posted(&ann, &take).as_deref(), Some("/admin/roles"));
    assert!(!staff(&store, &bob_id).await.allows("post.read"));

    // Bob, who owns nothing, presses no button on the roles tab.
    assert_eq!(
        status(
            &admin,
            Some(&bob),
            "POST",
            "/admin/roles",
            &give("r-bob", "post", "read")
        ),
        403
    );
}
