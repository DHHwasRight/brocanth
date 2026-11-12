//! The OIDC flow against a provider on a local port, through the real
//! `ReqwestClient`. No network beyond loopback.
//!
//! The provider checks what a real one checks: the client credentials, the
//! redirect URI, and the PKCE verifier against the challenge the code was
//! issued for. It signs its id_tokens with the test key and serves the
//! matching key set. Each test changes one thing and expects one refusal.

mod common;

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use brocanth_auth::clock::{Clock, TestClock};
use brocanth_auth::error::AuthError;
use brocanth_auth::http_client::ReqwestClient;
use brocanth_auth::oauth::{
    AuthorizationRequest, AuthorizeOptions, CallbackParams, ClientConfig, OauthClient,
};
use brocanth_auth::providers::catalog::GOOGLE;
use brocanth_auth::providers::{IssuerCheck, Provider, ProviderKind};
use common::{pkce_matches, sign_id_token, TEST_JWKS};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const CLIENT_ID: &str = "mock-client";
const SECRET: &str = "mock-secret";
const REDIRECT: &str = "https://app.test/oauth/mock/callback";

/// What the person consented to: the challenge the code is bound to, and
/// the id_token the provider will sign for it.
struct Grant {
    challenge: String,
    claims: Value,
    bad_signature: bool,
}

struct Mock {
    url: String,
    grants: Mutex<HashMap<String, Grant>>,
    token_requests: AtomicUsize,
}

impl Mock {
    async fn start() -> Arc<Mock> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock = Arc::new(Mock {
            url: format!("http://{}", listener.local_addr().unwrap()),
            grants: Mutex::new(HashMap::new()),
            token_requests: AtomicUsize::new(0),
        });
        let served = Arc::clone(&mock);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let mock = Arc::clone(&served);
                tokio::spawn(async move {
                    let Some((head, body)) = read_request(&mut socket).await else {
                        return;
                    };
                    let (status, text) = mock.answer(&head, &body);
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                        text.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        mock
    }

    fn provider(&self) -> Provider {
        Provider {
            id: Cow::Borrowed("mock"),
            display_name: Cow::Borrowed("Mock"),
            kind: ProviderKind::Oidc {
                issuer: IssuerCheck::Exact(Cow::Owned(self.url.clone())),
                jwks_url: Cow::Owned(format!("{}/jwks", self.url)),
            },
            authorize_url: Cow::Owned(format!("{}/authorize", self.url)),
            token_url: Cow::Owned(format!("{}/token", self.url)),
            userinfo_url: None,
            ..GOOGLE
        }
    }

    /// The claims a correct id_token for this flow carries.
    fn claims(&self, flow: &AuthorizationRequest, now: i64) -> Value {
        json!({
            "iss": self.url,
            "aud": CLIENT_ID,
            "sub": "mock-subject-1",
            "email": "ada@example.com",
            "email_verified": true,
            "name": "Ada Lovelace",
            "nonce": flow.flow.nonce,
            "iat": now,
            "exp": now + 600,
        })
    }

    /// The person consents: `code` is issued for the challenge on `url`.
    fn consent(&self, code: &str, url: &str, claims: Value, bad_signature: bool) {
        self.grants.lock().unwrap().insert(
            code.to_string(),
            Grant {
                challenge: param(url, "code_challenge"),
                claims,
                bad_signature,
            },
        );
    }

    fn answer(&self, head: &str, body: &str) -> (u16, String) {
        let line = head.lines().next().unwrap_or_default();
        if line.starts_with("GET /jwks") {
            return (200, TEST_JWKS.to_string());
        }
        if !line.starts_with("POST /token") {
            return (404, "{}".to_string());
        }
        self.token_requests.fetch_add(1, Ordering::SeqCst);
        let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
        let field = |name: &str| form.get(name).map(String::as_str).unwrap_or("");
        let refuse = |error: &str| (400, json!({ "error": error }).to_string());
        if field("client_id") != CLIENT_ID || field("client_secret") != SECRET {
            return refuse("invalid_client");
        }
        if field("grant_type") != "authorization_code" || field("redirect_uri") != REDIRECT {
            return refuse("invalid_request");
        }
        let Some(grant) = self.grants.lock().unwrap().remove(field("code")) else {
            return refuse("invalid_grant");
        };
        if !pkce_matches(field("code_verifier"), &grant.challenge) {
            return refuse("invalid_grant");
        }
        let mut id_token = sign_id_token(&grant.claims);
        if grant.bad_signature {
            // The header and the claims of this token, the signature of
            // another: well formed, signed with the right key, and wrong.
            let other = sign_id_token(&json!({ "sub": "somebody else" }));
            let (signed, _) = id_token.rsplit_once('.').unwrap();
            let (_, signature) = other.rsplit_once('.').unwrap();
            id_token = format!("{signed}.{signature}");
        }
        (
            200,
            json!({
                "access_token": "mock-at",
                "token_type": "Bearer",
                "expires_in": 3600,
                "id_token": id_token,
            })
            .to_string(),
        )
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let length = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buf.len() < end + length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[end..]).into_owned();
    Some((head, body))
}

fn param(url: &str, name: &str) -> String {
    url::Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

struct Setup {
    mock: Arc<Mock>,
    provider: Provider,
    client: ClientConfig,
    clock: Arc<TestClock>,
    oauth: OauthClient<ReqwestClient, TestClock>,
}

async fn setup() -> Setup {
    let mock = Mock::start().await;
    let clock = Arc::new(TestClock::default());
    let http = Arc::new(ReqwestClient::with_default_timeout().unwrap());
    Setup {
        provider: mock.provider(),
        mock,
        client: ClientConfig::new(CLIENT_ID, SECRET, REDIRECT),
        oauth: OauthClient::new(http, clock.clone()),
        clock,
    }
}

impl Setup {
    fn begin(&self) -> AuthorizationRequest {
        self.oauth
            .begin(&self.provider, &self.client, &AuthorizeOptions::default())
            .unwrap()
    }

    fn now(&self) -> i64 {
        self.clock.now()
    }

    async fn complete(
        &self,
        started: &AuthorizationRequest,
        state: &str,
    ) -> Result<brocanth_auth::oauth::Authenticated, AuthError> {
        let callback = CallbackParams {
            code: "the-code".into(),
            state: state.to_string(),
            ..Default::default()
        };
        self.oauth
            .complete(&self.provider, &self.client, &started.flow, &callback)
            .await
    }
}

#[tokio::test]
async fn a_correct_flow_signs_in_over_a_real_socket() {
    let s = setup().await;
    let started = s.begin();
    s.mock.consent(
        "the-code",
        &started.url,
        s.mock.claims(&started, s.now()),
        false,
    );
    let done = s.complete(&started, &started.flow.state).await.unwrap();
    assert_eq!(done.identity.provider, "mock");
    assert_eq!(done.identity.subject, "mock-subject-1");
    assert!(done.identity.email_verified);
    assert_eq!(done.identity.name.as_deref(), Some("Ada Lovelace"));
    assert_eq!(done.tokens.access_token, "mock-at");
}

#[tokio::test]
async fn a_state_mismatch_is_refused_before_the_provider_is_asked() {
    let s = setup().await;
    let started = s.begin();
    s.mock.consent(
        "the-code",
        &started.url,
        s.mock.claims(&started, s.now()),
        false,
    );
    let err = s.complete(&started, "forged-state").await.unwrap_err();
    assert!(matches!(err, AuthError::StateMismatch), "{err:?}");
    assert_eq!(s.mock.token_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_pkce_verifier_that_is_not_the_challenges_preimage_is_refused_by_the_provider() {
    let s = setup().await;
    let mut started = s.begin();
    s.mock.consent(
        "the-code",
        &started.url,
        s.mock.claims(&started, s.now()),
        false,
    );
    // A stolen code, redeemed with a verifier of the thief's own.
    started.flow.pkce_verifier = "a".repeat(43);
    let err = s
        .complete(&started, &started.flow.state.clone())
        .await
        .unwrap_err();
    match err {
        AuthError::ProviderRefused { error, .. } => assert_eq!(error, "invalid_grant"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(s.mock.token_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_nonce_mismatch_is_refused() {
    let s = setup().await;
    let started = s.begin();
    let mut claims = s.mock.claims(&started, s.now());
    claims["nonce"] = json!("a-nonce-from-another-flow");
    s.mock.consent("the-code", &started.url, claims, false);
    let err = s.complete(&started, &started.flow.state).await.unwrap_err();
    assert!(matches!(err, AuthError::NonceMismatch), "{err:?}");
}

#[tokio::test]
async fn an_id_token_with_a_bad_signature_is_refused() {
    let s = setup().await;
    let started = s.begin();
    s.mock.consent(
        "the-code",
        &started.url,
        s.mock.claims(&started, s.now()),
        true,
    );
    let err = s.complete(&started, &started.flow.state).await.unwrap_err();
    match err {
        AuthError::ProviderResponse { detail, .. } => {
            assert!(detail.contains("did not verify"), "{detail}")
        }
        other => panic!("expected a refused id_token, got {other:?}"),
    }
}

#[tokio::test]
async fn an_id_token_for_another_audience_is_refused() {
    let s = setup().await;
    let started = s.begin();
    let mut claims = s.mock.claims(&started, s.now());
    claims["aud"] = json!("another-client");
    s.mock.consent("the-code", &started.url, claims, false);
    let err = s.complete(&started, &started.flow.state).await.unwrap_err();
    match err {
        AuthError::ProviderResponse { detail, .. } => {
            assert!(detail.to_lowercase().contains("audience"), "{detail}")
        }
        other => panic!("expected a refused id_token, got {other:?}"),
    }
}

#[tokio::test]
async fn an_id_token_that_also_names_an_untrusted_audience_is_refused() {
    let s = setup().await;
    let started = s.begin();
    let mut claims = s.mock.claims(&started, s.now());
    claims["aud"] = json!([CLIENT_ID, "another-client"]);
    s.mock.consent("the-code", &started.url, claims, false);
    let err = s.complete(&started, &started.flow.state).await.unwrap_err();
    assert!(matches!(err, AuthError::ProviderResponse { .. }), "{err:?}");
}

#[tokio::test]
async fn an_expired_id_token_is_refused() {
    let s = setup().await;
    let started = s.begin();
    let mut claims = s.mock.claims(&started, s.now());
    // Past the minute of leeway for a provider's clock.
    claims["iat"] = json!(s.now() - 3600);
    claims["exp"] = json!(s.now() - 120);
    s.mock.consent("the-code", &started.url, claims, false);
    let err = s.complete(&started, &started.flow.state).await.unwrap_err();
    match err {
        AuthError::ProviderResponse { detail, .. } => {
            assert!(detail.to_lowercase().contains("expired"), "{detail}")
        }
        other => panic!("expected a refused id_token, got {other:?}"),
    }
}

#[tokio::test]
async fn an_id_token_from_another_issuer_is_refused() {
    let s = setup().await;
    let started = s.begin();
    let mut claims = s.mock.claims(&started, s.now());
    claims["iss"] = json!("https://evil.test");
    s.mock.consent("the-code", &started.url, claims, false);
    let err = s.complete(&started, &started.flow.state).await.unwrap_err();
    assert!(matches!(err, AuthError::ProviderResponse { .. }), "{err:?}");
}
