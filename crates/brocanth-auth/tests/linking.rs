//! Identity linking, and the refusal that is the whole point of the module.

mod common;

use brocanth_auth::error::AuthError;
use brocanth_auth::identity::{EmailAddress, ProviderIdentity};
use brocanth_auth::linking::{LinkRefusal, Linking, SignInOutcome};
use brocanth_auth::storage::{AuthStore, NewAccount};
use common::Fixture;
use serde_json::json;

fn identity(
    provider: &str,
    subject: &str,
    email: Option<&str>,
    verified: bool,
) -> ProviderIdentity {
    ProviderIdentity {
        provider: provider.into(),
        subject: subject.into(),
        email: email.map(|e| EmailAddress::parse(e).unwrap()),
        email_verified: verified,
        name: Some("Somebody".into()),
        avatar_url: None,
        raw: json!({}),
    }
}

#[tokio::test]
async fn an_unknown_identity_with_a_free_address_registers() {
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());

    let outcome = linking
        .sign_in(identity("google", "g-1", Some("ada@example.com"), true))
        .await
        .unwrap();
    let account = match outcome {
        SignInOutcome::Registered { account, link, .. } => {
            assert_eq!(link.provider, "google");
            assert_eq!(link.subject, "g-1");
            account
        }
        other => panic!("expected Registered, got {other:?}"),
    };
    assert!(account.email_verified, "the provider asserted the address");
    assert!(
        account.password_hash.is_none(),
        "a provider account has no password until its owner sets one"
    );

    // The same identity again signs in rather than registering twice.
    let outcome = linking
        .sign_in(identity("google", "g-1", Some("ada@example.com"), true))
        .await
        .unwrap();
    match outcome {
        SignInOutcome::SignedIn { account: a, .. } => assert_eq!(a.id, account.id),
        other => panic!("expected SignedIn, got {other:?}"),
    }
    assert_eq!(f.store.account_count(), 1);
}

#[tokio::test]
async fn an_unverified_matching_email_never_links() {
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());

    // The victim's account, with a password.
    let victim = f
        .store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse("victim@example.com").unwrap()),
            email_verified: true,
            password_hash: Some("$argon2id$pretend".into()),
        })
        .await
        .unwrap();

    // The attacker signs in at a provider that reports an address it never
    // checked — a profile field they typed themselves.
    let outcome = linking
        .sign_in(identity(
            "github",
            "attacker-subject",
            Some("victim@example.com"),
            false,
        ))
        .await
        .unwrap();

    match outcome {
        SignInOutcome::LinkRequiresAuthentication { reason, identity } => {
            assert_eq!(reason, LinkRefusal::EmailBelongsToAnotherAccount);
            assert_eq!(identity.subject, "attacker-subject");
        }
        other => panic!("an unverified matching address must never link: {other:?}"),
    }

    // Nothing was created and nothing was attached.
    assert_eq!(f.store.account_count(), 1);
    assert!(f
        .store
        .link("github", "attacker-subject")
        .await
        .unwrap()
        .is_none());
    assert!(f
        .store
        .links_for_account(&victim.id)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_verified_matching_email_does_not_link_either() {
    // Narrowing the rule to verified addresses would feel safer and is not.
    // "Verified" is verified to the provider, and the provider is chosen by
    // the person signing in.
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());
    f.store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse("victim@example.com").unwrap()),
            email_verified: true,
            password_hash: Some("$argon2id$pretend".into()),
        })
        .await
        .unwrap();

    let outcome = linking
        .sign_in(identity("google", "g-9", Some("victim@example.com"), true))
        .await
        .unwrap();
    assert!(
        matches!(outcome, SignInOutcome::LinkRequiresAuthentication { .. }),
        "{outcome:?}"
    );
    assert_eq!(f.store.account_count(), 1);
}

#[tokio::test]
async fn an_explicit_link_from_the_account_itself_succeeds() {
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());
    let ada = f
        .store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse("ada@example.com").unwrap()),
            email_verified: true,
            password_hash: Some("$argon2id$pretend".into()),
        })
        .await
        .unwrap();

    let waiting = identity("google", "g-5", Some("ada@example.com"), true);
    let refused = linking.sign_in(waiting.clone()).await.unwrap();
    assert!(matches!(
        refused,
        SignInOutcome::LinkRequiresAuthentication { .. }
    ));

    // Ada signs in with her password, and only then does the application link.
    let link = linking.link(&ada.id, &waiting).await.expect("link");
    assert_eq!(link.account, ada.id);

    // Now the provider signs her in directly.
    match linking.sign_in(waiting).await.unwrap() {
        SignInOutcome::SignedIn { account, .. } => assert_eq!(account.id, ada.id),
        other => panic!("expected SignedIn, got {other:?}"),
    }
}

#[tokio::test]
async fn an_identity_owned_by_another_account_cannot_be_linked() {
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());
    let ada = f
        .store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse("ada@example.com").unwrap()),
            ..Default::default()
        })
        .await
        .unwrap();
    let grace = f
        .store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse("grace@example.com").unwrap()),
            ..Default::default()
        })
        .await
        .unwrap();

    let shared = identity("gitlab", "gl-1", Some("ada@example.com"), true);
    linking.link(&ada.id, &shared).await.unwrap();

    let err = linking.link(&grace.id, &shared).await.unwrap_err();
    assert!(matches!(err, AuthError::AlreadyLinked), "{err:?}");
}

#[tokio::test]
async fn relinking_the_same_identity_to_the_same_account_is_fine() {
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());
    let ada = f.store.create_account(NewAccount::default()).await.unwrap();
    let id = identity("discord", "d-1", Some("ada@example.com"), true);
    linking.link(&ada.id, &id).await.unwrap();
    linking.link(&ada.id, &id).await.expect("idempotent");
    assert_eq!(linking.links(&ada.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn unlinking_the_last_way_in_is_refused() {
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());

    let registered = linking
        .sign_in(identity("slack", "s-1", Some("ada@example.com"), true))
        .await
        .unwrap();
    let account = match registered {
        SignInOutcome::Registered { account, .. } => account,
        other => panic!("{other:?}"),
    };

    let err = linking.unlink(&account.id, "slack").await.unwrap_err();
    assert!(
        err.to_string().contains("only way into this account"),
        "{err}"
    );

    // With a second provider attached it is allowed.
    linking
        .link(&account.id, &identity("github", "gh-1", None, false))
        .await
        .unwrap();
    linking.unlink(&account.id, "slack").await.expect("unlink");
    assert_eq!(linking.links(&account.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_join_key_is_the_subject_and_not_the_address() {
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());

    let registered = linking
        .sign_in(identity("google", "g-7", Some("old@example.com"), true))
        .await
        .unwrap();
    let account = match registered {
        SignInOutcome::Registered { account, .. } => account,
        other => panic!("{other:?}"),
    };

    // The person changes their address at the provider. Same subject, so the
    // same account.
    match linking
        .sign_in(identity("google", "g-7", Some("new@example.com"), true))
        .await
        .unwrap()
    {
        SignInOutcome::SignedIn { account: a, .. } => assert_eq!(a.id, account.id),
        other => panic!("a changed address must not lose the account: {other:?}"),
    }
}

#[tokio::test]
async fn a_provider_that_returns_no_address_still_registers() {
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());
    match linking
        .sign_in(identity("github", "gh-9", None, false))
        .await
        .unwrap()
    {
        SignInOutcome::Registered { account, .. } => {
            assert!(account.email.is_none());
            assert!(!account.email_verified);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_unverified_address_on_a_new_account_is_stored_unverified() {
    let f = Fixture::new();
    let linking = Linking::new(f.store.clone(), f.clock.clone());
    match linking
        .sign_in(identity(
            "microsoft",
            "m-1",
            Some("nobody@example.com"),
            false,
        ))
        .await
        .unwrap()
    {
        SignInOutcome::Registered { account, .. } => {
            assert_eq!(
                account.email.as_ref().map(|e| e.as_str()),
                Some("nobody@example.com")
            );
            assert!(
                !account.email_verified,
                "the provider asserted nothing, so neither do we"
            );
        }
        other => panic!("{other:?}"),
    }
}
