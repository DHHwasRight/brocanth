//! One-time codes: the attempt cap, single use, expiry, and the enumeration
//! line the rest of the crate holds.

mod common;

use std::time::{Duration, Instant};

use brocanth_auth::error::AuthError;
use brocanth_auth::otp::{OtpChallengeId, OtpCode, OtpDelivery, OtpRequested};
use common::Fixture;

fn median(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn ratio(a: Duration, b: Duration) -> f64 {
    a.as_secs_f64() / b.as_secs_f64().max(f64::EPSILON)
}

/// The code a mailer would have sent, for a test that has to know it.
fn sent(requested: &OtpRequested) -> (OtpChallengeId, OtpCode) {
    match &requested.delivery {
        OtpDelivery::Send { code, .. } => (requested.challenge.clone(), code.clone()),
        OtpDelivery::Nothing => panic!("nothing was addressed"),
    }
}

async fn registered(f: &Fixture) {
    f.accounts()
        .register("ada@example.com", "a long enough password")
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// The code itself
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_code_is_eight_digits_and_works_once() {
    let f = Fixture::new();
    registered(&f).await;
    let otp = f.otp();

    let requested = otp.request("ada@example.com").await.unwrap();
    let (challenge, code) = sent(&requested);
    assert_eq!(code.as_str().len(), 8);
    assert!(code.as_str().bytes().all(|b| b.is_ascii_digit()));

    let proof = otp
        .authenticate(&challenge, code.as_str())
        .await
        .expect("the first use");
    assert!(!proof.account().as_str().is_empty());
    // A mailed code is a *second* factor here, not a sign-in on its own.
    assert_eq!(
        proof.kinds(),
        brocanth_auth::mfa::FactorSet::of([brocanth_auth::mfa::FactorKind::EmailOtp])
    );

    let err = otp
        .authenticate(&challenge, code.as_str())
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
}

#[tokio::test]
async fn an_expired_challenge_is_refused_even_with_the_right_code() {
    let f = Fixture::new();
    registered(&f).await;
    let otp = f.otp();
    let requested = otp.request("ada@example.com").await.unwrap();
    let (challenge, code) = sent(&requested);

    f.clock.advance(otp.ttl() - 1);
    // Still inside: a fresh challenge proves the clock is what moved.
    let fresh = otp.request("ada@example.com").await.unwrap();
    let (fresh_challenge, fresh_code) = sent(&fresh);
    assert!(otp
        .authenticate(&fresh_challenge, fresh_code.as_str())
        .await
        .is_ok());

    f.clock.advance(1);
    let err = otp
        .authenticate(&challenge, code.as_str())
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
}

// ---------------------------------------------------------------------------
// The attempt cap, which is half the arithmetic in the module documentation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn five_wrong_guesses_destroy_the_challenge() {
    let f = Fixture::new();
    registered(&f).await;
    let otp = f.otp();
    assert_eq!(otp.max_attempts(), 5);

    let requested = otp.request("ada@example.com").await.unwrap();
    let (challenge, code) = sent(&requested);

    for guess in 0..5 {
        let err = otp.authenticate(&challenge, "00000000").await.unwrap_err();
        assert!(
            matches!(err, AuthError::InvalidCredentials),
            "guess {guess} answered {err:?}"
        );
    }

    // The correct code, after the budget is spent. Without the cap this loop
    // runs 10^8 times and the code space stops mattering.
    let err = otp
        .authenticate(&challenge, code.as_str())
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
    assert_eq!(f.store.otp_challenge_count(), 0, "the row outlived its cap");
}

#[tokio::test]
async fn the_last_attempt_in_the_budget_still_counts() {
    let f = Fixture::new();
    registered(&f).await;
    let otp = f.otp();
    let requested = otp.request("ada@example.com").await.unwrap();
    let (challenge, code) = sent(&requested);

    for _ in 0..4 {
        assert!(otp.authenticate(&challenge, "00000000").await.is_err());
    }
    // Four typos and then the right code is a real person, not an attack.
    let proof = otp
        .authenticate(&challenge, code.as_str())
        .await
        .expect("the fifth attempt is still an attempt");
    assert!(!proof.account().as_str().is_empty());
}

#[tokio::test]
async fn an_attempt_against_one_challenge_does_not_spend_another() {
    let f = Fixture::new();
    registered(&f).await;
    let otp = f.otp();

    let first = otp.request("ada@example.com").await.unwrap();
    let second = otp.request("ada@example.com").await.unwrap();
    let (first_challenge, _) = sent(&first);
    let (second_challenge, second_code) = sent(&second);

    for _ in 0..5 {
        assert!(otp
            .authenticate(&first_challenge, "00000000")
            .await
            .is_err());
    }
    // Asking twice does not invalidate the code somebody is part-way through
    // typing, and burning one challenge does not burn the other.
    let proof = otp
        .authenticate(&second_challenge, second_code.as_str())
        .await
        .expect("the second challenge is its own budget");
    assert!(!proof.account().as_str().is_empty());
}

// ---------------------------------------------------------------------------
// Enumeration: the response shape, and the clock
// ---------------------------------------------------------------------------

#[tokio::test]
async fn requesting_a_code_succeeds_for_an_address_nobody_has() {
    let f = Fixture::new();
    registered(&f).await;
    let otp = f.otp();

    let known = otp.request("ada@example.com").await.expect("known");
    let unknown = otp.request("nobody@example.com").await.expect("unknown");
    let malformed = otp.request("not an address").await.expect("malformed");

    assert!(matches!(known.delivery, OtpDelivery::Send { .. }));
    assert!(matches!(unknown.delivery, OtpDelivery::Nothing));
    assert!(matches!(malformed.delivery, OtpDelivery::Nothing));

    // The part that reaches the browser is indistinguishable: same type, same
    // shape, same length of identifier.
    assert_eq!(
        known.challenge.as_str().len(),
        unknown.challenge.as_str().len()
    );
    assert_eq!(
        unknown.challenge.as_str().len(),
        malformed.challenge.as_str().len()
    );
    assert_ne!(known.challenge, unknown.challenge);

    // And the sentinel's challenge really is a challenge: it accepts guesses
    // and answers exactly what a real one answers.
    let err = otp
        .authenticate(&unknown.challenge, "00000000")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
    assert_eq!(err.to_string(), "invalid credentials");
}

#[tokio::test]
async fn an_unknown_challenge_answers_what_a_wrong_code_answers() {
    let f = Fixture::new();
    let otp = f.otp();
    let err = otp
        .authenticate(&OtpChallengeId::new("never-existed"), "00000000")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials), "{err:?}");
}

#[tokio::test]
async fn requesting_a_code_takes_comparable_time_either_way() {
    // Both paths mint a code, pay one argon2id hash, and write one challenge
    // row — the unknown one against the sentinel. Without the sentinel the
    // unknown path returns as soon as the lookup misses and this ratio runs
    // into the hundreds.
    let f = Fixture::with_measurable_cost();
    f.accounts()
        .register("ada@example.com", "a long enough password")
        .await
        .unwrap();
    let otp = f.otp();

    let samples = 9;
    let mut known = Vec::with_capacity(samples);
    let mut unknown = Vec::with_capacity(samples);
    for _ in 0..samples {
        let t = Instant::now();
        let _ = otp.request("ada@example.com").await;
        known.push(t.elapsed());

        let t = Instant::now();
        let _ = otp.request("nobody@example.com").await;
        unknown.push(t.elapsed());
    }

    let (k, u) = (median(known), median(unknown));
    let r = ratio(k, u);
    assert!(
        (0.5..=2.0).contains(&r),
        "a known address took {k:?} and an unknown one {u:?} (ratio {r:.2}); \
         the lookup miss is leaking through the clock"
    );
}

#[tokio::test]
async fn guessing_against_a_sentinel_challenge_costs_what_a_real_one_costs() {
    let f = Fixture::with_measurable_cost();
    f.accounts()
        .register("ada@example.com", "a long enough password")
        .await
        .unwrap();
    let otp = f.otp();

    let samples = 9;
    let mut real = Vec::with_capacity(samples);
    let mut sentinel = Vec::with_capacity(samples);
    for _ in 0..samples {
        let known = otp.request("ada@example.com").await.unwrap();
        let unknown = otp.request("nobody@example.com").await.unwrap();

        let t = Instant::now();
        let _ = otp.authenticate(&known.challenge, "00000000").await;
        real.push(t.elapsed());

        let t = Instant::now();
        let _ = otp.authenticate(&unknown.challenge, "00000000").await;
        sentinel.push(t.elapsed());
    }

    let (r, s) = (median(real), median(sentinel));
    let q = ratio(r, s);
    assert!(
        (0.5..=2.0).contains(&q),
        "a real challenge took {r:?} and a sentinel one {s:?} (ratio {q:.2})"
    );
}

#[tokio::test]
async fn a_sweeper_removes_expired_challenges_and_nothing_else() {
    let f = Fixture::new();
    registered(&f).await;
    let otp = f.otp();

    otp.request("ada@example.com").await.unwrap();
    otp.request("nobody@example.com").await.unwrap();
    assert_eq!(f.store.otp_challenge_count(), 2);

    assert_eq!(otp.purge_expired().await.unwrap(), 0);
    f.clock.advance(otp.ttl() + 1);
    assert_eq!(otp.purge_expired().await.unwrap(), 2);
    assert_eq!(f.store.otp_challenge_count(), 0);
}
