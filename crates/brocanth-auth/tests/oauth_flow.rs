//! The authorization-code flow, end to end, and every way it is supposed to
//! refuse.

mod common;

use std::sync::Arc;

use brocanth_auth::clock::TestClock;
use brocanth_auth::error::AuthError;
use brocanth_auth::http_client::{HttpResponse, StubHttp};
use brocanth_auth::oauth::{pkce, AuthorizeOptions, CallbackParams, FlowState};
use brocanth_auth::providers::catalog::{GITHUB, GOOGLE, SLACK};
use brocanth_auth::session::SigningKey;
use common::*;
use serde_json::json;

fn url_param(url: &str, name: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

#[tokio::test]
async fn the_whole_code_flow_runs_and_the_pkce_halves_match() {
    let clock = Arc::new(TestClock::default());
    let client = client_config();

    // 1. Begin. The authorization URL must carry a challenge, the S256 method,
    //    a state and a nonce.
    let stub = Arc::new(StubHttp::new());
    let oauth = oauth_client(stub.clone(), clock.clone());
    let request = oauth
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .expect("begin");

    let challenge = url_param(&request.url, "code_challenge").expect("code_challenge");
    assert_eq!(
        url_param(&request.url, "code_challenge_method").as_deref(),
        Some("S256")
    );
    assert_eq!(
        url_param(&request.url, "state").as_deref(),
        Some(request.flow.state.as_str())
    );
    assert_eq!(
        url_param(&request.url, "nonce").as_deref(),
        Some(request.flow.nonce.as_str())
    );
    assert_eq!(
        url_param(&request.url, "redirect_uri").as_deref(),
        Some("https://app.test/auth/callback")
    );
    assert_eq!(
        url_param(&request.url, "scope").as_deref(),
        Some("openid email profile")
    );

    // The verifier the flow kept is the pre-image of the challenge it sent.
    // This is the property the whole mechanism rests on.
    assert!(pkce::verify(&request.flow.pkce_verifier, &challenge));

    // 2. The provider answers with an id_token bound to this flow's nonce.
    let id_token = sign_id_token(&json!({
        "iss": "https://accounts.google.com",
        "aud": "client-id-123",
        "sub": "google-subject-1",
        "email": "ada@example.com",
        "email_verified": true,
        "name": "Ada Lovelace",
        "picture": "https://example.test/ada.png",
        "nonce": request.flow.nonce,
        "exp": real_now() + 600,
        "iat": real_now(),
    }));

    let stub = Arc::new(
        StubHttp::new()
            .on(
                http::Method::POST,
                "https://oauth2.googleapis.com/token",
                json(
                    json!({
                        "access_token": "at-1",
                        "token_type": "Bearer",
                        "expires_in": 3600,
                        "refresh_token": "rt-1",
                        "id_token": id_token,
                    })
                    .to_string(),
                ),
            )
            .on(
                http::Method::GET,
                "https://www.googleapis.com/oauth2/v3/certs",
                json(TEST_JWKS),
            ),
    );
    let oauth = oauth_client(stub.clone(), clock.clone());

    let callback = CallbackParams {
        code: "the-code".into(),
        state: request.flow.state.clone(),
        ..Default::default()
    };
    let done = oauth
        .complete(&GOOGLE, &client, &request.flow, &callback)
        .await
        .expect("complete");

    assert_eq!(done.identity.provider, "google");
    assert_eq!(done.identity.subject, "google-subject-1");
    assert_eq!(
        done.identity.email.as_ref().map(|e| e.as_str()),
        Some("ada@example.com")
    );
    assert!(done.identity.email_verified);
    assert_eq!(done.identity.name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(done.tokens.refresh_token.as_deref(), Some("rt-1"));
    assert_eq!(
        done.tokens.expires_at(clock_now(&clock)),
        Some(clock_now(&clock) + 3600)
    );

    // 3. The token request carried the verifier — not the challenge — and the
    //    verifier is the one from this flow.
    let token_request = stub
        .requests()
        .into_iter()
        .find(|r| r.method == http::Method::POST)
        .expect("a token request");
    let form = token_request.form.expect("a form body");
    let verifier = form
        .iter()
        .find(|(k, _)| k == "code_verifier")
        .map(|(_, v)| v.clone())
        .expect("code_verifier is sent");
    assert_eq!(verifier, request.flow.pkce_verifier);
    assert!(pkce::verify(&verifier, &challenge));
    assert!(form
        .iter()
        .any(|(k, v)| k == "grant_type" && v == "authorization_code"));
    assert!(form
        .iter()
        .any(|(k, v)| k == "client_secret" && v == "client-secret-456"));
}

fn clock_now(clock: &Arc<TestClock>) -> i64 {
    use brocanth_auth::clock::Clock;
    clock.now()
}

#[tokio::test]
async fn a_state_mismatch_is_rejected_before_anything_leaves_the_process() {
    let clock = Arc::new(TestClock::default());
    let stub = Arc::new(StubHttp::new());
    let oauth = oauth_client(stub.clone(), clock.clone());
    let client = client_config();
    let request = oauth
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .expect("begin");

    let callback = CallbackParams {
        code: "the-code".into(),
        state: "not-the-state".into(),
        ..Default::default()
    };
    let err = oauth
        .complete(&GOOGLE, &client, &request.flow, &callback)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::StateMismatch), "{err:?}");
    // The stub was never asked to send anything: the CSRF check comes first.
    assert!(stub.requests().is_empty());
}

#[tokio::test]
async fn a_flow_state_minted_for_one_provider_cannot_be_redeemed_at_another() {
    let clock = Arc::new(TestClock::default());
    let stub = Arc::new(StubHttp::new());
    let oauth = oauth_client(stub.clone(), clock.clone());
    let client = client_config();
    let request = oauth
        .begin(&GITHUB, &client, &AuthorizeOptions::default())
        .expect("begin");

    let callback = CallbackParams {
        code: "the-code".into(),
        state: request.flow.state.clone(),
        ..Default::default()
    };
    let err = oauth
        .complete(&GOOGLE, &client, &request.flow, &callback)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::StateMismatch), "{err:?}");
}

#[tokio::test]
async fn a_nonce_mismatch_is_rejected() {
    let clock = Arc::new(TestClock::default());
    let client = client_config();
    let begin_stub = Arc::new(StubHttp::new());
    let request = oauth_client(begin_stub, clock.clone())
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .expect("begin");

    // A correctly signed, unexpired, correctly addressed id_token — for a
    // different flow. Without the nonce check this is a successful sign-in.
    let id_token = sign_id_token(&json!({
        "iss": "https://accounts.google.com",
        "aud": "client-id-123",
        "sub": "google-subject-1",
        "email": "ada@example.com",
        "email_verified": true,
        "nonce": "a-nonce-from-somebody-elses-flow",
        "exp": real_now() + 600,
    }));
    let stub = Arc::new(
        StubHttp::new()
            .on(
                http::Method::POST,
                "https://oauth2.googleapis.com/token",
                json(json!({"access_token": "at", "id_token": id_token}).to_string()),
            )
            .on(
                http::Method::GET,
                "https://www.googleapis.com/oauth2/v3/certs",
                json(TEST_JWKS),
            ),
    );

    let err = oauth_client(stub, clock)
        .complete(
            &GOOGLE,
            &client,
            &request.flow,
            &CallbackParams {
                code: "c".into(),
                state: request.flow.state.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::NonceMismatch), "{err:?}");
}

#[tokio::test]
async fn an_id_token_with_a_missing_nonce_is_also_a_nonce_mismatch() {
    let clock = Arc::new(TestClock::default());
    let client = client_config();
    let request = oauth_client(Arc::new(StubHttp::new()), clock.clone())
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .expect("begin");

    let id_token = sign_id_token(&json!({
        "iss": "https://accounts.google.com",
        "aud": "client-id-123",
        "sub": "s",
        "exp": real_now() + 600,
    }));
    let stub = Arc::new(
        StubHttp::new()
            .on(
                http::Method::POST,
                "https://oauth2.googleapis.com/token",
                json(json!({"access_token": "at", "id_token": id_token}).to_string()),
            )
            .on(
                http::Method::GET,
                "https://www.googleapis.com/oauth2/v3/certs",
                json(TEST_JWKS),
            ),
    );
    let err = oauth_client(stub, clock)
        .complete(
            &GOOGLE,
            &client,
            &request.flow,
            &CallbackParams {
                code: "c".into(),
                state: request.flow.state.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::NonceMismatch), "{err:?}");
}

#[tokio::test]
async fn an_id_token_signed_with_a_symmetric_algorithm_is_refused() {
    // The alg-confusion attack: take the provider's public JWKS, sign an
    // id_token with HS256, and hand it to a verifier that reads `alg` out of
    // the token it is checking.
    let clock = Arc::new(TestClock::default());
    let client = client_config();
    let request = oauth_client(Arc::new(StubHttp::new()), clock.clone())
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .expect("begin");

    let forged = sign_hs256(&json!({
        "iss": "https://accounts.google.com",
        "aud": "client-id-123",
        "sub": "attacker",
        "email": "victim@example.com",
        "email_verified": true,
        "nonce": request.flow.nonce,
        "exp": real_now() + 600,
    }));
    let stub = Arc::new(
        StubHttp::new()
            .on(
                http::Method::POST,
                "https://oauth2.googleapis.com/token",
                json(json!({"access_token": "at", "id_token": forged}).to_string()),
            )
            .on(
                http::Method::GET,
                "https://www.googleapis.com/oauth2/v3/certs",
                json(TEST_JWKS),
            ),
    );
    let err = oauth_client(stub, clock)
        .complete(
            &GOOGLE,
            &client,
            &request.flow,
            &CallbackParams {
                code: "c".into(),
                state: request.flow.state.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("public-key algorithm"), "{message}");
}

#[tokio::test]
async fn an_id_token_for_another_client_is_refused() {
    let clock = Arc::new(TestClock::default());
    let client = client_config();
    let request = oauth_client(Arc::new(StubHttp::new()), clock.clone())
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .expect("begin");

    let id_token = sign_id_token(&json!({
        "iss": "https://accounts.google.com",
        "aud": "somebody-elses-client-id",
        "sub": "s",
        "nonce": request.flow.nonce,
        "exp": real_now() + 600,
    }));
    let stub = Arc::new(
        StubHttp::new()
            .on(
                http::Method::POST,
                "https://oauth2.googleapis.com/token",
                json(json!({"access_token": "at", "id_token": id_token}).to_string()),
            )
            .on(
                http::Method::GET,
                "https://www.googleapis.com/oauth2/v3/certs",
                json(TEST_JWKS),
            ),
    );
    let err = oauth_client(stub, clock)
        .complete(
            &GOOGLE,
            &client,
            &request.flow,
            &CallbackParams {
                code: "c".into(),
                state: request.flow.state.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("did not verify"), "{err}");
}

#[tokio::test]
async fn a_flow_that_takes_too_long_expires() {
    let clock = Arc::new(TestClock::default());
    let stub = Arc::new(StubHttp::new());
    let oauth = oauth_client(stub, clock.clone());
    let client = client_config();
    let request = oauth
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .expect("begin");

    clock.advance(601);
    let err = oauth
        .complete(
            &GOOGLE,
            &client,
            &request.flow,
            &CallbackParams {
                code: "c".into(),
                state: request.flow.state.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::FlowExpired), "{err:?}");
}

#[tokio::test]
async fn a_provider_that_refuses_is_reported_as_a_refusal() {
    let clock = Arc::new(TestClock::default());
    let oauth = oauth_client(Arc::new(StubHttp::new()), clock.clone());
    let client = client_config();
    let request = oauth
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .expect("begin");

    let err = oauth
        .complete(
            &GOOGLE,
            &client,
            &request.flow,
            &CallbackParams {
                state: request.flow.state.clone(),
                error: Some("access_denied".into()),
                error_description: Some("the user said no".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AuthError::ProviderRefused { error, .. } if error == "access_denied"),
        "{err:?}"
    );
}

#[tokio::test]
async fn github_takes_the_verified_address_and_not_the_profile_one() {
    let clock = Arc::new(TestClock::default());
    let client = client_config();
    let request = oauth_client(Arc::new(StubHttp::new()), clock.clone())
        .begin(&GITHUB, &client, &AuthorizeOptions::default())
        .expect("begin");
    // GitHub has no id_token, so there is no nonce to carry.
    assert!(request.flow.nonce.is_empty());

    let stub = Arc::new(
        StubHttp::new()
            .on(
                http::Method::POST,
                "https://github.com/login/oauth/access_token",
                json(json!({"access_token": "gho_x", "token_type": "bearer"}).to_string()),
            )
            .on(
                http::Method::GET,
                "https://api.github.com/user/emails",
                json(
                    json!([
                        {"email": "public@example.com", "primary": false, "verified": false},
                        {"email": "real@example.com", "primary": true, "verified": true},
                    ])
                    .to_string(),
                ),
            )
            .on(
                http::Method::GET,
                "https://api.github.com/user",
                json(
                    json!({
                        "id": 4242,
                        "login": "octocat",
                        "name": "The Octocat",
                        // The address an attacker types into their profile.
                        "email": "victim@example.com",
                        "avatar_url": "https://example.test/o.png",
                    })
                    .to_string(),
                ),
            ),
    );
    let done = oauth_client(stub, clock)
        .complete(
            &GITHUB,
            &client,
            &request.flow,
            &CallbackParams {
                code: "c".into(),
                state: request.flow.state.clone(),
                ..Default::default()
            },
        )
        .await
        .expect("complete");

    assert_eq!(done.identity.subject, "4242");
    assert_eq!(
        done.identity.email.as_ref().map(|e| e.as_str()),
        Some("real@example.com"),
        "the profile email must never be used"
    );
    assert!(done.identity.email_verified);
}

#[tokio::test]
async fn slacks_two_hundred_with_ok_false_is_a_refusal_and_not_a_success() {
    let clock = Arc::new(TestClock::default());
    let client = client_config();
    let request = oauth_client(Arc::new(StubHttp::new()), clock.clone())
        .begin(&SLACK, &client, &AuthorizeOptions::default())
        .expect("begin");

    let stub = Arc::new(StubHttp::new().on(
        http::Method::POST,
        "https://slack.com/api/openid.connect.token",
        HttpResponse::new(
            http::StatusCode::OK,
            json!({"ok": false, "error": "invalid_code"}).to_string(),
        ),
    ));
    let err = oauth_client(stub, clock)
        .complete(
            &SLACK,
            &client,
            &request.flow,
            &CallbackParams {
                code: "c".into(),
                state: request.flow.state.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, AuthError::ProviderRefused { error, .. } if error == "invalid_code"),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_refresh_sends_the_refresh_grant_and_returns_the_rotated_token() {
    let clock = Arc::new(TestClock::default());
    let stub = Arc::new(StubHttp::new().on(
        http::Method::POST,
        "https://oauth2.googleapis.com/token",
        json(
            json!({"access_token": "at-2", "refresh_token": "rt-2", "expires_in": 60}).to_string(),
        ),
    ));
    let oauth = oauth_client(stub.clone(), clock);
    let tokens = oauth
        .refresh(&GOOGLE, &client_config(), "rt-1")
        .await
        .expect("refresh");
    assert_eq!(tokens.access_token, "at-2");
    assert_eq!(tokens.refresh_token.as_deref(), Some("rt-2"));

    let form = stub.requests()[0].form.clone().expect("form");
    assert!(form
        .iter()
        .any(|(k, v)| k == "grant_type" && v == "refresh_token"));
    assert!(form
        .iter()
        .any(|(k, v)| k == "refresh_token" && v == "rt-1"));
}

#[test]
fn a_sealed_flow_state_round_trips_and_a_tampered_one_does_not() {
    let key = SigningKey::from_bytes([3u8; 32]);
    let clock = Arc::new(TestClock::default());
    let request = oauth_client(Arc::new(StubHttp::new()), clock)
        .begin(&GOOGLE, &client_config(), &AuthorizeOptions::default())
        .expect("begin");

    let sealed = request.flow.seal(&key).expect("seal");
    let recovered = FlowState::unseal(&key, &sealed).expect("unseal");
    assert_eq!(recovered.state, request.flow.state);
    assert_eq!(recovered.pkce_verifier, request.flow.pkce_verifier);

    // One flipped character in the payload, and the signature no longer holds.
    let (payload, signature) = sealed.rsplit_once('.').unwrap();
    let mut mangled = payload.to_string();
    mangled.replace_range(0..1, if payload.starts_with('a') { "b" } else { "a" });
    let err = FlowState::unseal(&key, &format!("{mangled}.{signature}")).unwrap_err();
    assert!(matches!(err, AuthError::StateMismatch), "{err:?}");

    // A different key does not open it either.
    let err = FlowState::unseal(&SigningKey::from_bytes([4u8; 32]), &sealed).unwrap_err();
    assert!(matches!(err, AuthError::StateMismatch), "{err:?}");
}

#[test]
fn every_begin_produces_a_fresh_state_nonce_and_verifier() {
    let clock = Arc::new(TestClock::default());
    let oauth = oauth_client(Arc::new(StubHttp::new()), clock);
    let client = client_config();
    let a = oauth
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .unwrap();
    let b = oauth
        .begin(&GOOGLE, &client, &AuthorizeOptions::default())
        .unwrap();
    assert_ne!(a.flow.state, b.flow.state);
    assert_ne!(a.flow.nonce, b.flow.nonce);
    assert_ne!(a.flow.pkce_verifier, b.flow.pkce_verifier);
}
