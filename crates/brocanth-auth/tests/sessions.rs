//! Sessions: rotation on privilege change, both expiries, and revocation.

mod common;

use brocanth_auth::accounts::Accounts;
use brocanth_auth::clock::Clock;
use brocanth_auth::identity::AccountId;
use brocanth_auth::session::Privilege;
use brocanth_auth::storage::{AuthStore, NewAccount};
use common::Fixture;

async fn an_account(f: &Fixture) -> AccountId {
    f.store
        .create_account(NewAccount {
            email: Some("ada@example.com".parse_email()),
            email_verified: true,
            password_hash: None,
        })
        .await
        .expect("create")
        .id
}

trait ParseEmail {
    fn parse_email(&self) -> brocanth_auth::identity::EmailAddress;
}
impl ParseEmail for &str {
    fn parse_email(&self) -> brocanth_auth::identity::EmailAddress {
        brocanth_auth::identity::EmailAddress::parse(self).expect("address")
    }
}

#[tokio::test]
async fn a_session_rotates_on_privilege_change() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let account = an_account(&f).await;

    // The pre-login session: a cart, a flash message, no identity.
    let before = sessions
        .begin(&account, Privilege::Anonymous)
        .await
        .expect("begin");
    let fixed_by_an_attacker = before.session.id.clone();

    f.clock.advance(5);
    let after = sessions
        .rotate(&fixed_by_an_attacker, Privilege::Authenticated)
        .await
        .expect("rotate");

    // A new identifier. This is the whole session-fixation defence.
    assert_ne!(after.session.id, fixed_by_an_attacker);
    assert_eq!(after.session.account, account);
    assert_eq!(after.session.privilege, Privilege::Authenticated);

    // The old identifier is gone from the store, so the cookie the attacker
    // planted is now worth nothing.
    assert!(f
        .store
        .session(&fixed_by_an_attacker)
        .await
        .unwrap()
        .is_none());
    assert_eq!(f.store.session_count(), 1);

    // The absolute deadline is carried over, not reset: rotating repeatedly
    // must not be a way to hold a session open forever.
    assert_eq!(
        after.session.absolute_expires_at,
        before.session.absolute_expires_at
    );

    // The cookie that comes back carries the new identifier, signed.
    let value = after
        .cookie
        .as_str()
        .split(';')
        .next()
        .unwrap()
        .split_once('=')
        .unwrap()
        .1
        .to_string();
    assert_eq!(sessions.unseal(&value).as_ref(), Some(&after.session.id));
}

#[tokio::test]
async fn elevating_rotates_again() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let account = an_account(&f).await;

    let signed_in = sessions
        .begin(&account, Privilege::Authenticated)
        .await
        .unwrap();
    let elevated = sessions
        .rotate(&signed_in.session.id, Privilege::Elevated)
        .await
        .unwrap();
    assert_ne!(elevated.session.id, signed_in.session.id);
    assert_eq!(elevated.session.privilege, Privilege::Elevated);
    assert_eq!(f.store.session_count(), 1);
}

#[tokio::test]
async fn revocation_ends_every_session_for_an_account() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let ada = an_account(&f).await;
    let grace = f
        .store
        .create_account(NewAccount {
            email: Some("grace@example.com".parse_email()),
            email_verified: true,
            password_hash: None,
        })
        .await
        .unwrap()
        .id;

    // Three devices for Ada, one for Grace.
    let a = sessions.begin(&ada, Privilege::Authenticated).await.unwrap();
    let b = sessions.begin(&ada, Privilege::Authenticated).await.unwrap();
    let c = sessions.begin(&ada, Privilege::Elevated).await.unwrap();
    let d = sessions
        .begin(&grace, Privilege::Authenticated)
        .await
        .unwrap();
    assert_eq!(f.store.session_count(), 4);

    let ended = sessions.revoke_all(&ada).await.unwrap();
    assert_eq!(ended, 3);

    for session in [&a, &b, &c] {
        assert!(f
            .store
            .session(&session.session.id)
            .await
            .unwrap()
            .is_none());
    }
    // Grace is untouched: revocation is per account, not global.
    assert!(f.store.session(&d.session.id).await.unwrap().is_some());
}

#[tokio::test]
async fn an_idle_session_dies_and_is_deleted_by_the_request_that_found_it() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let account = an_account(&f).await;
    let started = sessions
        .begin(&account, Privilege::Authenticated)
        .await
        .unwrap();
    let cookie = sessions.seal(&started.session.id);

    // Just inside the idle window: alive, and the timer is refreshed.
    f.clock.advance(sessions.config().idle_timeout - 1);
    let loaded = sessions.load(&cookie).await.expect("still alive");
    assert_eq!(loaded.last_seen_at, f.clock.now());

    // The refresh moved the window, so the same wait again is still fine.
    f.clock.advance(sessions.config().idle_timeout - 1);
    sessions.load(&cookie).await.expect("refreshed");

    // Past it: dead, and the row is gone.
    f.clock.advance(sessions.config().idle_timeout);
    assert!(sessions.load(&cookie).await.is_err());
    assert_eq!(f.store.session_count(), 0);
}

#[tokio::test]
async fn an_absolutely_expired_session_dies_however_busy_it_was() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let account = an_account(&f).await;
    let started = sessions
        .begin(&account, Privilege::Authenticated)
        .await
        .unwrap();
    let cookie = sessions.seal(&started.session.id);

    // Used constantly, so the idle timer never fires.
    let step = sessions.config().idle_timeout / 2;
    let mut elapsed = 0;
    while elapsed + step < sessions.config().absolute_timeout {
        f.clock.advance(step);
        elapsed += step;
        sessions.load(&cookie).await.expect("busy and alive");
    }

    f.clock.advance(sessions.config().absolute_timeout);
    assert!(sessions.load(&cookie).await.is_err());
}

#[tokio::test]
async fn a_forged_cookie_never_reaches_the_store() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let account = an_account(&f).await;
    let started = sessions
        .begin(&account, Privilege::Authenticated)
        .await
        .unwrap();

    // The right identifier with the wrong signature.
    let forged = format!("{}.{}", started.session.id, "AAAAAAAAAAAAAAAAAAAAAA");
    assert!(sessions.load(&forged).await.is_err());
    // Still there: the forgery was rejected by the HMAC, not by the store.
    assert!(f
        .store
        .session(&started.session.id)
        .await
        .unwrap()
        .is_some());

    // And the real one still works.
    sessions
        .load(&sessions.seal(&started.session.id))
        .await
        .expect("the genuine cookie");
}

#[tokio::test]
async fn a_password_reset_revokes_every_session_without_being_asked() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let accounts = Accounts::new(f.store.clone(), f.clock.clone(), f.passwords.clone())
        .expect("accounts");

    let registered = accounts
        .register("ada@example.com", "a long enough password")
        .await
        .expect("register");
    let account = match registered.notice {
        brocanth_auth::accounts::RegistrationNotice::Created { account, .. } => account,
        other => panic!("expected a new account, got {other:?}"),
    };

    // Two live sessions — one of them the attacker's.
    let mine = sessions
        .begin(&account.id, Privilege::Authenticated)
        .await
        .unwrap();
    let theirs = sessions
        .begin(&account.id, Privilege::Authenticated)
        .await
        .unwrap();
    assert_eq!(f.store.session_count(), 2);

    let requested = accounts
        .request_password_reset("ada@example.com")
        .await
        .expect("reset requested");
    let token = match requested.outcome {
        brocanth_auth::accounts::ResetOutcome::Issue { token, .. } => token,
        other => panic!("expected a token, got {other:?}"),
    };
    accounts
        .reset_password(token.secret(), "a different long password")
        .await
        .expect("reset");

    // A reset is what somebody does when they think they are compromised. A
    // reset that leaves the attacker signed in has done nothing.
    assert_eq!(f.store.session_count(), 0);
    assert!(sessions
        .load(&sessions.seal(&mine.session.id))
        .await
        .is_err());
    assert!(sessions
        .load(&sessions.seal(&theirs.session.id))
        .await
        .is_err());
}
