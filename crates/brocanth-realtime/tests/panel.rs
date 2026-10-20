//! The broker's tab, rendered through the real admin.
//!
//! These assert on the page and not on a context struct, because a screen
//! that compiles and draws the wrong thing is the failure worth catching.
//! The admin is real, the policy is real, and the broker has real clients
//! with real subscriptions on it.
//!
//! The questions are asked with [`brocanth_test::Html`] rather than with
//! `contains`, because "the audit table names this topic" and "that string
//! occurs somewhere in the document" are different questions and only one of
//! them is the one these tests mean.

#![cfg(feature = "admin")]

mod common;

use std::sync::Arc;

use brocanth_admin::policy::{policy_fn, Action, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_policy::Pred;
use brocanth_realtime::admin::RealtimePanel;
use brocanth_realtime::{Broker, Client, Subscription, Topic};
use brocanth_test::Html;

use common::{not_draft, PostColumn};

struct Operator;

/// An admin with the broker's tab on it. `act` decides whether this person
/// may press buttons — which this panel has none of, and that is one of the
/// things these tests check.
fn admin(broker: &Arc<Broker>, act: bool) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .panel(RealtimePanel::new(Arc::clone(broker)))
            .with_policy(policy_fn(move |_: &Principal<'_>, r: Request<'_>| {
                act || r.action != Action::ActOnPanel
            })),
    )
}

fn posts() -> Topic {
    Topic::collection("posts").unwrap()
}

fn get(admin: &Admin, path: &str) -> Html {
    let who = Operator;
    let response =
        block_on(admin.dispatch(Principal::new(&who), "GET", path, "", &Submission::new()))
            .expect("dispatch");
    assert_eq!(response.status(), 200, "{path}");
    Html::new(response.html().expect("a page"))
}

fn status(admin: &Admin, method: &str, path: &str) -> u16 {
    let who = Operator;
    block_on(admin.dispatch(Principal::new(&who), method, path, "", &Submission::new()))
        .expect("dispatch")
        .status()
}

/// A broker with one subscription the index can key and one it cannot.
///
/// The clients come back with it: dropping a [`Client`] disconnects it, so a
/// helper that kept them to itself would hand back an empty broker.
fn mixed() -> (Arc<Broker>, Vec<Client>) {
    let broker = Broker::new();

    let keyed = broker.connect().unwrap();
    keyed
        .subscribe(Subscription::gated_by(
            posts(),
            Pred::eq(PostColumn::AuthorId, 7i64),
        ))
        .unwrap();

    // `status <> 'draft'` is a complement, so the policy does not invert and
    // this subscription is evaluated on every change of the topic.
    let scanned = broker.connect().unwrap();
    scanned
        .subscribe(Subscription::gated_by(posts(), not_draft()))
        .unwrap();

    (broker, vec![keyed, scanned])
}

#[test]
fn an_empty_broker_says_so_rather_than_claiming_a_perfect_index() {
    let broker = Broker::new();
    let page = get(&admin(&broker, true), "/admin/realtime");

    page.assert_text("Nothing is subscribed");
    page.assert_text("No topic is on the slow path");
}

#[test]
fn the_split_is_the_first_thing_on_the_screen() {
    let (broker, _clients) = mixed();
    let page = get(&admin(&broker, true), "/admin/realtime");

    // The split lives in its own labelled region, and both halves of the
    // pair are in it. Scoped, because "1" occurs all over a page of counts.
    let split = page.within("section", "aria-label", "Subscription index");
    split.assert_text("On the index");
    split.assert_text("Scanned per change");
    split.assert_text("50% of what can receive is on the index.");
    split.assert_text("One subscription is checked by hand");

    // And it is above the counts it is the headline for. The assertion is
    // about order, which is the whole claim this screen makes.
    let source = page.source();
    let split_at = source.find("Subscription index").expect("the split");
    let counts_at = source.find("connections held").expect("the count cards");
    assert!(split_at < counts_at, "the split comes first");
}

#[test]
fn the_audit_names_the_topic_and_quotes_the_reason() {
    let (broker, _clients) = mixed();
    let page = get(&admin(&broker, true), "/admin/realtime");

    page.assert_no_text("No topic is on the slow path");
    page.assert_text("posts");
    // Verbatim from `brocanth_policy::NotInvertible`, which is the point:
    // the screen says what to change in the policy, not merely that
    // something is slow.
    assert!(
        page.text().contains("complement"),
        "the reason the policy would not invert: {}",
        page.text()
    );
}

#[test]
fn a_subscription_the_policy_denies_outright_is_called_a_likely_bug() {
    let broker = Broker::new();
    let denied = broker.connect().unwrap();
    denied
        .subscribe(Subscription::gated_by(posts(), Pred::<PostColumn>::Never))
        .unwrap();

    let page = get(&admin(&broker, true), "/admin/realtime");
    page.assert_text("receives nothing at all");
}

#[test]
fn the_tab_is_in_the_rail_and_the_screen_has_no_buttons() {
    let (broker, _clients) = mixed();

    for act in [true, false] {
        let a = admin(&broker, act);
        let page = get(&a, "/admin/realtime");

        page.within("nav", "aria-label", "Sections")
            .assert_link_to("/admin/realtime");

        // Nothing here changes anything, so there is nothing to hide from a
        // reader who may not act: the screen is the same either way.
        assert!(
            page.buttons().is_empty(),
            "a read-only panel draws no buttons"
        );
        assert!(page.forms().is_empty(), "and posts nothing");
    }
}

#[test]
fn a_sub_path_and_a_post_are_both_the_admins_404() {
    let (broker, _clients) = mixed();
    let a = admin(&broker, true);

    assert_eq!(status(&a, "GET", "/admin/realtime/clients"), 404);
    // A POST with permission still has no screen behind it.
    assert_eq!(status(&a, "POST", "/admin/realtime"), 404);
    // And without permission the admin refuses before the panel runs.
    assert_eq!(
        status(&admin(&broker, false), "POST", "/admin/realtime"),
        403
    );
}

/// Prints the screen, for a human who wants to see it.
#[test]
#[ignore = "prints a page, asserts nothing"]
fn show() {
    let (broker, _clients) = mixed();
    println!("{}", get(&admin(&broker, true), "/admin/realtime").source());
}
