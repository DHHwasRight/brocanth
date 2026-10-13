//! Impersonation: both identities on the row, and the five things an operator
//! wearing somebody's face cannot do.

mod common;

use brocanth_auth::error::AuthError;
use brocanth_auth::identity::{AccountId, EmailAddress};
use brocanth_auth::impersonation::{AccountHolder, Restricted};
use brocanth_auth::mfa::FactorSet;
use brocanth_auth::session::Privilege;
use brocanth_auth::storage::{AuthStore, NewAccount, SessionRecord};
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

/// An operator with a session that has re-proved itself recently.
async fn an_elevated_operator(f: &Fixture) -> SessionRecord {
    let sessions = f.sessions();
    let operator = an_account(f, "ops@example.com").await;
    common::elevated(&sessions, &operator).await.session
}

#[tokio::test]
async fn an_impersonated_session_carries_both_identities() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let operator = an_elevated_operator(&f).await;
    let subject = an_account(&f, "ada@example.com").await;

    let acting = sessions
        .impersonate(&operator, &subject, "TICKET-4471")
        .await
        .unwrap()
        .session;

    // The account is the subject, so every authorization check in the
    // application keeps working unchanged. That is the point of the feature.
    assert_eq!(acting.account, subject);
    // And the operator is on the row, with their own session, for the audit.
    assert!(acting.is_impersonated());
    assert_eq!(acting.operator(), Some(&operator.account));
    let marker = acting.impersonation.as_ref().unwrap();
    assert_eq!(marker.operator_session, operator.id);
    assert_eq!(marker.reason, "TICKET-4471");
    assert_eq!(marker.started_at, acting.created_at);

    // Nobody proved anything *as the subject*, and the row says so.
    assert_eq!(acting.factors, FactorSet::empty());
    assert_eq!(acting.privilege, Privilege::Authenticated);

    // It is read back from the store exactly as it was written: an auditor
    // reading the table sees both names.
    let stored = f.store.session(&acting.id).await.unwrap().unwrap();
    assert_eq!(stored.impersonation, acting.impersonation);
}

#[tokio::test]
async fn an_impersonated_session_cannot_change_the_password() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let accounts = f.accounts();
    let operator = an_elevated_operator(&f).await;

    let registered = accounts
        .register("ada@example.com", "the real password")
        .await
        .unwrap();
    let subject = match registered.notice {
        brocanth_auth::accounts::RegistrationNotice::Created { account, .. } => account.id,
        other => panic!("{other:?}"),
    };

    let acting = sessions
        .impersonate(&operator, &subject, "TICKET-4471")
        .await
        .unwrap()
        .session;

    // The refusal is in the type: `change_password` takes an `AccountHolder`,
    // and an impersonated session cannot produce one. There is no argument
    // this handler could pass instead.
    let err = AccountHolder::of(&acting).unwrap_err();
    assert!(matches!(err, AuthError::Impersonated), "{err:?}");

    // The subject's own session still can.
    let theirs = common::signed_in(&sessions, &subject).await.session;
    let who = AccountHolder::of(&theirs).expect("their own session");
    accounts
        .change_password(&who, "the real password", "a new one")
        .await
        .expect("the account holder may");
    let after = accounts
        .login("ada@example.com", "a new one")
        .await
        .expect("and it took effect");
    assert_eq!(after.account().id, subject);
}

#[tokio::test]
async fn an_impersonated_session_cannot_elevate() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let operator = an_elevated_operator(&f).await;
    let subject = an_account(&f, "ada@example.com").await;

    let acting = sessions
        .impersonate(&operator, &subject, "TICKET-4471")
        .await
        .unwrap()
        .session;

    let err = sessions
        .elevate(&acting.id, common::elevation(&subject))
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Impersonated), "{err:?}");

    // Rotating for any other reason still works, and stays impersonated.
    let again = sessions.rotate(&acting.id).await.unwrap().session;
    assert!(again.is_impersonated(), "a rotation laundered the marker");
    assert_eq!(again.operator(), Some(&operator.account));
}

#[tokio::test]
async fn impersonation_does_not_nest() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let operator = an_elevated_operator(&f).await;
    let subject = an_account(&f, "ada@example.com").await;
    let third = an_account(&f, "grace@example.com").await;

    let acting = sessions
        .impersonate(&operator, &subject, "TICKET-4471")
        .await
        .unwrap()
        .session;

    // A chain records one operator and hides the rest, which is worse than
    // recording nothing because it looks like a record.
    let err = sessions
        .impersonate(&acting, &third, "TICKET-4472")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Impersonated), "{err:?}");
}

#[tokio::test]
async fn an_operator_who_has_not_re_proved_themselves_cannot_start() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let operator = an_account(&f, "ops@example.com").await;
    let subject = an_account(&f, "ada@example.com").await;

    let ordinary = common::signed_in(&sessions, &operator).await.session;
    let err = sessions
        .impersonate(&ordinary, &subject, "TICKET-4471")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::ElevationRequired), "{err:?}");
}

#[tokio::test]
async fn a_reason_is_not_optional() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let operator = an_elevated_operator(&f).await;
    let subject = an_account(&f, "ada@example.com").await;

    for empty in ["", "   "] {
        assert!(sessions
            .impersonate(&operator, &subject, empty)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn an_impersonated_session_is_short_lived() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let operator = an_elevated_operator(&f).await;
    let subject = an_account(&f, "ada@example.com").await;

    let acting = sessions
        .impersonate(&operator, &subject, "TICKET-4471")
        .await
        .unwrap()
        .session;
    let limit = sessions.config().impersonation_timeout;
    assert_eq!(acting.absolute_expires_at, acting.created_at + limit);
    assert!(
        limit < sessions.config().absolute_timeout,
        "an operator's borrowed identity must not outlive their own session"
    );
}

#[tokio::test]
async fn ending_an_impersonation_deletes_it_and_returns_the_operators_cookie() {
    let f = Fixture::new();
    let sessions = f.sessions();
    let operator = an_elevated_operator(&f).await;
    let subject = an_account(&f, "ada@example.com").await;

    let acting = sessions
        .impersonate(&operator, &subject, "TICKET-4471")
        .await
        .unwrap()
        .session;

    let cookie = sessions.end_impersonation(&acting).await.unwrap();
    assert!(cookie.as_str().contains(&sessions.seal(&operator.id)));
    assert!(
        f.store.session(&acting.id).await.unwrap().is_none(),
        "the impersonated session was laundered rather than deleted"
    );
    // The operator's own session is untouched.
    assert!(f.store.session(&operator.id).await.unwrap().is_some());

    // And it is not something you can do to an ordinary session.
    let theirs = common::signed_in(&sessions, &subject).await.session;
    assert!(sessions.end_impersonation(&theirs).await.is_err());
}

#[test]
fn the_list_of_refusals_names_where_each_one_lives() {
    // A test that fails when somebody quietly shortens the list.
    assert_eq!(Restricted::all().len(), 5);
    for refusal in Restricted::all() {
        assert!(!refusal.enforced_by().is_empty(), "{refusal}");
        assert!(!refusal.to_string().is_empty());
    }
}
