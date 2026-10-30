//! The accounts tab, rendered through the real admin.
//!
//! These assert on HTML and not on a context struct, because a screen that
//! compiles and draws the wrong thing is the failure worth catching. The
//! admin is real, the policy is real, and the store is a real `MemoryStore`
//! with real rows in it.
//!
//! The assertions go through `brocanth_test::Html`, so "there is a button
//! that says End every session" is a question about an element rather than a
//! `contains` that also passes when the words are in a comment.

#![cfg(feature = "admin")]

use std::sync::Arc;

use brocanth_admin::policy::{policy_fn, Action, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_auth::admin::AccountsPanel;
use brocanth_auth::identity::{AccountId, EmailAddress};
use brocanth_auth::impersonation::Impersonation;
use brocanth_auth::memory::MemoryStore;
use brocanth_auth::mfa::{FactorKind, FactorSet};
use brocanth_auth::session::{Privilege, SessionId};
use brocanth_auth::storage::{AccountDirectory, AuthStore, NewAccount, SessionRecord};
use brocanth_test::Html;

struct Operator;

/// 2023-11-14 22:13 UTC, which is the timestamp the screens print.
const MADE_AT: i64 = 1_700_000_000;

// ---------------------------------------------------------------------------
// Scaffolding
// ---------------------------------------------------------------------------

fn store() -> Arc<MemoryStore> {
    let store = Arc::new(MemoryStore::new());
    store.set_now(MADE_AT);
    store
}

async fn account(
    store: &MemoryStore,
    email: Option<&str>,
    verified: bool,
    password: bool,
) -> AccountId {
    store
        .create_account(NewAccount {
            email: email.map(|e| EmailAddress::parse(e).expect("address")),
            email_verified: verified,
            // A PHC string is opaque to every screen here; what the column
            // reports is whether there is one.
            password_hash: password
                .then(|| "$argon2id$v=19$m=19456,t=2,p=1$c2FsdA$aGFzaA".to_string()),
        })
        .await
        .expect("create")
        .id
}

async fn session(store: &MemoryStore, of: &AccountId, id: &str) -> SessionId {
    let session = SessionRecord {
        id: SessionId::new(id),
        account: of.clone(),
        privilege: Privilege::Authenticated,
        created_at: MADE_AT,
        last_seen_at: MADE_AT + 60,
        absolute_expires_at: MADE_AT + 86_400,
        factors: FactorSet::of([FactorKind::Password, FactorKind::Totp]),
        impersonation: None,
    };
    store
        .create_session(session.clone())
        .await
        .expect("session");
    session.id
}

/// An admin with the accounts tab on it. `act` decides whether this person
/// may press the buttons.
fn admin<S: AccountDirectory>(store: Arc<S>, act: bool) -> Admin {
    admin_with(AccountsPanel::new(store), act)
}

fn admin_with<S: AccountDirectory>(panel: AccountsPanel<S>, act: bool) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(panel)
            .with_policy(policy_fn(move |_: &Principal<'_>, r: Request<'_>| {
                act || r.action != Action::ActOnPanel
            })),
    )
}

fn get(admin: &Admin, path: &str) -> Html {
    let who = Operator;
    let (path, query) = match path.split_once('?') {
        Some((path, query)) => (path, query),
        None => (path, ""),
    };
    let response =
        block_on(admin.dispatch(Principal::new(&who), "GET", path, query, &Submission::new()))
            .expect("dispatch");
    assert_eq!(response.status(), 200, "{path}");
    Html::new(response.html().expect("a page").to_string())
}

fn status(admin: &Admin, path: &str) -> u16 {
    let who = Operator;
    block_on(admin.dispatch(Principal::new(&who), "GET", path, "", &Submission::new()))
        .expect("dispatch")
        .status()
}

fn post(admin: &Admin, path: &str, body: &Submission) -> (u16, Option<String>) {
    let who = Operator;
    let response =
        block_on(admin.dispatch(Principal::new(&who), "POST", path, "", body)).expect("dispatch");
    (response.status(), response.location().map(str::to_string))
}

// ---------------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_list_shows_what_an_operator_came_for() {
    let store = store();
    account(&store, Some("ada@example.com"), true, true).await;
    account(&store, Some("grace@example.com"), false, false).await;
    // An account made from a provider that reported no address.
    account(&store, None, false, false).await;

    let page = get(&admin(store.clone(), true), "/admin/accounts");
    page.assert_text("Accounts");
    page.assert_text("3 accounts");
    page.assert_text("ada@example.com");
    page.assert_text("grace@example.com");
    // Not an empty cell, which reads as a bug rather than as a fact.
    page.assert_text("no address");
    // Whether the address is proven, whether a password is set, and when the
    // account was made. The four columns the screen exists for.
    page.assert_text("provider only");
    page.assert_text("2023-11-14 22:13 UTC");
}

#[tokio::test]
async fn the_list_says_plainly_that_the_store_is_a_hashmap() {
    // The screen looks like a user database. It is a `HashMap` behind a
    // `Mutex`, so the screen has to say so where somebody about to act can
    // read it, and name the store that would keep the accounts.
    let store = store();
    account(&store, Some("ada@example.com"), true, true).await;

    let page = get(&admin(store.clone(), true), "/admin/accounts");
    page.assert_text("These accounts are held in memory.");
    page.assert_text("gone when this process restarts");
    page.assert_text("PgStore, behind the postgres feature");
}

#[tokio::test]
async fn an_empty_store_says_so_rather_than_drawing_an_empty_table() {
    let page = get(&admin(store(), true), "/admin/accounts");
    page.assert_text("No account has been created.");
    page.assert_text("0 accounts");
}

#[tokio::test]
async fn paging_walks_the_whole_list_and_shows_each_account_once() {
    let store = store();
    for n in 0..5 {
        account(&store, Some(&format!("person{n}@example.com")), true, true).await;
    }
    let a = admin_with(AccountsPanel::new(store.clone()).per_page(2), true);

    let mut seen = Vec::new();
    for page in 1..=3 {
        let html = get(&a, &format!("/admin/accounts?page={page}"));
        html.assert_text(&format!("Page {page} of 3"));
        for n in 0..5 {
            if html.source().contains(&format!("person{n}@example.com")) {
                seen.push(n);
            }
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, vec![0, 1, 2, 3, 4], "every account, exactly once");
}

// ---------------------------------------------------------------------------
// One account
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_account_lists_the_sessions_open_on_it() {
    let store = store();
    let ada = account(&store, Some("ada@example.com"), true, true).await;
    let other = account(&store, Some("grace@example.com"), true, true).await;
    session(&store, &ada, "ada-session-one").await;
    session(&store, &ada, "ada-session-two").await;
    session(&store, &other, "grace-session").await;

    let a = admin(store.clone(), true);
    let list = get(&a, "/admin/accounts");
    let href = format!("/admin/accounts/account?id={ada}");
    list.assert_link_to(&href);

    let page = get(&a, &href);
    page.assert_text("ada@example.com");
    // A prefix of the identifier and never the whole one: the whole one is
    // what the cookie carries.
    page.assert_text("ada-session-…");
    page.assert_no_text("ada-session-one");
    // What proved the session, which is the column an incident reads.
    page.assert_text("password, totp");
    page.assert_text("signed in");
    // And not somebody else's session.
    page.assert_no_text("grace-session");
}

#[tokio::test]
async fn a_session_opened_by_an_operator_does_not_read_as_an_ordinary_login() {
    let store = store();
    let ada = account(&store, Some("ada@example.com"), true, true).await;
    let operator = account(&store, Some("ops@example.com"), true, true).await;
    store
        .create_session(SessionRecord {
            id: SessionId::new("impersonated-session"),
            account: ada.clone(),
            privilege: Privilege::Authenticated,
            created_at: MADE_AT,
            last_seen_at: MADE_AT,
            absolute_expires_at: MADE_AT + 3_600,
            // Nobody proved anything *as this account*, which is the point.
            factors: FactorSet::empty(),
            impersonation: Some(Impersonation {
                operator: operator.clone(),
                operator_session: SessionId::new("operator-session"),
                started_at: MADE_AT,
                reason: "ticket SUP-4412".to_string(),
            }),
        })
        .await
        .expect("session");

    let page = get(
        &admin(store.clone(), true),
        &format!("/admin/accounts/account?id={ada}"),
    );
    page.assert_text("impersonated by");
    page.assert_text("ticket SUP-4412");
    // An empty factor set is printed, not left blank for somebody to read as
    // "password".
    page.assert_text("none");
}

#[tokio::test]
async fn an_account_nobody_is_signed_in_as_says_so() {
    let store = store();
    let ada = account(&store, Some("ada@example.com"), true, true).await;
    let page = get(
        &admin(store.clone(), true),
        &format!("/admin/accounts/account?id={ada}"),
    );
    page.assert_text("Nobody is signed in as this account.");
    page.assert_no_button("End every session");
}

#[tokio::test]
async fn an_account_that_is_not_there_is_the_admins_404() {
    let store = store();
    assert_eq!(
        status(
            &admin(store.clone(), true),
            "/admin/accounts/account?id=nope"
        ),
        404
    );
    // And so is a sub-path the panel does not serve.
    assert_eq!(status(&admin(store, true), "/admin/accounts/nope"), 404);
}

// ---------------------------------------------------------------------------
// The one button
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ending_every_session_ends_them_and_redirects() {
    let store = store();
    let ada = account(&store, Some("ada@example.com"), true, true).await;
    session(&store, &ada, "ada-session-one").await;
    session(&store, &ada, "ada-session-two").await;
    let a = admin(store.clone(), true);

    let page = get(&a, &format!("/admin/accounts/account?id={ada}"));
    page.assert_form_posts_to("/admin/accounts/account/end-sessions");
    page.assert_button("End every session");
    // The account travels in the body and not in the path, because an
    // account id is opaque.
    page.assert_field("id");

    let (status, location) = post(
        &a,
        "/admin/accounts/account/end-sessions",
        &Submission::new().with("id", ada.as_str()),
    );
    assert_eq!(
        status, 303,
        "a redirect, so a reload does not press it twice"
    );
    assert_eq!(
        location.as_deref(),
        Some(format!("/admin/accounts/account?id={ada}").as_str())
    );
    assert_eq!(
        store.sessions_for_account(&ada).await.expect("sessions"),
        Vec::new(),
        "every session is gone"
    );
}

#[tokio::test]
async fn a_reader_who_may_not_act_is_not_shown_the_button() {
    // The whole reason reading and acting are two policy questions. Somebody
    // who may look at an account must not be one click from signing them out
    // of everything.
    let store = store();
    let ada = account(&store, Some("ada@example.com"), true, true).await;
    session(&store, &ada, "ada-session-one").await;

    let readonly = admin(store.clone(), false);
    let page = get(&readonly, &format!("/admin/accounts/account?id={ada}"));
    page.assert_text("ada-session-…");
    page.assert_no_button("End every session");

    // And pressing it anyway is refused rather than merely hidden.
    let (status, _) = post(
        &readonly,
        "/admin/accounts/account/end-sessions",
        &Submission::new().with("id", ada.as_str()),
    );
    assert_eq!(status, 403);
    assert_eq!(
        store
            .sessions_for_account(&ada)
            .await
            .expect("sessions")
            .len(),
        1,
        "and nothing happened"
    );
}

/// Prints the two screens, for a human who wants to see them.
#[tokio::test]
#[ignore = "prints a page, asserts nothing"]
async fn show() {
    let store = store();
    let ada = account(&store, Some("ada@example.com"), true, true).await;
    account(&store, None, false, false).await;
    session(&store, &ada, "ada-session-one").await;

    let a = admin(store.clone(), true);
    for path in [
        "/admin/accounts".to_string(),
        format!("/admin/accounts/account?id={ada}"),
    ] {
        println!("\n===== {path} =====");
        println!("{}", get(&a, &path).source());
    }
}
