//! Login alerts: what "somewhere new" means here, and what it does not mean.

mod common;

use brocanth_auth::alerts::LoginAlert;
use brocanth_auth::clock::Clock;
use brocanth_auth::identity::{AccountId, EmailAddress};
use brocanth_auth::storage::{AuthStore, NewAccount};
use common::Fixture;

async fn an_account(f: &Fixture, email: &str) -> AccountId {
    f.store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse(email).unwrap()),
            email_verified: true,
            password_hash: None,
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn the_first_sign_in_is_new_but_says_it_is_the_first() {
    let f = Fixture::new();
    let alerts = f.alerts();
    let account = an_account(&f, "ada@example.com").await;

    match alerts.note(&account, Some("203.0.113.7")).await.unwrap() {
        LoginAlert::Unfamiliar {
            account: who,
            first_ever,
            at,
        } => {
            assert_eq!(who, account);
            assert!(first_ever, "a welcome, not a warning");
            assert_eq!(at, f.clock.now());
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_network_already_seen_is_silent() {
    let f = Fixture::new();
    let alerts = f.alerts();
    let account = an_account(&f, "ada@example.com").await;

    alerts.note(&account, Some("203.0.113.7")).await.unwrap();
    let first_seen = f.clock.now();
    f.clock.advance(3600);

    // A different address in the same /24. The definition is the network, so
    // this is the same place.
    match alerts.note(&account, Some("203.0.113.209")).await.unwrap() {
        LoginAlert::Familiar { last_seen_at } => assert_eq!(last_seen_at, first_seen),
        other => panic!("a sibling address alerted: {other:?}"),
    }
}

#[tokio::test]
async fn a_different_network_alerts_and_is_not_the_first() {
    let f = Fixture::new();
    let alerts = f.alerts();
    let account = an_account(&f, "ada@example.com").await;

    alerts.note(&account, Some("203.0.113.7")).await.unwrap();
    match alerts.note(&account, Some("198.51.100.7")).await.unwrap() {
        LoginAlert::Unfamiliar { first_ever, .. } => {
            assert!(!first_ever, "the account had been seen before, elsewhere")
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_network_goes_unfamiliar_again_once_it_is_old_enough() {
    let f = Fixture::new();
    let alerts = f.alerts();
    let account = an_account(&f, "ada@example.com").await;

    alerts.note(&account, Some("203.0.113.7")).await.unwrap();
    f.clock.advance(alerts.familiar_for() - 1);
    assert!(matches!(
        alerts.note(&account, Some("203.0.113.7")).await.unwrap(),
        LoginAlert::Familiar { .. }
    ));

    // The window restarts from the last use, not from the first.
    f.clock.advance(alerts.familiar_for());
    match alerts.note(&account, Some("203.0.113.7")).await.unwrap() {
        LoginAlert::Unfamiliar { first_ever, .. } => assert!(!first_ever),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn two_accounts_do_not_share_a_location() {
    let f = Fixture::new();
    let alerts = f.alerts();
    let ada = an_account(&f, "ada@example.com").await;
    let grace = an_account(&f, "grace@example.com").await;

    alerts.note(&ada, Some("203.0.113.7")).await.unwrap();
    assert!(matches!(
        alerts.note(&grace, Some("203.0.113.7")).await.unwrap(),
        LoginAlert::Unfamiliar { .. }
    ));
}

#[tokio::test]
async fn no_address_means_no_judgement_rather_than_an_alert() {
    let f = Fixture::new();
    let alerts = f.alerts();
    let account = an_account(&f, "ada@example.com").await;

    // Treating "unknown" as "new" alerts on every single sign-in behind a
    // misconfigured proxy, which teaches people to delete the mail unread.
    assert_eq!(
        alerts.note(&account, None).await.unwrap(),
        LoginAlert::NotJudged
    );
    assert_eq!(
        alerts.note(&account, Some("not an address")).await.unwrap(),
        LoginAlert::NotJudged
    );
}

#[tokio::test]
async fn nothing_about_the_address_is_stored() {
    let f = Fixture::new();
    let alerts = f.alerts();
    let account = an_account(&f, "ada@example.com").await;
    alerts.note(&account, Some("203.0.113.7")).await.unwrap();

    // The row is a keyed digest. Nobody reading the store finds a /24 in it,
    // and nobody without the signing key can derive one — an unkeyed digest of
    // a /24 is 2^24 guesses, which is not a hash.
    let stored = format!("{:?}", f.store);
    assert!(!stored.contains("203.0.113"), "{stored}");
    assert_eq!(alerts.fingerprint("203.0.113.7").unwrap().len(), 32);
}

#[tokio::test]
async fn a_sweeper_forgets_where_somebody_was() {
    let f = Fixture::new();
    let alerts = f.alerts();
    let account = an_account(&f, "ada@example.com").await;

    alerts.note(&account, Some("203.0.113.7")).await.unwrap();
    alerts.note(&account, Some("198.51.100.7")).await.unwrap();

    assert_eq!(alerts.purge(f.clock.now()).await.unwrap(), 0);
    f.clock.advance(alerts.familiar_for() + 1);
    assert_eq!(
        alerts
            .purge(f.clock.now() - alerts.familiar_for())
            .await
            .unwrap(),
        2
    );
    // Forgotten, so it is new again.
    assert!(matches!(
        alerts.note(&account, Some("203.0.113.7")).await.unwrap(),
        LoginAlert::Unfamiliar { .. }
    ));
}
