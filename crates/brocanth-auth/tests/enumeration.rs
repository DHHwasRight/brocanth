//! Account enumeration, in both channels: the response and the clock.
//!
//! The timing assertions compare medians rather than means, because a test
//! machine running six other agents produces outliers that no auth library can
//! do anything about. What they are really asserting is that both paths do the
//! *same work* — one argon2 hash, the same store calls — so the difference
//! between them stays inside the noise instead of growing with it.

mod common;

use std::time::{Duration, Instant};

use brocanth_auth::accounts::{Accounts, RegistrationNotice, ResetOutcome};
use brocanth_auth::error::AuthError;
use brocanth_auth::password::{PasswordCost, Passwords};
use common::Fixture;

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn ratio(a: Duration, b: Duration) -> f64 {
    a.as_secs_f64() / b.as_secs_f64().max(f64::EPSILON)
}

// ---------------------------------------------------------------------------
// The response channel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_gives_one_answer_for_four_different_reasons() {
    let f = Fixture::new();
    let accounts =
        Accounts::new(f.store.clone(), f.clock.clone(), f.passwords.clone()).unwrap();
    accounts
        .register("ada@example.com", "the right password")
        .await
        .unwrap();

    // An account that only signs in through a provider: no password at all.
    let linking = brocanth_auth::linking::Linking::new(f.store.clone(), f.clock.clone());
    linking
        .sign_in(brocanth_auth::identity::ProviderIdentity {
            provider: "google".into(),
            subject: "g-1".into(),
            email: Some(
                brocanth_auth::identity::EmailAddress::parse("grace@example.com").unwrap(),
            ),
            email_verified: true,
            name: None,
            avatar_url: None,
            raw: serde_json::Value::Null,
        })
        .await
        .unwrap();

    let cases = [
        ("ada@example.com", "the wrong password"),
        ("nobody@example.com", "any password"),
        ("not an address at all", "any password"),
        ("grace@example.com", "any password"),
    ];
    for (email, password) in cases {
        let err = accounts.login(email, password).await.unwrap_err();
        assert!(
            matches!(err, AuthError::InvalidCredentials),
            "`{email}` answered {err:?}"
        );
        assert_eq!(err.to_string(), "invalid credentials");
    }

    // And the right password still works.
    let ada = accounts
        .login("ada@example.com", "the right password")
        .await
        .expect("the real credentials");
    assert_eq!(ada.email.unwrap().as_str(), "ada@example.com");
}

#[tokio::test]
async fn registration_succeeds_whether_or_not_the_address_is_taken() {
    let f = Fixture::new();
    let accounts =
        Accounts::new(f.store.clone(), f.clock.clone(), f.passwords.clone()).unwrap();

    let first = accounts
        .register("ada@example.com", "a long enough password")
        .await
        .expect("a free address");
    assert!(matches!(first.notice, RegistrationNotice::Created { .. }));

    // The attacker tries the same address. Same `Ok`, same type, same shape.
    let second = accounts
        .register("ada@example.com", "a different password")
        .await
        .expect("a taken address must not fail");
    assert!(matches!(
        second.notice,
        RegistrationNotice::AlreadyRegistered { .. }
    ));

    // The second attempt created nothing and did not change the password.
    assert_eq!(f.store.account_count(), 1);
    accounts
        .login("ada@example.com", "a long enough password")
        .await
        .expect("the original password still works");
}

#[tokio::test]
async fn a_reset_request_succeeds_for_an_address_nobody_has() {
    let f = Fixture::new();
    let accounts =
        Accounts::new(f.store.clone(), f.clock.clone(), f.passwords.clone()).unwrap();
    accounts
        .register("ada@example.com", "a long enough password")
        .await
        .unwrap();

    let known = accounts
        .request_password_reset("ada@example.com")
        .await
        .expect("known");
    let unknown = accounts
        .request_password_reset("nobody@example.com")
        .await
        .expect("unknown must also succeed");

    assert!(matches!(known.outcome, ResetOutcome::Issue { .. }));
    assert!(matches!(unknown.outcome, ResetOutcome::Nothing));
}

// ---------------------------------------------------------------------------
// The timing channel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_takes_comparable_time_for_a_known_and_an_unknown_address() {
    let f = Fixture::with_measurable_cost();
    let accounts =
        Accounts::new(f.store.clone(), f.clock.clone(), f.passwords.clone()).unwrap();
    accounts
        .register("ada@example.com", "the right password")
        .await
        .unwrap();

    let samples = 9;
    let mut known = Vec::with_capacity(samples);
    let mut unknown = Vec::with_capacity(samples);
    for _ in 0..samples {
        let t = Instant::now();
        let _ = accounts.login("ada@example.com", "the wrong password").await;
        known.push(t.elapsed());

        let t = Instant::now();
        let _ = accounts.login("nobody@example.com", "the wrong password").await;
        unknown.push(t.elapsed());
    }

    let (k, u) = (median(known), median(unknown));
    let r = ratio(k, u);
    // Without the dummy hash this ratio is in the hundreds: the unknown path
    // would return as soon as the lookup missed.
    assert!(
        (0.5..=2.0).contains(&r),
        "a known address took {k:?} and an unknown one {u:?} (ratio {r:.2}); \
         the lookup miss is leaking through the clock"
    );
}

#[tokio::test]
async fn a_reset_request_takes_comparable_time_either_way() {
    let f = Fixture::new();
    let accounts =
        Accounts::new(f.store.clone(), f.clock.clone(), f.passwords.clone()).unwrap();
    accounts
        .register("ada@example.com", "a long enough password")
        .await
        .unwrap();

    let samples = 41;
    let mut known = Vec::with_capacity(samples);
    let mut unknown = Vec::with_capacity(samples);
    for _ in 0..samples {
        let t = Instant::now();
        let _ = accounts.request_password_reset("ada@example.com").await;
        known.push(t.elapsed());

        let t = Instant::now();
        let _ = accounts.request_password_reset("nobody@example.com").await;
        unknown.push(t.elapsed());
    }

    let (k, u) = (median(known), median(unknown));
    let r = ratio(k, u);
    // Both paths mint and store a token — the unknown one against the
    // sentinel — so they issue the same queries and cost the same.
    assert!(
        (0.25..=4.0).contains(&r),
        "a known address took {k:?} and an unknown one {u:?} (ratio {r:.2})"
    );
}

#[tokio::test]
async fn registration_takes_comparable_time_for_a_free_and_a_taken_address() {
    let f = Fixture::with_measurable_cost();
    let accounts =
        Accounts::new(f.store.clone(), f.clock.clone(), f.passwords.clone()).unwrap();
    accounts.register("taken@example.com", "pw").await.unwrap();

    let samples = 9;
    let mut free = Vec::with_capacity(samples);
    let mut taken = Vec::with_capacity(samples);
    for i in 0..samples {
        let address = format!("free{i}@example.com");
        let t = Instant::now();
        let _ = accounts.register(&address, "pw").await;
        free.push(t.elapsed());

        let t = Instant::now();
        let _ = accounts.register("taken@example.com", "pw").await;
        taken.push(t.elapsed());
    }

    let (fr, tk) = (median(free), median(taken));
    let r = ratio(fr, tk);
    assert!(
        (0.5..=2.0).contains(&r),
        "a free address took {fr:?} and a taken one {tk:?} (ratio {r:.2}); \
         hashing after the lookup instead of before would show up here"
    );
}

// ---------------------------------------------------------------------------
// The hasher itself
// ---------------------------------------------------------------------------

#[test]
fn password_verification_does_not_depend_on_where_the_password_goes_wrong() {
    // argon2 computes the whole digest before comparing, and `password-hash`
    // compares the two digests with `subtle::ConstantTimeEq`. So a password
    // that is wrong in its first byte must cost the same as one that is wrong
    // in its last, and both the same as the correct one.
    let passwords = Passwords::new(PasswordCost::custom(32 * 1024, 1, 1).unwrap()).unwrap();
    let correct = "correct horse battery staple";
    let hash = passwords.hash(correct).unwrap();

    let samples = 9;
    let mut right = Vec::new();
    let mut wrong_first = Vec::new();
    let mut wrong_last = Vec::new();
    for _ in 0..samples {
        let t = Instant::now();
        assert!(passwords.verify(&hash, correct));
        right.push(t.elapsed());

        let t = Instant::now();
        assert!(!passwords.verify(&hash, "xorrect horse battery staple"));
        wrong_first.push(t.elapsed());

        let t = Instant::now();
        assert!(!passwords.verify(&hash, "correct horse battery staplx"));
        wrong_last.push(t.elapsed());
    }

    let (r, wf, wl) = (median(right), median(wrong_first), median(wrong_last));
    for (name, d) in [("wrong in the first byte", wf), ("wrong in the last byte", wl)] {
        let ratio = ratio(d, r);
        assert!(
            (0.5..=2.0).contains(&ratio),
            "a correct password took {r:?} and one {name} took {d:?} (ratio {ratio:.2})"
        );
    }
}

#[test]
fn verifying_against_no_hash_costs_the_same_as_verifying_against_one() {
    let passwords = Passwords::new(PasswordCost::custom(32 * 1024, 1, 1).unwrap()).unwrap();
    let hash = passwords.hash("a password").unwrap();

    let samples = 9;
    let mut with = Vec::new();
    let mut without = Vec::new();
    for _ in 0..samples {
        let t = Instant::now();
        assert!(!passwords.verify_or_dummy(Some(&hash), "the wrong one"));
        with.push(t.elapsed());

        let t = Instant::now();
        assert!(!passwords.verify_or_dummy(None, "the wrong one"));
        without.push(t.elapsed());
    }

    let (w, wo) = (median(with), median(without));
    let r = ratio(w, wo);
    assert!(
        (0.5..=2.0).contains(&r),
        "an existing hash took {w:?} and a missing one {wo:?} (ratio {r:.2})"
    );
}
