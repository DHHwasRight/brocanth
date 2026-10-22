//! Single-use tokens: spent once, expiring, and not interchangeable between
//! purposes.

mod common;

use std::sync::Arc;

use brocanth_auth::error::AuthError;
use brocanth_auth::identity::{AccountId, EmailAddress};
use brocanth_auth::storage::{AuthStore, NewAccount};
use brocanth_auth::tokens::{TokenPurpose, Tokens};
use common::Fixture;

async fn an_account(f: &Fixture) -> AccountId {
    f.store
        .create_account(NewAccount {
            email: Some(EmailAddress::parse("ada@example.com").unwrap()),
            email_verified: false,
            password_hash: None,
        })
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn a_token_works_exactly_once() {
    let f = Fixture::new();
    let tokens = Tokens::new(f.store.clone(), f.clock.clone());
    let account = an_account(&f).await;

    let issued = tokens
        .issue(TokenPurpose::PasswordReset, &account)
        .await
        .unwrap();

    let spent = tokens
        .consume(TokenPurpose::PasswordReset, issued.secret())
        .await
        .expect("the first use");
    assert_eq!(spent, account);

    let err = tokens
        .consume(TokenPurpose::PasswordReset, issued.secret())
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidToken), "{err:?}");
}

#[tokio::test]
async fn an_expired_token_is_rejected() {
    let f = Fixture::new();
    let tokens = Tokens::new(f.store.clone(), f.clock.clone());
    let account = an_account(&f).await;

    let issued = tokens
        .issue(TokenPurpose::PasswordReset, &account)
        .await
        .unwrap();

    // One second short of the lifetime: still good.
    f.clock.advance(tokens.ttl(TokenPurpose::PasswordReset) - 1);
    let reissued = {
        let spent = tokens
            .consume(TokenPurpose::PasswordReset, issued.secret())
            .await
            .expect("just inside");
        assert_eq!(spent, account);
        tokens
            .issue(TokenPurpose::PasswordReset, &account)
            .await
            .unwrap()
    };

    // Exactly at the lifetime: gone.
    f.clock.advance(tokens.ttl(TokenPurpose::PasswordReset));
    let err = tokens
        .consume(TokenPurpose::PasswordReset, reissued.secret())
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidToken), "{err:?}");
}

#[tokio::test]
async fn an_expired_token_is_consumed_rather_than_left_for_a_wrong_clock() {
    let f = Fixture::new();
    let tokens = Tokens::new(f.store.clone(), f.clock.clone());
    let account = an_account(&f).await;
    let issued = tokens
        .issue(TokenPurpose::PasswordReset, &account)
        .await
        .unwrap();

    f.clock.advance(tokens.ttl(TokenPurpose::PasswordReset) + 1);
    assert!(tokens
        .consume(TokenPurpose::PasswordReset, issued.secret())
        .await
        .is_err());

    // Put the clock back. The row is gone, so the token stays dead.
    f.clock
        .advance(-(tokens.ttl(TokenPurpose::PasswordReset) + 1));
    assert!(tokens
        .consume(TokenPurpose::PasswordReset, issued.secret())
        .await
        .is_err());
}

#[tokio::test]
async fn a_verification_token_cannot_be_spent_as_a_reset_token() {
    let f = Fixture::new();
    let tokens = Tokens::new(f.store.clone(), f.clock.clone());
    let account = an_account(&f).await;

    let verification = tokens
        .issue(TokenPurpose::EmailVerification, &account)
        .await
        .unwrap();

    // Without the purpose in the lookup key, a verification link mailed to an
    // address nobody has proved is a password reset.
    let err = tokens
        .consume(TokenPurpose::PasswordReset, verification.secret())
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidToken), "{err:?}");

    // It still works for what it was minted for.
    assert_eq!(
        tokens
            .consume(TokenPurpose::EmailVerification, verification.secret())
            .await
            .unwrap(),
        account
    );
}

#[tokio::test]
async fn issuing_a_new_token_kills_the_previous_one() {
    let f = Fixture::new();
    let tokens = Tokens::new(f.store.clone(), f.clock.clone());
    let account = an_account(&f).await;

    let first = tokens
        .issue(TokenPurpose::PasswordReset, &account)
        .await
        .unwrap();
    let second = tokens
        .issue(TokenPurpose::PasswordReset, &account)
        .await
        .unwrap();

    assert!(tokens
        .consume(TokenPurpose::PasswordReset, first.secret())
        .await
        .is_err());
    assert_eq!(
        tokens
            .consume(TokenPurpose::PasswordReset, second.secret())
            .await
            .unwrap(),
        account
    );
}

#[tokio::test]
async fn the_right_selector_with_the_wrong_verifier_is_rejected_and_spends_the_token() {
    let f = Fixture::new();
    let tokens = Tokens::new(f.store.clone(), f.clock.clone());
    let account = an_account(&f).await;

    let issued = tokens
        .issue(TokenPurpose::PasswordReset, &account)
        .await
        .unwrap();
    let (selector, _) = issued.secret().split_once('.').unwrap();
    let forged = format!("{selector}.wrong-verifier");

    let err = tokens
        .consume(TokenPurpose::PasswordReset, &forged)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidToken), "{err:?}");

    // Deleted before it was checked, so a guessing loop gets one attempt per
    // issued token rather than unlimited attempts.
    assert!(tokens
        .consume(TokenPurpose::PasswordReset, issued.secret())
        .await
        .is_err());
}

#[tokio::test]
async fn a_malformed_token_is_rejected_without_touching_the_store() {
    let f = Fixture::new();
    let tokens = Tokens::new(f.store.clone(), f.clock.clone());
    for bad in ["", "no-separator", ".", "a."] {
        let err = tokens
            .consume(TokenPurpose::PasswordReset, bad)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidToken), "`{bad}` -> {err:?}");
    }
}

#[tokio::test]
async fn two_tokens_never_collide() {
    let f = Fixture::new();
    let tokens = Arc::new(Tokens::new(f.store.clone(), f.clock.clone()));
    let account = an_account(&f).await;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..64 {
        let issued = tokens
            .issue(TokenPurpose::EmailVerification, &account)
            .await
            .unwrap();
        assert!(seen.insert(issued.secret().to_string()), "a token repeated");
    }
}
